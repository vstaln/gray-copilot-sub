//! Binary resolution and the fixed ACP argv / child env for the official
//! GitHub Copilot CLI.
//!
//! The sidecar takes no credentials. Sign-in is the Copilot CLI's own job
//! (`copilot login`), and its token lives in the CLI's keychain or config.
//! This module never opens either one.

use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const INSTALL_HINT: &str = "GitHub Copilot CLI not found. Install it (npm package \
    `@github/copilot`) or set GRAY_COPILOT_SUB_COMMAND=/path/to/copilot.";
pub const LOGIN_HINT: &str = "GitHub Copilot CLI is not signed in. Run `copilot login` in a \
    terminal (or `/login` inside Copilot CLI), then retry. This plugin never sees your GitHub token.";

const BINARY_ENV: &str = "GRAY_COPILOT_SUB_COMMAND";

/// Built-in tools named in the Copilot CLI reference ("Tool availability
/// values": shell, file-operation, agent/task, and other tools). Removing
/// them leaves the model with text only; the host's own tools arrive as
/// `mcp__gray__*` through the bridge.
pub const EXCLUDED_TOOLS: &[&str] = &[
    "bash",
    "powershell",
    "list_bash",
    "list_powershell",
    "read_bash",
    "read_powershell",
    "stop_bash",
    "stop_powershell",
    "write_bash",
    "write_powershell",
    "apply_patch",
    "create",
    "edit",
    "view",
    "list_agents",
    "read_agent",
    "task",
    "write_agent",
    "ask_user",
    "glob",
    "grep",
    "rg",
    "skill",
    "web_fetch",
];

/// `copilot` on PATH, or the explicit override. `None` = not installed.
pub fn resolve_command() -> Option<String> {
    if let Ok(explicit) = std::env::var(BINARY_ENV) {
        let explicit = explicit.trim();
        if !explicit.is_empty() {
            return Some(explicit.to_string());
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["copilot", "copilot.exe", "copilot.cmd"] {
            let candidate: PathBuf = dir.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Argv for the ACP child. `--acp` starts the documented ACP server, and
/// stdio is its default transport. `--excluded-tools` and `--no-ask-user`
/// are fixed for the whole server, so no session can re-enable them.
pub fn acp_argv() -> Vec<String> {
    vec![
        "--acp".into(),
        "--stdio".into(),
        format!("--excluded-tools={}", EXCLUDED_TOOLS.join(",")),
        "--no-ask-user".into(),
    ]
}

/// Child env: blanket tool approval is forced off, so an inherited
/// `COPILOT_ALLOW_ALL=true` cannot skip the permission gate.
pub fn child_env() -> Vec<(String, String)> {
    vec![("COPILOT_ALLOW_ALL".into(), "false".into())]
}

/// `copilot --version`, first non-empty line. `None` when unavailable.
pub fn cli_version() -> Option<String> {
    let bin = resolve_command()?;
    let out = Command::new(&bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// The sidecar's own state dir (`$GRAY_HOME/copilot` or `~/.gray/copilot`).
fn state_dir() -> PathBuf {
    let home = std::env::var_os("GRAY_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".gray")))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("copilot")
}

/// Left behind by the retired in-plugin device login, which held a GitHub
/// token minted through a spoofed VS Code client. The file is reported,
/// never read or deleted by this plugin.
pub fn legacy_token_file() -> Option<PathBuf> {
    let p = state_dir().join("config.json");
    p.is_file().then_some(p)
}
