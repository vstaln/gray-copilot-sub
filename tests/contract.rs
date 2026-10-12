//! Contract tests: argv, child env, permission gate, model extraction, and
//! a source guard that fails if an internal endpoint or a spoofed editor
//! identity ever comes back.

use acp_bridge::session::{Driver, PermissionDecision};
use copilot_sub::driver::CopilotDriver;
use copilot_sub::models::{extract_models, is_auth_error, model_rows};
use copilot_sub::setup;
use serde_json::json;

#[test]
fn argv_starts_the_documented_acp_server_over_stdio() {
    let argv = setup::acp_argv();
    assert_eq!(argv[0], "--acp");
    assert!(argv.contains(&"--stdio".to_string()));
    assert!(argv.contains(&"--no-ask-user".to_string()));
}

#[test]
fn argv_excludes_every_documented_builtin_tool() {
    let argv = setup::acp_argv();
    let excluded = argv
        .iter()
        .find_map(|a| a.strip_prefix("--excluded-tools="))
        .expect("--excluded-tools=<list> must be present");
    let names: Vec<&str> = excluded.split(',').collect();
    for name in [
        "bash",
        "powershell",
        "view",
        "create",
        "edit",
        "apply_patch",
        "glob",
        "grep",
        "rg",
        "web_fetch",
        "task",
        "ask_user",
        "skill",
        "read_agent",
        "write_agent",
    ] {
        assert!(names.contains(&name), "missing excluded tool {name}");
    }
    assert_eq!(names.len(), setup::EXCLUDED_TOOLS.len());
}

#[test]
fn child_env_forces_blanket_approval_off() {
    let env = setup::child_env();
    assert!(env.contains(&("COPILOT_ALLOW_ALL".to_string(), "false".to_string())));
}

#[test]
fn spawn_spec_uses_the_resolved_command_and_argv() {
    let spec = CopilotDriver.spawn_spec(std::path::Path::new("/tmp"));
    assert_eq!(spec.args, setup::acp_argv());
    assert!(!spec.command.is_empty());
}

fn perm(title: &str, kind: &str) -> serde_json::Value {
    json!({
        "toolCall": {"title": title, "kind": kind},
        "options": [
            {"kind": "allow_always", "optionId": "always"},
            {"kind": "allow_once", "optionId": "once"},
            {"kind": "reject_once", "optionId": "no"},
        ],
    })
}

#[test]
fn permission_gate_denies_copilot_builtin_tools() {
    for title in [
        "Run: cat /etc/passwd",
        "Edit gray/src/main.rs",
        "shell: git push",
        "fetch https://x",
    ] {
        match CopilotDriver.permission_option(&perm(title, "execute")) {
            PermissionDecision::Deny => {}
            PermissionDecision::Allow(id) => panic!("allowed {title:?} as {id}"),
        }
    }
}

#[test]
fn permission_gate_allows_only_host_tools_and_prefers_allow_once() {
    match CopilotDriver.permission_option(&perm("mcp__gray__web_search", "other")) {
        PermissionDecision::Allow(id) => assert_eq!(id, "once"),
        PermissionDecision::Deny => panic!("host tool must be allowed"),
    }
}

#[test]
fn extract_models_reads_available_models_and_config_options() {
    let listed = json!({"models": {"availableModels": [{"modelId": "m1", "name": "M1"}], "currentModelId": "m1"}});
    assert_eq!(extract_models(&listed).len(), 1);
    let flat = json!({"models": [{"value": "m2", "name": "M2"}]});
    assert_eq!(model_rows(&extract_models(&flat))[0]["id"], "m2");
    let opts =
        json!({"configOptions": [{"id": "model", "options": [{"value": "m3", "name": "M3"}]}]});
    assert_eq!(model_rows(&extract_models(&opts))[0]["name"], "M3");
    assert!(extract_models(&json!({})).is_empty());
}

#[test]
fn auth_errors_are_recognized_and_other_errors_are_not() {
    assert!(is_auth_error("session/new: -32000 Authentication required"));
    assert!(is_auth_error("not logged in"));
    assert!(!is_auth_error("session/new: -32603 internal error"));
}

#[test]
fn sources_never_reference_internal_endpoints_or_spoofed_identity() {
    let sources = [
        ("lib.rs", include_str!("../src/lib.rs")),
        ("main.rs", include_str!("../src/main.rs")),
        ("setup.rs", include_str!("../src/setup.rs")),
        ("driver.rs", include_str!("../src/driver.rs")),
        ("models.rs", include_str!("../src/models.rs")),
        ("usage.rs", include_str!("../src/usage.rs")),
    ];
    let banned = [
        "githubcopilot",
        "copilot_internal",
        "Editor-Version",
        "Editor-Plugin-Version",
        "Copilot-Integration-Id",
        "GitHubCopilotChat",
        "Iv1.",
        "login/device",
        "login/oauth",
        "api.github.com",
        "Authorization",
        "Bearer ",
        "COPILOT_OAUTH_TOKEN",
        "COPILOT_CLIENT_ID",
    ];
    for (name, text) in sources {
        for needle in banned {
            assert!(!text.contains(needle), "{name} contains banned `{needle}`");
        }
    }
}
