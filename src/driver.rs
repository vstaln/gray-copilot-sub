//! Copilot ACP driver: wire-level quirks for the shared bridge.
//!
//! The Copilot CLI runs as `copilot --acp --stdio` (the documented ACP
//! server). Its built-in tools are removed at server start (see
//! `setup::acp_argv`). Any permission request it still raises is refused,
//! except for the host's own `mcp__gray__*` tools.

use std::path::Path;

use acp_bridge::acp::{AcpClient, SpawnSpec};
use acp_bridge::session::{Driver, PermissionDecision};
use serde_json::{json, Value};

use crate::setup;

/// Prefix the host's own tools carry in the bridge (`mcp__gray__*`).
const HOST_TOOL_PREFIX: &str = "mcp__gray__";

pub struct CopilotDriver;

impl Driver for CopilotDriver {
    fn spawn_spec(&self, stage_dir: &Path) -> SpawnSpec {
        SpawnSpec {
            command: setup::resolve_command().unwrap_or_else(|| "copilot".to_string()),
            args: setup::acp_argv(),
            cwd: stage_dir.to_path_buf(),
            env: setup::child_env(),
        }
    }

    fn client_info(&self) -> (&'static str, &'static str) {
        ("gray-copilot-sub", env!("CARGO_PKG_VERSION"))
    }

    fn client_capabilities(&self) -> Value {
        json!({
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
        })
    }

    /// Copilot signs in through its own CLI (`copilot login`). The ACP
    /// `authenticate` step is never sent with a token from this plugin.
    fn auth_method_id(&self) -> Option<&'static str> {
        None
    }

    /// Only the host's `mcp__gray__*` tools may run. Copilot's own tools are
    /// refused. The match is exact on the prefix, so a shell title that
    /// merely mentions a path cannot pass the gate.
    fn permission_option(&self, params: &Value) -> PermissionDecision {
        let tool = params.get("toolCall");
        let title = tool
            .and_then(|t| t.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = tool
            .and_then(|t| t.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !(title.contains(HOST_TOOL_PREFIX) || kind.contains(HOST_TOOL_PREFIX)) {
            return PermissionDecision::Deny;
        }
        let id = params
            .get("options")
            .and_then(Value::as_array)
            .and_then(|o| {
                o.iter()
                    .find(|opt| opt.get("kind").and_then(Value::as_str) == Some("allow_once"))
                    .or_else(|| {
                        o.iter().find(|opt| {
                            opt.get("kind").and_then(Value::as_str) == Some("allow_always")
                        })
                    })
            })
            .and_then(|opt| opt.get("optionId").and_then(Value::as_str))
            .unwrap_or("allow_once")
            .to_string();
        PermissionDecision::Allow(id)
    }

    /// Model selection. An empty model keeps Copilot's default. Otherwise
    /// `session/set_model` is tried first, then a model-shaped configOption
    /// from session/new. Effort and bracket overrides go through configOptions.
    fn apply_model(
        &self,
        client: &mut AcpClient,
        session_id: &str,
        session_new_result: &Value,
        model: &str,
        effort: Option<&str>,
        overrides: &[(String, String)],
    ) -> Result<(), String> {
        if !model.is_empty() {
            if let Err(e) = client.set_model(session_id, model, Value::Null) {
                if !is_unsupported(&e) {
                    return Err(format!("copilot set_model: {e}"));
                }
                let option = find_config_option(session_new_result, "model")
                    .ok_or_else(|| format!("unknown Copilot model `{model}`"))?;
                client
                    .set_config_option(session_id, &option, json!(model))
                    .map_err(|e| format!("copilot model option: {e}"))?;
            }
        }
        apply_overrides(client, session_id, session_new_result, effort, overrides);
        Ok(())
    }
}

/// The method or the value is not supported by this CLI build.
fn is_unsupported(err: &str) -> bool {
    err.contains("-32601") || err.contains("not found") || err.contains("Invalid")
}

fn apply_overrides(
    client: &mut AcpClient,
    session_id: &str,
    new_result: &Value,
    effort: Option<&str>,
    overrides: &[(String, String)],
) {
    for (key, value) in overrides {
        if let Some(option) = find_config_option(new_result, key) {
            let _ = client.set_config_option(session_id, &option, json!(value));
        }
    }
    if let Some(e) = effort {
        if !overrides.iter().any(|(k, _)| k == "effort") {
            if let Some(option) = find_config_option(new_result, "effort") {
                let _ = client.set_config_option(session_id, &option, json!(e));
            }
        }
    }
}

/// Match a bracket override key ("model"/"effort"/"mode"/"thinking")
/// against the session/new configOptions by id-or-name.
fn find_config_option(new_result: &Value, key: &str) -> Option<String> {
    let opts = new_result.get("configOptions")?.as_array()?;
    let want = key.to_lowercase();
    opts.iter()
        .find(|o| {
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
            match want.as_str() {
                "model" => id.contains("model") || name.contains("model"),
                "effort" => {
                    ["effort", "reasoning_effort", "reasoning", "thought_level"]
                        .contains(&id.as_str())
                        || name.contains("effort")
                        || name.contains("reasoning")
                }
                "mode" => id == "mode" || name == "mode",
                "thinking" => id == "thinking" || name.contains("thinking"),
                other => id == other,
            }
        })
        .and_then(|o| o.get("id").and_then(Value::as_str).map(str::to_string))
}
