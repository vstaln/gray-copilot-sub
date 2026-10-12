//! copilot-sub library: the GitHub Copilot CLI (ACP) driver, the sign-in
//! and model probe, the usage card, and the setup constants. The binary
//! (`main.rs`) wires these to the gray provider protocol.

pub mod driver;
pub mod models;
pub mod setup;
pub mod usage;
