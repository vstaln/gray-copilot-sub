//! Fake `copilot --acp` over the real ACP transport. The official CLI is
//! NOT installed on the test machine (`which copilot` is empty), so the
//! probe and catalog are exercised against this stand-in. It is labeled as
//! such. The real CLI's auth-error text and models field are unverified.
#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use copilot_sub::models::{self, Probe};
use copilot_sub::usage;

static ENV_LOCK: Mutex<()> = Mutex::new(());

const FAKE: &str = r#"#!/usr/bin/env python3
import json, os, sys
log = os.environ.get("FAKE_COPILOT_LOG")
if log:
    with open(log, "a") as f:
        f.write(json.dumps(sys.argv[1:]) + "\n")
if "--version" in sys.argv:
    print("GitHub Copilot CLI 0.0.0-fake")
    sys.exit(0)
if "--acp" not in sys.argv:
    sys.stderr.write("fake copilot: expected --acp\n")
    sys.exit(2)
mode = os.environ.get("FAKE_COPILOT_MODE", "ready")
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    rid, method = req["id"], req.get("method")
    if method == "initialize":
        out = {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}
    elif method == "session/new":
        if mode == "unauth":
            out = None
            print(json.dumps({"jsonrpc": "2.0", "id": rid,
                              "error": {"code": -32000, "message": "Authentication required"}}))
            sys.stdout.flush()
            continue
        out = {"sessionId": "fake-session",
               "models": {"availableModels": [
                   {"modelId": "fake-model-a", "name": "Fake A"},
                   {"modelId": "fake-model-b", "name": "Fake B"}],
                   "currentModelId": "fake-model-a"}}
    else:
        out = {}
    print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": out}))
    sys.stdout.flush()
"#;

struct Fake {
    dir: PathBuf,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("copilot-sub-fake-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("copilot");
        let mut f = std::fs::File::create(&bin).unwrap();
        f.write_all(FAKE.as_bytes()).unwrap();
        drop(f);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fake { dir }
    }
    fn bin(&self) -> String {
        self.dir.join("copilot").to_string_lossy().into_owned()
    }
    fn log(&self) -> PathBuf {
        self.dir.join("argv.log")
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn with_fake<T>(mode: &str, f: impl FnOnce(&Fake) -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fake = Fake::new(mode);
    std::env::set_var("GRAY_COPILOT_SUB_COMMAND", fake.bin());
    std::env::set_var("FAKE_COPILOT_MODE", mode);
    std::env::set_var("FAKE_COPILOT_LOG", fake.log());
    let out = f(&fake);
    std::env::remove_var("GRAY_COPILOT_SUB_COMMAND");
    std::env::remove_var("FAKE_COPILOT_MODE");
    std::env::remove_var("FAKE_COPILOT_LOG");
    out
}

#[test]
fn ready_probe_reports_models_the_cli_listed() {
    with_fake("ready", |fake| {
        match models::probe() {
            Probe::Ready(rows) => {
                let ids: Vec<String> = models::model_rows(&rows)
                    .iter()
                    .map(|r| r["id"].as_str().unwrap().to_string())
                    .collect();
                assert_eq!(ids, vec!["fake-model-a", "fake-model-b"]);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        let argv = std::fs::read_to_string(fake.log()).unwrap();
        assert!(argv.contains("--acp"), "argv: {argv}");
        assert!(argv.contains("--stdio"), "argv: {argv}");
        assert!(argv.contains("--excluded-tools="), "argv: {argv}");
        assert!(argv.contains("--no-ask-user"), "argv: {argv}");
    });
}

#[test]
fn unauthenticated_session_reports_not_signed_in() {
    with_fake("unauth", |_| match models::probe() {
        Probe::NotSignedIn(_) => {}
        other => panic!("expected NotSignedIn, got {other:?}"),
    });
}

#[test]
fn catalog_and_usage_follow_the_probe() {
    with_fake("ready", |_| {
        let rows = models::catalog().expect("catalog");
        assert_eq!(rows.len(), 2);
        let card = usage::handle().expect("usage");
        assert_eq!(card["available"], true);
        assert_eq!(card["windows"], serde_json::json!([]));
    });
    with_fake("unauth", |_| {
        assert!(models::catalog().unwrap_err().contains("copilot login"));
        assert!(usage::handle().unwrap_err().contains("copilot login"));
    });
}

#[test]
fn missing_binary_is_not_installed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(
        "GRAY_COPILOT_SUB_COMMAND",
        "/nonexistent/dir/copilot-does-not-exist",
    );
    let probe = models::probe();
    std::env::remove_var("GRAY_COPILOT_SUB_COMMAND");
    assert_eq!(probe, Probe::NotInstalled);
}
