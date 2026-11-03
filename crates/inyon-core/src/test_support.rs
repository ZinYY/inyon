//! What the ported tests share: the captures in fixtures/ and the `screen()` splitter.

use std::path::Path;

/// A file under the repo's fixtures/ folder, as text.
pub fn fixture(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `s.split('\n')`
pub fn screen(s: &str) -> Vec<String> {
    s.split('\n').map(str::to_owned).collect()
}

/// screenFixture(agent, name): fixtures/screens/<agent>/<name>.txt as lines
pub fn screen_fixture(agent: &str, name: &str) -> Vec<String> {
    screen(&fixture(&format!("screens/{agent}/{name}.txt")))
}

/// `expect(got).toMatchObject(want)`: every field `want` names has the same value in `got`
/// (objects compared by the fields given, arrays element for element). A `null` in `want` needs a
/// `null` in `got`, as toMatchObject's does: a missing field is undefined, which isn't null.
#[track_caller]
pub fn assert_match(got: &impl serde::Serialize, want: serde_json::Value) {
    let got = serde_json::to_value(got).unwrap();
    if let Some(path) = mismatch("", &got, &want) {
        panic!("{path}\n got: {got}\nwant: {want}");
    }
}

fn mismatch(path: &str, got: &serde_json::Value, want: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    match (got, want) {
        (Value::Object(g), Value::Object(w)) => w.iter().find_map(|(k, wv)| match g.get(k) {
            Some(gv) => mismatch(&format!("{path}.{k}"), gv, wv),
            // null is a value: it doesn't match a field that isn't there
            None => Some(format!("{path}.{k} missing")),
        }),
        (Value::Array(g), Value::Array(w)) if g.len() == w.len() => g.iter().zip(w).enumerate().find_map(|(i, (a, b))| mismatch(&format!("{path}[{i}]"), a, b)),
        _ if got == want => None,
        _ => Some(format!("{path}: {got} ≠ {want}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[should_panic(expected = ".background missing")]
    fn a_null_wanted_does_not_match_a_field_left_out() {
        assert_match(&json!({ "remote": true }), json!({ "background": null }));
    }

    #[test]
    fn a_null_wanted_matches_a_null() {
        assert_match(&json!({ "background": null, "remote": true }), json!({ "background": null }));
    }
}

/// The core's event loop for a test: tokio on this thread with time paused, so a timer fires as
/// soon as nothing else is left to run and `Date.now()` moves with it (the fake clock).
pub mod rt {
    use std::future::Future;
    use std::rc::Rc;

    use futures_util::future::LocalBoxFuture;

    use crate::platform::rt::{install, Runtime};

    /// The clock starts here, a moment in October 2026.
    pub const T0: u64 = 1_791_000_000_000;

    struct TestRuntime {
        start: tokio::time::Instant,
    }

    impl Runtime for TestRuntime {
        fn spawn(&self, fut: LocalBoxFuture<'static, ()>) {
            tokio::task::spawn_local(fut);
        }
        fn sleep(&self, ms: u64) -> LocalBoxFuture<'static, ()> {
            Box::pin(tokio::time::sleep(std::time::Duration::from_millis(ms)))
        }
        fn now_ms(&self) -> u64 {
            T0 + self.start.elapsed().as_millis() as u64
        }
    }

    /// Runs `test` on a fresh event loop with paused time.
    pub fn run<F: Future<Output = ()>>(test: impl FnOnce() -> F) {
        let tokio = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&tokio, async move {
            install(Rc::new(TestRuntime { start: tokio::time::Instant::now() }));
            test().await;
        });
    }

    /// `await sleep(ms)` in a test: lets everything due by then run.
    pub async fn wait(ms: u64) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// A machine that answers from a script: each command is noted (without the UTF8_TMUX prelude
/// Host puts in front) and answered by `answer`.
pub mod fake {
    use std::cell::RefCell;
    use std::rc::Rc;

    use futures_util::future::LocalBoxFuture;
    use inyon_protocol::HostConfig;

    use crate::hosts::host::{set_host_backends, ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle, UTF8_TMUX};
    use crate::hosts::http_error::HubResult;
    use crate::platform::rt::Deferred;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Call {
        pub host: String,
        pub cmd: String,
        pub input: Option<String>,
        pub timeout_ms: Option<u64>,
    }

    pub type Answer = Rc<dyn Fn(&str, &str, Option<&str>) -> ExecResult>;

    /// A stream someone opened: push lines into it, end it.
    #[derive(Clone)]
    pub struct OpenStream {
        pub host: String,
        pub cmd: String,
        pub on_line: Rc<RefCell<OnLine>>,
        pub exited: Deferred<i32>,
        pub killed: Rc<std::cell::Cell<bool>>,
    }

    impl OpenStream {
        pub fn line(&self, l: &str) {
            (self.on_line.borrow_mut())(l.to_owned());
        }
    }

    #[derive(Default)]
    pub struct Fake {
        pub calls: RefCell<Vec<Call>>,
        pub streams: RefCell<Vec<OpenStream>>,
        pub answer: RefCell<Option<Answer>>,
        /// how many times a machine's connection was closed
        pub closed: std::cell::Cell<u32>,
    }

    pub fn ok(stdout: &str) -> ExecResult {
        ExecResult { code: 0, stdout: stdout.into(), stderr: String::new() }
    }

    impl Fake {
        /// Makes it the backend of every machine on this thread.
        pub fn install(answer: impl Fn(&str, &str, Option<&str>) -> ExecResult + 'static) -> Rc<Fake> {
            let f = Rc::new(Fake { answer: RefCell::new(Some(Rc::new(answer))), ..Default::default() });
            let b = f.clone();
            set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
            f
        }

        pub fn cmds(&self) -> Vec<String> {
            self.calls.borrow().iter().map(|c| c.cmd.clone()).collect()
        }
    }

    /// Text typed through a buffer (`type_command`, fed on stdin) shown as the `send-keys -l` it
    /// stands for, so a fake reads typing one way however the hub delivers it.
    pub fn as_keys(cmd: String, input: Option<String>) -> (String, Option<String>) {
        let re = regex::Regex::new(r#"^(.*?)umask 077; f=\$\(mktemp\) .*? tmux paste-buffer -d -b '[^']*' -t ('[^']*'); r=\$\?; .*?\[ \$r = 0 \]; \}(.*)$"#).unwrap();
        match (re.captures(&cmd), &input) {
            (Some(c), Some(text)) => (format!("{}tmux send-keys -t {} -l -- {}{}", &c[1], &c[2], crate::hosts::host::shq(text), &c[3]), None),
            _ => (cmd, input),
        }
    }

    impl HostBackend for Fake {
        fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
            let cmd = cmd.strip_prefix(UTF8_TMUX).unwrap_or(cmd).to_owned();
            let input = opts.input.map(|i| String::from_utf8_lossy(&i).into_owned());
            let (cmd, input) = as_keys(cmd, input);
            self.calls.borrow_mut().push(Call { host: config.id.clone(), cmd: cmd.clone(), input: input.clone(), timeout_ms: opts.timeout_ms });
            let answer = self.answer.borrow().clone();
            let r = answer.map_or_else(|| ok(""), |a| a(&config.id, &cmd, input.as_deref()));
            // never ready on the first poll: a real command (and the TS's awaited promise) answers
            // on a later turn of the loop, after whatever else is ready has run
            Box::pin(async move {
                crate::platform::rt::yield_now().await;
                Ok(r)
            })
        }
        fn stream(&self, config: &HostConfig, cmd: &str, on_line: OnLine, _keep_stdin: bool) -> StreamHandle {
            let cmd = cmd.strip_prefix(UTF8_TMUX).unwrap_or(cmd).to_owned();
            let s = OpenStream { host: config.id.clone(), cmd, on_line: Rc::new(RefCell::new(on_line)), exited: Deferred::new(), killed: Rc::default() };
            self.streams.borrow_mut().push(s.clone());
            let (killed, exited) = (s.killed.clone(), s.exited.clone());
            StreamHandle::new(
                move || {
                    killed.set(true);
                    exited.resolve(143);
                },
                s.exited,
            )
        }
        fn pty(&self, _config: &HostConfig, _cmd: &str, _cols: u16, _rows: u16, _on_data: OnData) -> HubResult<PtyHandle> {
            Ok(PtyHandle::new(|_| {}, |_, _| {}, || {}, Deferred::new()))
        }
        fn close(&self, _config: &HostConfig) -> LocalBoxFuture<'static, ()> {
            self.closed.set(self.closed.get() + 1);
            Box::pin(async {})
        }
    }
}

/// A temporary folder, removed when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("{prefix}{}", crate::ids::random_uuid()));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    pub fn path(&self, rel: &str) -> std::path::PathBuf {
        self.0.join(rel)
    }

    pub fn s(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    pub fn write(&self, rel: &str, text: &str) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    pub fn mode(&self, rel: &str) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(self.path(rel)).unwrap().permissions().mode() & 0o777
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `sh -c script` with only `env` (and PATH) set, `input` on stdin: (code, stdout, stderr).
pub fn sh(script: &str, input: Option<&[u8]>, env: &[(&str, &str)]) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut c = Command::new("sh");
    c.arg("-c").arg(script).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    if let Some(i) = input {
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(i);
    }
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Every machine's shell runs here, seeing only a temporary HOME, so what a test installs can
/// never reach the real one.
pub mod sandbox {
    use std::rc::Rc;

    use futures_util::future::LocalBoxFuture;
    use inyon_protocol::HostConfig;

    use crate::hosts::host::{set_host_backends, ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
    use crate::hosts::http_error::HubResult;
    use crate::platform::rt::Deferred;

    pub struct Sandbox {
        pub home: String,
        /// put in front of every command
        pub prefix: String,
    }

    impl Sandbox {
        pub fn install(home: &str) {
            Self::install_with(home, "");
        }

        pub fn install_with(home: &str, prefix: &str) {
            let b = Rc::new(Sandbox { home: home.to_owned(), prefix: prefix.to_owned() });
            set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
        }
    }

    impl HostBackend for Sandbox {
        fn exec(&self, _config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
            let (code, stdout, stderr) = super::sh(&format!("{}{cmd}", self.prefix), opts.input.as_deref(), &[("HOME", &self.home)]);
            Box::pin(async move { Ok(ExecResult { code, stdout, stderr }) })
        }
        fn stream(&self, _config: &HostConfig, _cmd: &str, _on_line: OnLine, _keep_stdin: bool) -> StreamHandle {
            StreamHandle::new(|| {}, Deferred::resolved(0))
        }
        fn pty(&self, _config: &HostConfig, _cmd: &str, _cols: u16, _rows: u16, _on_data: OnData) -> HubResult<PtyHandle> {
            Ok(PtyHandle::new(|_| {}, |_, _| {}, || {}, Deferred::resolved(0)))
        }
    }
}

/// A machine of this kind (`{ id, name: id, kind }`).
pub fn host(id: &str, kind: &str) -> crate::hosts::host::Host {
    crate::hosts::host::Host::new(serde_json::from_value(serde_json::json!({ "id": id, "name": id, "kind": kind })).unwrap())
}

/// A machine in this thread's registry, online, with one tmux window holding `panes` (partial
/// pane objects; what they leave out is filled in). Nothing polls it.
pub fn machine(id: &str, kind: &str, panes: serde_json::Value) -> crate::hosts::registry::Rt {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::hosts::registry::{registry, HostRuntime};
    let panes: Vec<serde_json::Value> = panes
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let mut full = serde_json::json!({ "index": 0, "active": true, "command": "claude", "cwd": "/w", "title": "claude", "width": 100, "height": 30, "dead": false });
            for (k, v) in p.as_object().unwrap() {
                full[k] = v.clone();
            }
            full
        })
        .collect();
    let state: inyon_protocol::HostState = serde_json::from_value(serde_json::json!({
        "config": { "id": id, "name": "Box", "kind": kind }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0,
        "sessions": [{ "id": "$1", "name": "work", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": panes }] }],
    }))
    .unwrap();
    let host = crate::hosts::host::Host::new(state.config.clone());
    let rt = Rc::new(RefCell::new(HostRuntime::new(host, state)));
    registry().insert_runtime(id, rt.clone());
    rt
}

/// A machine in the registry whose last probe found `home`; nothing polls it.
pub fn connected(id: &str, home: &str, panes: serde_json::Value) -> crate::hosts::registry::Rt {
    let rt = machine(id, "local", panes);
    rt.borrow_mut().hold_polls();
    let probe = crate::tmux::tmux::ProbeResult::at_home(home);
    rt.borrow_mut().probe = Some(std::rc::Rc::new(std::cell::RefCell::new(probe)));
    rt
}
