//! Structured operations for the Windows Herdr SSH bridge. These never run in a shell.
use crate::hosts::host::Host;
use serde_json::Value;

pub const PREFIX: &str = "# inyon-herdr-rpc-v1\n";

pub fn enabled(host: &Host) -> bool {
    host.config().ssh_mode.as_deref() == Some("windows-herdr")
}

pub fn command(request: Value) -> String {
    format!("{PREFIX}{request}")
}
