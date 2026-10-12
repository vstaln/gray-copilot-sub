//! copilot-sub — Gray sidecar for GitHub Copilot. It relays chat turns to
//! the official GitHub Copilot CLI's ACP server (`copilot --acp --stdio`).
//! The CLI does all authentication and model traffic. The sidecar is a
//! model, not an agent runtime: the CLI's built-in tools are excluded at
//! server start, and only the host's own `mcp__gray__*` tools may run.
//!
//! This plugin never reads a GitHub or Copilot token, never calls a Copilot
//! internal endpoint, and never sends an editor identity it does not own.

use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use acp_bridge::manifest as pmanifest;
use acp_bridge::relay::{self, Intents, TurnFn};
use acp_bridge::session::{self, Pool};
use copilot_sub::driver::CopilotDriver;
use copilot_sub::models::{self, Probe};
use copilot_sub::{setup, usage};
use serde_json::{json, Value};

const NAME: &str = "copilot";
const PROVIDER_ID: &str = "copilot";
const DISPLAY: &str = "GitHub Copilot";
const AUTH_ID: &str = "copilot-cli-login";
const AUTH_NAME: &str = "GitHub Copilot CLI login";
const TOKEN_PREFIX: &str = "copilot-sub-";
const REVOKE_MSG: &str = "Copilot credentials belong to the Copilot CLI. Sign out there with \
    /logout. This plugin stores no credentials to revoke.";

fn manifest() -> Value {
    json!({
        "name": NAME,
        "display_name": DISPLAY,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.2",
        "capabilities": ["provider.credentials"],
        "providers": [pmanifest::provider_decl(
            PROVIDER_ID, DISPLAY, AUTH_ID, AUTH_NAME, "oauth",
            &["login", "refresh", "revoke", "models", "chat"], 0,
        )],
        "commands": ["/copilot"],
        "subcommands": ["copilot"],
    })
}

// ---------- errors ----------

#[derive(Debug)]
enum RpcError {
    Protocol(String),
    Unavailable(String),
    Rpc { code: &'static str, message: String },
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::Unavailable(msg.into())
}

fn error_value(e: &RpcError) -> Value {
    match e {
        RpcError::Rpc { code, message } => json!({
            "code": code, "message": message, "retryable": false, "terminal": true,
        }),
        RpcError::Unavailable(message) => json!({
            "code": "unavailable", "message": message, "retryable": true, "terminal": false,
        }),
        RpcError::Protocol(message) => json!({
            "code": "protocol", "message": message, "retryable": false, "terminal": true,
        }),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Map a failed probe to the host-facing error (install / sign-in hints).
fn probe_error(p: Probe) -> RpcError {
    match p {
        Probe::NotInstalled => unavailable(setup::INSTALL_HINT),
        Probe::NotSignedIn(_) => unavailable(setup::LOGIN_HINT),
        Probe::Failed(e) => unavailable(e),
        Probe::Ready(_) => RpcError::Protocol("unexpected probe state".into()),
    }
}

// ---------- auth (the CLI owns the login; the sidecar only checks it) ----------

/// Marker credential for the host. It is a flag, not a token: the real
/// credential stays in the Copilot CLI.
fn login_material() -> Value {
    json!({
        "secrets": {"copilot_cli_login": "1"},
        "metadata": {"cli_version": setup::cli_version().unwrap_or_default()},
        "expires_at": null,
    })
}

fn auth_start(_params: &Value) -> Result<Value, RpcError> {
    match models::probe() {
        Probe::Ready(_) => Ok(json!({
            "operation_id": "",
            "status": "completed",
            "verification_uri": "",
            "expires_at": now_secs() + 600,
            "retry_after_ms": 0,
        })),
        other => Err(probe_error(other)),
    }
}

fn auth_poll(_params: &Value) -> Result<Value, RpcError> {
    Ok(match models::probe() {
        Probe::Ready(_) => json!({"state": "completed", "credential": login_material()}),
        Probe::NotSignedIn(_) => failed("unavailable", setup::LOGIN_HINT, false),
        Probe::NotInstalled => failed("unavailable", setup::INSTALL_HINT, false),
        Probe::Failed(e) => failed("unavailable", &e, true),
    })
}

fn failed(code: &str, message: &str, retryable: bool) -> Value {
    json!({"state": "failed", "error": {
        "code": code, "message": message, "retryable": retryable, "terminal": !retryable,
    }})
}

fn auth_refresh(_params: &Value) -> Result<Value, RpcError> {
    match models::probe() {
        Probe::Ready(_) => Ok(json!({"credential": login_material()})),
        Probe::NotSignedIn(_) => Err(RpcError::Rpc {
            code: "invalid_grant",
            message: setup::LOGIN_HINT.into(),
        }),
        other => Err(probe_error(other)),
    }
}

// ---------- models + chat ----------

fn provider_models(_params: &Value) -> Result<Value, RpcError> {
    let rows = models::catalog().map_err(unavailable)?;
    let default_model = rows
        .first()
        .and_then(|m| m.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "provider": PROVIDER_ID,
        "models": rows,
        "default_model": default_model,
    }))
}

fn provider_chat(intents: &Intents, pool: &Arc<Pool>, params: &Value) -> Result<Value, RpcError> {
    let provider = params
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if provider != PROVIDER_ID {
        return Err(RpcError::Protocol("unknown provider".into()));
    }
    if setup::resolve_command().is_none() {
        return Err(unavailable(setup::INSTALL_HINT));
    }
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pool2 = pool.clone();
    let turn: TurnFn = Arc::new(move |raw: &[u8], cancel: &AtomicBool| {
        session::run_turn(&pool2, raw, cancel).map_err(|e| {
            if models::is_auth_error(&e) {
                format!("{e} — {}", setup::LOGIN_HINT)
            } else {
                e
            }
        })
    });
    let (relay_url, relay_token) =
        relay::mint_turn(intents, model, turn, TOKEN_PREFIX).map_err(unavailable)?;
    Ok(json!({
        "relay_url": relay_url,
        "relay_token": relay_token,
    }))
}

// ---------- command surface ----------

fn help_text() -> String {
    format!(
        "{NAME}-sub — {DISPLAY} provider for gray (official Copilot CLI over ACP)\n\
         usage: {NAME}-sub manifest | sidecar | status | models | login | logout\n\
         slash: /{NAME} status\n\
         login:  run `copilot login` in a terminal (this plugin never signs in itself)\n\
         logout: run /logout inside Copilot CLI\n\
         env: GRAY_COPILOT_SUB_COMMAND (binary) · GRAY_COPILOT_SUB_MODELS (fallback list)"
    )
}

fn status_text() -> String {
    let cli = match setup::resolve_command() {
        Some(bin) => match setup::cli_version() {
            Some(v) => format!("{bin} ({v})"),
            None => bin,
        },
        None => "not installed".to_string(),
    };
    let sign_in = match models::probe() {
        Probe::Ready(_) => "signed in (checked through the Copilot CLI's ACP server)".to_string(),
        Probe::NotSignedIn(_) => format!("not signed in: {}", setup::LOGIN_HINT),
        Probe::NotInstalled => setup::INSTALL_HINT.to_string(),
        Probe::Failed(e) => format!("unknown: {e}"),
    };
    let mut out = format!(
        "{DISPLAY} (official Copilot CLI, ACP)\n  cli: {cli}\n  sign-in: {sign_in}\n  \
         transport: copilot --acp --stdio (this plugin reads no tokens)"
    );
    if let Some(path) = setup::legacy_token_file() {
        out.push_str(&format!(
            "\n  legacy: {} is left over from the retired in-plugin login. This plugin does \
             not read it. Delete it, and remove the GitHub OAuth authorization you granted \
             for that login in your GitHub account settings.",
            path.display()
        ));
    }
    out
}

fn run_cli(argv: &[String]) -> Result<String, String> {
    match argv.first().map(String::as_str) {
        None | Some("status") => Ok(status_text()),
        Some("models") => models::catalog().map(|rows| {
            rows.iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Some("login") => Ok(format!("{}\n\n{}", setup::LOGIN_HINT, help_text())),
        Some("logout") => Ok(REVOKE_MSG.to_string()),
        Some("help") | Some("--help") | Some("-h") => Ok(help_text()),
        Some(other) => Err(format!("unknown verb {other:?}\n\n{}", help_text())),
    }
}

fn command_run(params: &Value) -> Result<Value, RpcError> {
    let argv: Vec<String> = params
        .get("argv")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let text = run_cli(&argv).unwrap_or_else(|e| e);
    Ok(json!({"text": text}))
}

// ---------- wire loop ----------

fn dispatch(
    method: &str,
    params: &Value,
    intents: &Intents,
    pool: &Arc<Pool>,
) -> Result<Value, RpcError> {
    match method {
        "plugin/manifest" => Ok(manifest()),
        "provider/auth/start" => auth_start(params),
        "provider/auth/poll" => auth_poll(params),
        "provider/auth/cancel" => Ok(json!({})),
        "provider/auth/refresh" => auth_refresh(params),
        "provider/auth/revoke" => Ok(json!({"status": "unsupported", "message": REVOKE_MSG})),
        "provider/usage" => usage::handle().map_err(unavailable),
        "provider/models" => provider_models(params),
        "provider/chat" => provider_chat(intents, pool, params),
        "command/run" => command_run(params),
        "plugin/tools" => Ok(json!({"tools": []})),
        "event/notify" | "plugin/shutdown" => Ok(json!({})),
        other => Err(RpcError::Protocol(format!("method not found: {other}"))),
    }
}

fn write_response(out: &mut impl Write, id: Value, outcome: Result<Value, RpcError>) -> bool {
    let response = match outcome {
        Ok(result) => json!({"id": id, "result": result}),
        Err(e) => json!({"id": id, "error": error_value(&e)}),
    };
    writeln!(out, "{response}")
        .and_then(|()| out.flush())
        .is_ok()
}

fn serve() {
    let intents: Intents = Default::default();
    let pool: Arc<Pool> = Pool::new(Arc::new(CopilotDriver));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let shutdown = method == "plugin/shutdown";
        match req.get("id").cloned() {
            // Notifications carry no id: only a shutdown notification matters.
            None if shutdown => return,
            None => continue,
            Some(id) => {
                let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
                let result = dispatch(&method, &params, &intents, &pool);
                if !write_response(&mut out, id, result) {
                    return;
                }
                if shutdown {
                    return;
                }
            }
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(NAME) {
        args.remove(0);
    }
    match args.first().map(String::as_str) {
        Some("manifest") => println!("{}", manifest()),
        Some("sidecar") => serve(),
        None if !std::io::stdin().is_terminal() => serve(),
        None => println!("{}", help_text()),
        Some(_) => match run_cli(&args) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
    }
}
