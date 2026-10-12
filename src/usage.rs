//! `provider/usage`: GitHub Copilot connection card.
//!
//! The Copilot CLI publishes no quota counter through its ACP server, and
//! the plugin does not query Copilot's internal quota endpoints. The card
//! reports the connection honestly, with no fabricated windows.

use serde_json::{json, Value};

use crate::models::{probe, Probe};
use crate::setup;

/// `provider/usage` entry: an error unless the CLI is installed and signed in.
pub fn handle() -> Result<Value, String> {
    match probe() {
        Probe::Ready(_) => Ok(json!({
            "available": true,
            "title": "GitHub Copilot",
            "windows": [],
            "note": "the Copilot CLI publishes no usage counter over ACP; check your Copilot account settings",
        })),
        Probe::NotInstalled => Err(setup::INSTALL_HINT.to_string()),
        Probe::NotSignedIn(_) => Err(setup::LOGIN_HINT.to_string()),
        Probe::Failed(e) => Err(e),
    }
}
