//! Sign-in probe and model catalog, both from one ACP round trip:
//! `initialize` + `session/new` against a throwaway `copilot --acp` child.
//! Models come from what the CLI reports, never from a hardcoded list.

use acp_bridge::acp::AcpClient;
use acp_bridge::session::Driver;
use serde_json::{json, Value};

use crate::driver::CopilotDriver;
use crate::setup;

/// What the probe found.
#[derive(Debug, PartialEq)]
pub enum Probe {
    /// Signed in. Carries whatever model rows the CLI reported (may be empty).
    Ready(Vec<Value>),
    NotInstalled,
    /// The CLI answered session/new with an authentication error.
    NotSignedIn(String),
    Failed(String),
}

/// Probe the installed CLI. The child runs in the temp dir and is killed on drop.
pub fn probe() -> Probe {
    if setup::resolve_command().is_none() {
        return Probe::NotInstalled;
    }
    let stage = std::env::temp_dir();
    let driver = CopilotDriver;
    let spec = driver.spawn_spec(&stage);
    let mut client = match AcpClient::spawn(&spec) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Probe::NotInstalled,
        Err(e) => return Probe::Failed(format!("spawn copilot: {e}")),
    };
    if let Err(e) = client.initialize(driver.client_info(), driver.client_capabilities()) {
        return Probe::Failed(format!("copilot initialize: {e}"));
    }
    match client.session_new(&stage.to_string_lossy(), json!([])) {
        Ok(sess) => Probe::Ready(extract_models(&sess)),
        Err(e) if is_auth_error(&e) => Probe::NotSignedIn(e),
        Err(e) => Probe::Failed(format!("copilot session/new: {e}")),
    }
}

/// An authentication failure from the CLI (JSON-RPC -32000 or a
/// "not signed in" style message).
pub fn is_auth_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("-32000")
        || m.contains("authentication required")
        || m.contains("not authenticated")
        || m.contains("not logged in")
        || m.contains("unauthorized")
}

/// Model rows from a session/new result: the `models` (or `availableModels`)
/// list, else a model-shaped configOption's options.
pub fn extract_models(sess: &Value) -> Vec<Value> {
    let listed = sess
        .get("models")
        .and_then(|m| m.get("availableModels").or(Some(m)))
        .or_else(|| sess.get("availableModels"))
        .and_then(Value::as_array);
    if let Some(list) = listed {
        if !list.is_empty() {
            return list.clone();
        }
    }
    if let Some(opts) = sess.get("configOptions").and_then(Value::as_array) {
        for o in opts {
            let id = o
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let name = o
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if !(id.contains("model") || name.contains("model")) {
                continue;
            }
            if let Some(options) = o.get("options").and_then(Value::as_array) {
                if !options.is_empty() {
                    return options.clone();
                }
            }
        }
    }
    Vec::new()
}

/// `{id, name}` rows for provider/models.
pub fn model_rows(raw: &[Value]) -> Vec<Value> {
    raw.iter()
        .filter_map(|m| {
            let id = m
                .get("value")
                .or_else(|| m.get("modelId"))
                .or_else(|| m.get("id"))
                .and_then(Value::as_str)?;
            Some(json!({
                "id": id,
                "name": m.get("name").and_then(Value::as_str).unwrap_or(id),
            }))
        })
        .collect()
}

/// Env fallback for when the CLI reports no models: `GRAY_COPILOT_SUB_MODELS`
/// ("id1,id2"). Explicit user input only, never a built-in list.
pub fn env_catalog() -> Vec<Value> {
    std::env::var("GRAY_COPILOT_SUB_MODELS")
        .ok()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|id| json!({"id": id, "name": id}))
                .collect()
        })
        .unwrap_or_default()
}

/// Catalog for provider/models: discovered rows, else the env list, else an error.
pub fn catalog() -> Result<Vec<Value>, String> {
    match probe() {
        Probe::Ready(raw) => {
            let rows = model_rows(&raw);
            if !rows.is_empty() {
                return Ok(rows);
            }
            let env = env_catalog();
            if env.is_empty() {
                Err("copilot reported no models; set GRAY_COPILOT_SUB_MODELS=id1,id2".to_string())
            } else {
                Ok(env)
            }
        }
        Probe::NotInstalled => Err(setup::INSTALL_HINT.to_string()),
        Probe::NotSignedIn(_) => Err(setup::LOGIN_HINT.to_string()),
        Probe::Failed(e) => Err(e),
    }
}
