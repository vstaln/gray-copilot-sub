//! copilot-sub — GitHub Copilot subscription provider for gray.
//!
//! Auth: GitHub OAuth device flow (`copilot-sub login` or the
//! `provider/auth/*` ops) → OAuth token in ~/.gray/copilot/config.json →
//! per-need Copilot session token from
//! api.github.com/copilot_internal/v2/token (~30min TTL, re-minted).
//! Chat rides the shared openai_compat relay to
//! https://api.githubcopilot.net/chat/completions with Copilot's
//! Editor-* headers.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acp_bridge::manifest as pmanifest;
use acp_bridge::relay::{self, Intents};
use openai_compat::{compat_turn, credential_key, probe_models, Compat};
use serde_json::{json, Value};

const NAME: &str = "copilot";
const PROVIDER_ID: &str = "copilot";
const DISPLAY: &str = "GitHub Copilot";
const AUTH_ID: &str = "github-device";
const AUTH_NAME: &str = "GitHub device login";
const TOKEN_PREFIX: &str = "copilot-sub-";
const API_BASE: &str = "https://api.githubcopilot.net";
const TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// The public VS Code Copilot client id every copilot-api bridge uses;
/// overridable via COPILOT_CLIENT_ID for forks.
const DEFAULT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const ENV_OAUTH: &str = "COPILOT_OAUTH_TOKEN";
const ENV_CLIENT: &str = "COPILOT_CLIENT_ID";
/// Editor headers Copilot's API expects.
const EDITOR_HEADERS: &[(&str, &str)] = &[
    ("Editor-Version", "vscode/1.95.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.26.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
    ("User-Agent", "GitHubCopilotChat/0.26.0"),
];
const FALLBACK_MODELS: &[&str] = &[
    "gpt-4o", "gpt-4.1", "gpt-5-mini", "claude-sonnet-4", "o4-mini",
];

// ---------- small helpers ----------

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn state_dir() -> std::path::PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".gray")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(NAME)
}
fn config_path() -> std::path::PathBuf { state_dir().join("config.json") }
fn read_config() -> Value {
    std::fs::read_to_string(config_path())
        .ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}))
}
fn write_config(v: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir())?;
    std::fs::write(config_path(), format!("{}\n", serde_json::to_string_pretty(v)?))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config_path(), std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn err(code: &str, msg: impl Into<String>, retryable: bool) -> Value {
    json!({"code": code, "message": msg.into(), "retryable": retryable,
           "terminal": !retryable})
}

fn http_json(method: &str, url: &str, body: Option<&Value>, headers: &[(String, String)])
    -> Result<Value, String>
{
    let mut req = match method {
        "GET" => ureq::get(url),
        _ => ureq::post(url),
    }
    .timeout(Duration::from_secs(30))
    .set("Accept", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = match body {
        Some(b) => req.set("Content-Type", "application/json").send_json(b),
        None => req.call(),
    };
    match resp {
        Ok(r) => r.into_json().map_err(|e| format!("JSON decode {url}: {e}")),
        Err(ureq::Error::Status(code, r)) => {
            let mut t = r.into_string().unwrap_or_default();
            if t.len() > 400 {
                let mut end = 400;
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                t.truncate(end);
            }
            Err(format!("HTTP {code} {url}: {t}"))
        }
        Err(e) => Err(format!("{e}")),
    }
}

// ---------- credentials ----------

fn oauth_token() -> Option<String> {
    std::env::var(ENV_OAUTH).ok().filter(|s| !s.is_empty())
        .or_else(|| read_config()["oauth_token"].as_str().map(str::to_string))
}

/// Mint (or reuse) a Copilot session token from the stored OAuth token.
fn session_token() -> Result<String, String> {
    let cfg = read_config();
    if let (Some(t), Some(exp)) = (
        cfg["session_token"].as_str(),
        cfg["session_expires_at"].as_u64(),
    ) {
        if exp > now() + 60 {
            return Ok(t.to_string());
        }
    }
    let oauth = oauth_token().ok_or(
        "no GitHub OAuth token — run `copilot-sub login` or /connect")?;
    let headers: Vec<(String, String)> = std::iter::once(("Authorization".into(), format!("token {oauth}")))
        .chain(EDITOR_HEADERS.iter().map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let v = http_json("GET", TOKEN_URL, None, &headers)?;
    let token = v["token"].as_str().ok_or("no token in copilot session reply")?;
    let exp = v["expires_at"].as_u64().unwrap_or(now() + 1500);
    let mut cfg = read_config();
    cfg["session_token"] = json!(token);
    cfg["session_expires_at"] = json!(exp);
    let _ = write_config(&cfg);
    Ok(token.to_string())
}

fn compat() -> Result<Compat, String> {
    let key = session_token()?;
    let mut c = Compat::new(API_BASE, Some(key));
    for (k, v) in EDITOR_HEADERS {
        c = c.header(*k, *v);
    }
    Ok(c)
}

// ---------- device flow ----------

#[derive(Clone)]
struct Pending {
    device_code: String,
    interval_ms: u64,
    deadline: u64,
}
type Ops = Arc<Mutex<HashMap<String, Pending>>>;

fn auth_start(ops: &Ops) -> Result<Value, Value> {
    if oauth_token().is_some() {
        return Ok(json!({"status": "authenticated"}));
    }
    let client = std::env::var(ENV_CLIENT).unwrap_or_else(|_| DEFAULT_CLIENT_ID.into());
    let v = http_json("POST", DEVICE_CODE_URL,
        Some(&json!({"client_id": client, "scope": "read:user"})), &[])
        .map_err(|e| err("unavailable", format!("device code: {e}"), true))?;
    let device_code = v["device_code"].as_str().ok_or_else(||
        err("protocol", "device flow reply missing device_code", false))?;
    let op_id = format!("copilot-{}", now());
    let expires = now() + v["expires_in"].as_u64().unwrap_or(900);
    let interval_ms = v["interval"].as_u64().unwrap_or(5) * 1000;
    ops.lock().map_err(|_| err("unavailable", "ops lock", true))?
        .insert(op_id.clone(), Pending {
            device_code: device_code.to_string(),
            interval_ms, deadline: expires,
        });
    Ok(json!({
        "operation_id": op_id,
        "status": "pending",
        "verification_uri": v["verification_uri_complete"]
            .as_str()
            .or_else(|| v["verification_uri"].as_str())
            .unwrap_or("https://github.com/login/device"),
        "user_code": v["user_code"],
        "expires_at": expires,
        "retry_after_ms": interval_ms,
    }))
}

fn auth_poll(ops: &Ops, params: &Value) -> Result<Value, Value> {
    let op_id = params["operation_id"].as_str().unwrap_or_default();
    let Some(p) = ops.lock().ok().and_then(|m| m.get(op_id).cloned()) else {
        return Ok(json!("operation_lost"));
    };
    if now() > p.deadline {
        return Ok(json!({"failed": {"code": "expired",
            "message": "device code expired", "retryable": false, "terminal": true}}));
    }
    let client = std::env::var(ENV_CLIENT).unwrap_or_else(|_| DEFAULT_CLIENT_ID.into());
    let v = http_json("POST", ACCESS_TOKEN_URL, Some(&json!({
        "client_id": client, "device_code": p.device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
    })), &[]).map_err(|e| err("unavailable", e, true))?;

    if let Some(tok) = v["access_token"].as_str() {
        ops.lock().ok().map(|mut m| m.remove(op_id));
        let mut cfg = read_config();
        cfg["oauth_token"] = json!(tok);
        cfg.as_object_mut().map(|m| { m.remove("session_token"); m.remove("session_expires_at"); });
        let _ = write_config(&cfg);
        return Ok(json!({"completed": {
            "secrets": {"oauth_token": tok},
            "metadata": {"kind": "github-device"},
        }}));
    }
    match v["error"].as_str().unwrap_or("") {
        "authorization_pending" => Ok(json!({"pending": {"retry_after_ms": p.interval_ms}})),
        "slow_down" => Ok(json!({"pending": {"retry_after_ms": p.interval_ms + 5000}})),
        other => Ok(json!({"failed": {"code": other,
            "message": v["error_description"].as_str().unwrap_or(other),
            "retryable": false, "terminal": true}})),
    }
}

// ---------- provider ops ----------

fn provider_models(params: &Value) -> Result<Value, Value> {
    // Host may deliver an oauth_token credential on this op — persist it.
    if let Some(k) = credential_key(params) {
        let mut cfg = read_config();
        cfg["oauth_token"] = json!(k);
        let _ = write_config(&cfg);
    }
    match compat().and_then(|c| probe_models(&c)) {
        Ok(rows) if !rows.is_empty() => Ok(json!({"models": rows})),
        _ => Ok(json!({"models": FALLBACK_MODELS.iter()
            .map(|id| json!({"id": id, "name": id})).collect::<Vec<_>>()})),
    }
}

fn provider_chat(intents: &Intents, params: &Value) -> Result<Value, Value> {
    let model = params["model"].as_str().unwrap_or_default();
    let cfg = Arc::new(compat().map_err(|e| err("unavailable", e, true))?);
    let (relay_url, relay_token) =
        relay::mint_turn(intents, model, compat_turn(cfg, None), TOKEN_PREFIX)
            .map_err(|e| err("unavailable", e, true))?;
    Ok(json!({"relay_url": relay_url, "relay_token": relay_token}))
}

// ---------- command surface ----------

fn cli_help() -> String {
    format!("{NAME}-sub — {DISPLAY} provider for gray\n\
        usage: {NAME}-sub manifest | sidecar | login | status | models | logout\n\
        slash:  /{NAME} status\n\
        config: ~/.gray/{NAME}/config.json · env {ENV_OAUTH} {ENV_CLIENT}")
}

fn cli_status() -> String {
    let cfg = read_config();
    let oauth = oauth_token().is_some();
    let sess = cfg["session_expires_at"].as_u64().unwrap_or(0);
    format!(
        "{DISPLAY}\n  oauth: {}\n  session token: {}\n  api: {API_BASE}",
        if oauth { "set" } else { "MISSING — run `copilot-sub login`" },
        if sess > now() { "valid" } else { "expired/absent" },
    )
}

/// Blocking device flow for humans at a terminal.
fn cli_login() -> Result<String, String> {
    let client = std::env::var(ENV_CLIENT).unwrap_or_else(|_| DEFAULT_CLIENT_ID.into());
    let v = http_json("POST", DEVICE_CODE_URL,
        Some(&json!({"client_id": client, "scope": "read:user"})), &[])?;
    let uri = v["verification_uri"].as_str().unwrap_or("https://github.com/login/device");
    let code = v["user_code"].as_str().unwrap_or("?");
    println!("Open {uri} and enter: {code}");
    let device_code = v["device_code"].as_str().ok_or("no device_code")?.to_string();
    let interval = v["interval"].as_u64().unwrap_or(5);
    let deadline = now() + v["expires_in"].as_u64().unwrap_or(900);
    while now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        let r = http_json("POST", ACCESS_TOKEN_URL, Some(&json!({
            "client_id": client, "device_code": device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        })), &[])?;
        if let Some(tok) = r["access_token"].as_str() {
            let mut cfg = read_config();
            cfg["oauth_token"] = json!(tok);
            write_config(&cfg).map_err(|e| e.to_string())?;
            return Ok(format!("logged in — session token minted: {}",
                session_token().map(|_| "ok").unwrap_or("pending")));
        }
        match r["error"].as_str().unwrap_or("") {
            "authorization_pending" => continue,
            "slow_down" => std::thread::sleep(Duration::from_secs(5)),
            e => return Err(format!("login failed: {e}")),
        }
    }
    Err("device code expired".into())
}

fn run_cli(argv: &[String]) -> Result<String, String> {
    match argv.first().map(String::as_str) {
        Some("status") | None => Ok(cli_status()),
        Some("login") => cli_login(),
        Some("logout") => {
            let _ = std::fs::remove_file(config_path());
            Ok("logged out".into())
        }
        Some("models") => match compat().and_then(|c| probe_models(&c)) {
            Ok(rows) => Ok(rows.iter()
                .filter_map(|m| m["id"].as_str()).collect::<Vec<_>>().join("\n")),
            Err(e) => Err(e),
        },
        Some("help") | Some("--help") | Some("-h") => Ok(cli_help()),
        Some(other) => Err(format!("unknown verb {other:?}\n\n{}", cli_help())),
    }
}

// ---------- wire loop ----------

fn manifest() -> Value {
    json!({
        "name": NAME,
        "display_name": DISPLAY,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.2",
        "capabilities": ["provider.credentials"],
        "providers": [pmanifest::provider_decl(
            PROVIDER_ID, DISPLAY, AUTH_ID, AUTH_NAME, "oauth",
            &["login", "models", "chat", "refresh", "revoke"], 300,
        )],
        "commands": ["/copilot"],
        "subcommands": ["copilot"],
    })
}

fn handle(intents: &Intents, ops: &Ops, method: &str, params: &Value) -> Value {
    macro_rules! wrap { ($e:expr) => {
        match $e { Ok(r) => json!({"result": r}), Err(e) => json!({"error": e}) } } }
    match method {
        "provider/auth/start" => wrap!(auth_start(ops)),
        "provider/auth/poll" => wrap!(auth_poll(ops, params)),
        "provider/auth/cancel" => {
            if let Some(id) = params["operation_id"].as_str() {
                ops.lock().ok().map(|mut m| m.remove(id));
            }
            json!({"result": {}})
        }
        "provider/auth/refresh" => json!({"result": {"status": "unsupported"}}),
        "provider/auth/revoke" => {
            let _ = std::fs::remove_file(config_path());
            json!({"result": {"status": "ok"}})
        }
        "provider/models" => wrap!(provider_models(params)),
        "provider/chat" => wrap!(provider_chat(intents, params)),
        "provider/usage" => json!({"result": {"windows": []}}),
        "command/run" => {
            let argv: Vec<String> = params["argv"].as_array()
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let text = run_cli(&argv).unwrap_or_else(|e| e);
            json!({"result": {"text": text}})
        }
        other => json!({"error": {"code": -32601,
            "message": format!("method not found: {other}")}}),
    }
}

fn sidecar() -> std::io::Result<()> {
    let intents = Intents::default();
    let ops = Ops::default();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let method = v["method"].as_str().unwrap_or("");
        match v.get("id").cloned() {
            None if method == "plugin/shutdown" => return Ok(()),
            None => {}
            Some(id) => {
                let reply = if method == "plugin/manifest" {
                    json!({"id": id, "result": manifest()})
                } else {
                    let mut r = handle(&intents, &ops, method, &v["params"]);
                    r["id"] = id;
                    r
                };
                writeln!(out, "{reply}")?;
                out.flush()?;
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<String> = match args.first().map(String::as_str) {
        Some(NAME) => args[1..].to_vec(),
        _ => args,
    };
    match args.first().map(String::as_str) {
        Some("manifest") => println!("{}", manifest()),
        Some("sidecar") => {
            if let Err(e) = sidecar() {
                eprintln!("{NAME}-sub sidecar: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            if args.is_empty() {
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    println!("{}", cli_help());
                    return;
                }
                if let Err(e) = sidecar() {
                    eprintln!("{NAME}-sub sidecar: {e}");
                    std::process::exit(1);
                }
                return;
            }
            match run_cli(&args) {
                Ok(t) => println!("{t}"),
                Err(e) => { eprintln!("{e}"); std::process::exit(1); }
            }
        }
    }
}
