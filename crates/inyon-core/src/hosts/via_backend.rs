//! Port of hosts/via-backend.ts.
//!
//! Servers reached over SSH from a paired computer rather than from the phone: a jump box, or a
//! laptop on the company VPN, that can already ssh to them. It runs `ssh` for each command with its
//! own keys, ~/.ssh/config and known_hosts, so nothing is installed on the server and the phone
//! needs no route to it and no key for it. One shared connection per server keeps commands cheap.

use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::{HostConfig, HostKind};

use super::host::{shq, ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use super::http_error::{HubError, HubResult};
use super::node_backend::NodeBackend;
use crate::platform::rt::Deferred;

pub fn via_command(c: &HostConfig, cmd: &str, tty: bool) -> String {
    let opts = [
        "BatchMode=yes",
        "ControlMaster=auto",
        "\"ControlPath=$HOME/.cache/inyon-ssh/%C\"",
        "ControlPersist=600",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ConnectTimeout=10",
        "StrictHostKeyChecking=accept-new",
    ]
    .map(|o| format!("-o {o}"));
    let port = c.port.map(|p| format!(" -p {p}")).unwrap_or_default();
    let target = c.target.as_deref().unwrap_or("undefined");
    format!(
        "mkdir -p \"$HOME/.cache/inyon-ssh\" 2>/dev/null; exec ssh {}{port} {} -- {} {}",
        opts.join(" "),
        if tty { "-tt" } else { "-T" },
        shq(target),
        shq(&format!("sh -c {}", shq(cmd)))
    )
}

/// `lookup` finds the paired computer a server names in `via`.
pub struct ViaBackend {
    lookup: Lookup,
}

type Lookup = Rc<dyn Fn(&str) -> Option<HostConfig>>;

pub fn via_backend(lookup: impl Fn(&str) -> Option<HostConfig> + 'static) -> Rc<dyn HostBackend> {
    Rc::new(ViaBackend { lookup: Rc::new(lookup) })
}

const GONE: &str = "The computer this server is reached through is no longer paired. Edit the server and pick another.";

/// the server itself runs nothing of Inyon's: when the computer isn't there, say so by name
fn away(v: &HostConfig) -> String {
    format!("{} isn't connected right now, and this server is reached through it.", v.name)
}

impl ViaBackend {
    fn through(&self, c: &HostConfig) -> Option<HostConfig> {
        let v = c.via.as_deref().and_then(|id| (self.lookup)(id))?;
        matches!(v.kind, HostKind::Link | HostKind::Node).then_some(v)
    }
}

impl HostBackend for ViaBackend {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        let Some(v) = self.through(config) else {
            return Box::pin(async { Ok(ExecResult { code: 255, stdout: String::new(), stderr: GONE.into() }) });
        };
        let run = NodeBackend.exec(&v, &via_command(config, cmd, false), opts);
        Box::pin(async move {
            match run.await {
                Ok(r) => Ok(r),
                Err(_) => Ok(ExecResult { code: 255, stdout: String::new(), stderr: away(&v) }),
            }
        })
    }

    fn stream(&self, config: &HostConfig, cmd: &str, on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        match self.through(config) {
            None => StreamHandle::new(|| {}, Deferred::resolved(255)),
            Some(v) => NodeBackend.stream(&v, &via_command(config, cmd, false), on_line, keep_stdin),
        }
    }

    // ends the computer's shared connection to the server, which would otherwise stay up ten minutes
    fn close(&self, config: &HostConfig) -> LocalBoxFuture<'static, ()> {
        let Some(v) = self.through(config) else { return Box::pin(async {}) };
        let port = config.port.map(|p| format!(" -p {p}")).unwrap_or_default();
        let target = config.target.as_deref().unwrap_or("undefined");
        let run = NodeBackend.exec(&v, &format!("ssh -o \"ControlPath=$HOME/.cache/inyon-ssh/%C\"{port} -O exit -- {} 2>/dev/null; true", shq(target)), ExecOptions::timeout(5000));
        Box::pin(async move {
            let _ = run.await;
        })
    }

    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle> {
        let v = self.through(config).ok_or_else(|| HubError::plain(GONE))?;
        NodeBackend.pty(&v, &via_command(config, cmd, true), cols, rows, on_data).map_err(|_| HubError::plain(away(&v)))
    }
}

#[cfg(test)]
#[path = "via_backend_tests.rs"]
mod tests;
