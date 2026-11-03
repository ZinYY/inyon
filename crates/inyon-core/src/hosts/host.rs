//! Port of hosts/host.ts: how commands reach a machine. The hub runs them as processes (directly,
//! or through the system `ssh`) and over inyon links; the phone app opens its own SSH
//! connections. Each is a [`HostBackend`]; the platform says which serves each machine.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::HostConfig;

use super::http_error::{HubError, HubResult};
use crate::platform::rt::Deferred;

/// `shq`: one argument for a POSIX shell, in single quotes.
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `tmuxLiteral`: a name or folder as tmux should take it literally. tmux expands formats in
/// `new-session -s/-c`, `new-window -c/-n`, `respawn-pane -c` and `rename-window`, and `#(cmd)`
/// runs a command, so a folder an agent made called `x#(curl …|sh)` would run that on Fork or
/// Restart. `##` is a plain `#`. psmux is left alone: whether it expands formats there isn't known
/// yet, and a doubled `#` would send a `C#` folder astray.
pub fn tmux_literal(s: &str, psmux: bool) -> String {
    if psmux {
        s.to_owned()
    } else {
        s.replace('#', "##")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    /// what the command reads on stdin
    pub input: Option<Vec<u8>>,
    /// 20 s when left out
    pub timeout_ms: Option<u64>,
}

impl ExecOptions {
    pub fn timeout(ms: u64) -> Self {
        ExecOptions { input: None, timeout_ms: Some(ms) }
    }

    pub fn input(text: impl Into<Vec<u8>>) -> Self {
        ExecOptions { input: Some(text.into()), timeout_ms: None }
    }

    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
}

/// A long-running command (`tail -F`, tmux control mode).
#[derive(Clone)]
pub struct StreamHandle {
    kill: Rc<dyn Fn()>,
    /// the exit code, once it has ended
    pub exited: Deferred<i32>,
}

impl StreamHandle {
    pub fn new(kill: impl Fn() + 'static, exited: Deferred<i32>) -> Self {
        StreamHandle { kill: Rc::new(kill), exited }
    }

    pub fn kill(&self) {
        (self.kill)()
    }

    /// The same stream (`a === b`).
    pub fn same(&self, other: &StreamHandle) -> bool {
        self.exited.same(&other.exited)
    }
}

/// An interactive command in a pseudo-terminal (terminal views).
#[derive(Clone)]
pub struct PtyHandle {
    write: Rc<dyn Fn(&str)>,
    resize: Rc<dyn Fn(u16, u16)>,
    kill: Rc<dyn Fn()>,
    pub exited: Deferred<i32>,
}

impl PtyHandle {
    pub fn new(write: impl Fn(&str) + 'static, resize: impl Fn(u16, u16) + 'static, kill: impl Fn() + 'static, exited: Deferred<i32>) -> Self {
        PtyHandle { write: Rc::new(write), resize: Rc::new(resize), kill: Rc::new(kill), exited }
    }
    pub fn write(&self, data: &str) {
        (self.write)(data)
    }
    pub fn resize(&self, cols: u16, rows: u16) {
        (self.resize)(cols, rows)
    }
    pub fn kill(&self) {
        (self.kill)()
    }
}

/// Called for each line a stream prints, on the core's thread.
pub type OnLine = Box<dyn FnMut(String)>;
/// Called for each piece of output a terminal prints, on the core's thread.
pub type OnData = Box<dyn FnMut(Vec<u8>)>;

/// How commands reach a machine.
pub trait HostBackend {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>>;
    /// Long-running command; calls `on_line` for each stdout line (e.g. `tail -F`).
    fn stream(&self, config: &HostConfig, cmd: &str, on_line: OnLine, keep_stdin: bool) -> StreamHandle;
    /// Interactive command in a pseudo-terminal (terminal views).
    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle>;
    /// Ends any connection kept open for the machine (an SSH master), when it's removed or changed.
    fn close(&self, _config: &HostConfig) -> LocalBoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

type Pick = Rc<dyn Fn(&HostConfig) -> Rc<dyn HostBackend>>;

thread_local! {
    static PICK: RefCell<Option<Pick>> = const { RefCell::new(None) };
}

/// The platform says which backend serves each machine.
pub fn set_host_backends(choose: impl Fn(&HostConfig) -> Rc<dyn HostBackend> + 'static) {
    PICK.with(|p| *p.borrow_mut() = Some(Rc::new(choose)));
}

fn backend(config: &HostConfig) -> HubResult<Rc<dyn HostBackend>> {
    let pick = PICK.with(|p| p.borrow().clone()).ok_or_else(|| HubError::plain("No way to reach machines has been set up"))?;
    Ok(pick(config))
}

/// tmux only writes UTF-8 when the locale says so, and a non-interactive SSH shell often has no
/// locale at all: then its field separator and every non-ASCII character come out as "_", and a
/// machine with sessions running lists none. -u makes it write UTF-8 whatever the locale. Only for
/// the real binary: on Windows `tmux` is already a shell function around psmux.
pub const UTF8_TMUX: &str = "case \"$(command -v tmux 2>/dev/null)\" in /*) tmux() { command tmux -u \"$@\"; };; esac\n";

/// Runs POSIX shell snippets on one machine, whichever way it's reached. Clones share the
/// machine's settings, as every holder of the TS object does.
#[derive(Clone)]
pub struct Host {
    config: Rc<RefCell<HostConfig>>,
}

impl Host {
    pub fn new(config: HostConfig) -> Self {
        Host { config: Rc::new(RefCell::new(config)) }
    }

    pub fn id(&self) -> String {
        self.config.borrow().id.clone()
    }

    /// A copy of the machine's settings as they are now.
    pub fn config(&self) -> HostConfig {
        self.config.borrow().clone()
    }

    /// Changes the settings in place (`Object.assign(host.config, patch)`).
    pub fn update_config(&self, f: impl FnOnce(&mut HostConfig)) {
        f(&mut self.config.borrow_mut())
    }

    /// The same machine object.
    pub fn same(&self, other: &Host) -> bool {
        Rc::ptr_eq(&self.config, &other.config)
    }

    pub fn exec(&self, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        let config = self.config();
        match backend(&config) {
            Ok(b) => b.exec(&config, &format!("{UTF8_TMUX}{cmd}"), opts),
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }

    /// `exec` with the default options.
    pub fn run(&self, cmd: &str) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        self.exec(cmd, ExecOptions::default())
    }

    pub fn stream(&self, cmd: &str, on_line: impl FnMut(String) + 'static, keep_stdin: bool) -> HubResult<StreamHandle> {
        let config = self.config();
        Ok(backend(&config)?.stream(&config, &format!("{UTF8_TMUX}{cmd}"), Box::new(on_line), keep_stdin))
    }

    pub fn pty(&self, cmd: &str, cols: u16, rows: u16, on_data: impl FnMut(Vec<u8>) + 'static) -> HubResult<PtyHandle> {
        let config = self.config();
        backend(&config)?.pty(&config, cmd, cols, rows, Box::new(on_data))
    }

    pub fn close(&self) -> LocalBoxFuture<'static, HubResult<()>> {
        let config = self.config();
        match backend(&config) {
            Ok(b) => {
                let f = b.close(&config);
                Box::pin(async move {
                    f.await;
                    Ok(())
                })
            }
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }
}
