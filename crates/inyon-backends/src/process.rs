//! Port of hosts/process-backend.ts: the hub's machines reached as processes. The local machine
//! runs commands directly; remote machines go through the system `ssh` binary so ~/.ssh/config,
//! agents, jump hosts and known_hosts all just work. A ControlMaster socket keeps one connection
//! per machine, so each command is cheap.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use futures_util::future::LocalBoxFuture;
use inyon_core::hosts::host::{shq, ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use inyon_core::hosts::http_error::{HubError, HubResult};
use inyon_core::platform::rt::Deferred;
use inyon_protocol::{HostConfig, HostKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// How long a command may take when the caller doesn't say (`timeoutMs ?? 20000`).
const DEFAULT_TIMEOUT_MS: u64 = 20_000;

/// The hub's machines reached as processes: `control_dir` holds the ssh ControlMaster sockets.
/// `env` is added to every command's environment (the INYON_* names of TETHER_* variables, which
/// the TS sets in process.env for its children).
pub struct ProcessBackend {
    control_dir: PathBuf,
    env: Vec<(String, String)>,
}

/// What a test's commands never get (test-setup.ts): with $TMUX set, tmux ignores TMUX_TMPDIR and
/// talks to the server it runs inside, and a test once stopped the user's real one.
#[cfg(feature = "testing")]
pub(crate) const DROPPED: &[&str] = &["TMUX", "TMUX_PANE"];
#[cfg(not(feature = "testing"))]
pub(crate) const DROPPED: &[&str] = &[];

impl ProcessBackend {
    pub fn new(control_dir: impl Into<PathBuf>, env: Vec<(String, String)>) -> Rc<ProcessBackend> {
        #[cfg(feature = "testing")]
        let env = test_env(env);
        Rc::new(ProcessBackend { control_dir: control_dir.into(), env })
    }

    fn ssh_args(&self, c: &HostConfig, tty: bool) -> Vec<String> {
        let mut args: Vec<String> = [
            "-o",
            "BatchMode=yes",
            "-o",
            "ControlMaster=auto",
            "-o",
            &format!("ControlPath={}", self.control_dir.join("%C").display()),
            "-o",
            "ControlPersist=600",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
        ]
        .map(str::to_owned)
        .to_vec();
        if let Some(port) = c.port.filter(|p| *p != 0) {
            args.extend(["-p".into(), port.to_string()]);
        }
        if let Some(id) = c.identity_file.as_deref().filter(|i| !i.is_empty()) {
            args.extend(["-i".into(), id.to_owned()]);
        }
        args.push(if tty { "-tt" } else { "-T" }.into());
        args.push(c.target.clone().unwrap_or_else(|| "undefined".into()));
        args
    }

    /// argv that runs `cmd` (a POSIX shell snippet) on this host.
    pub fn argv(&self, c: &HostConfig, cmd: &str, tty: bool) -> Vec<String> {
        if c.kind == HostKind::Local {
            return vec!["/bin/sh".into(), "-c".into(), cmd.into()];
        }
        let mut v = vec!["ssh".to_owned()];
        v.extend(self.ssh_args(c, tty));
        v.push(format!("sh -c {}", shq(cmd)));
        v
    }

    fn command(&self, argv: &[String]) -> Command {
        let mut c = Command::new(&argv[0]);
        for k in DROPPED {
            c.env_remove(k);
        }
        c.args(&argv[1..]).envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).kill_on_drop(false);
        own_group(&mut c);
        hide_window(&mut c);
        c
    }
}

/// A test's commands get a tmux folder of their own (TMUX_TMPDIR, made once per test run) unless
/// the test gives one.
#[cfg(feature = "testing")]
fn test_env(env: Vec<(String, String)>) -> Vec<(String, String)> {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let mut env: Vec<(String, String)> = env.into_iter().filter(|(k, _)| !DROPPED.contains(&k.as_str())).collect();
    if !env.iter().any(|(k, _)| k == "TMUX_TMPDIR") {
        let dir = DIR.get_or_init(|| {
            let d = std::env::temp_dir().join(format!("inyon-test-tmux-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&d);
            d.to_string_lossy().into_owned()
        });
        env.push(("TMUX_TMPDIR".into(), dir.clone()));
    }
    env
}

/// Its own session and process group, so a timeout or a kill reaches the whole pipeline: killing
/// only /bin/sh leaves children holding the output open (Bun's `detached: true`).
fn own_group(c: &mut Command) {
    #[cfg(unix)]
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            crate::limits::restore_in_child();
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = c;
}

/// No console window for it on Windows (ssh.exe is a console program, and the engine that starts
/// it has no console of its own to lend it).
fn hide_window(c: &mut Command) {
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    #[cfg(not(windows))]
    let _ = c;
}

/// Signals a whole process group (whose leader was `pid`), else the process alone.
fn kill_group(pid: Option<u32>, sig: i32) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: plain signals to a group this backend started
        if unsafe { libc::kill(-(pid as i32), sig) } != 0 {
            unsafe { libc::kill(pid as i32, sig) };
        }
    }
    #[cfg(not(unix))]
    let _ = (pid, sig);
}

/// What a finished process's status reads as: its exit code, or 128 + the signal that ended it.
fn code_of(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    status.code().unwrap_or(1)
}

async fn read_all(r: Option<impl AsyncReadExt + Unpin>) -> Vec<u8> {
    let mut out = vec![];
    if let Some(mut r) = r {
        let _ = r.read_to_end(&mut out).await;
    }
    out
}

impl HostBackend for ProcessBackend {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        let mut c = self.command(&self.argv(config, cmd, false));
        c.stdin(if opts.input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let spawned = c.spawn();
        let timeout = opts.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        Box::pin(async move {
            let mut child = spawned.map_err(|e| HubError::plain(e.to_string()))?;
            // the group's id, kept: once the process has been waited for, `child.id()` is gone
            let group = child.id();
            if let (Some(mut stdin), Some(input)) = (child.stdin.take(), opts.input) {
                tokio::task::spawn_local(async move {
                    let _ = stdin.write_all(&input).await;
                });
            }
            let mut stdout = tokio::task::spawn_local(read_all(child.stdout.take()));
            let mut stderr = tokio::task::spawn_local(read_all(child.stderr.take()));
            // the timer covers the exit and both pipes, as the TS's Promise.all does: something the
            // command left running in the background can hold the output open after it exits
            let finished = tokio::time::timeout(Duration::from_millis(timeout), async {
                let status = child.wait().await;
                (status, (&mut stdout).await.unwrap_or_default(), (&mut stderr).await.unwrap_or_default())
            })
            .await;
            let timed_out = finished.is_err();
            let (status, stdout, stderr) = match finished {
                Ok(r) => r,
                Err(_) => {
                    kill_group(group, 9);
                    #[cfg(not(unix))]
                    let _ = child.start_kill();
                    let status = child.wait().await;
                    (status, stdout.await.unwrap_or_default(), stderr.await.unwrap_or_default())
                }
            };
            let code = status.map(code_of).unwrap_or(1);
            let stdout = String::from_utf8_lossy(&stdout).into_owned();
            let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
            if timed_out {
                stderr.push_str("timed out");
            }
            Ok(ExecResult { code: if timed_out { 124 } else { code }, stdout, stderr })
        })
    }

    fn stream(&self, config: &HostConfig, cmd: &str, mut on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        let exited = Deferred::new();
        // control-mode tmux exits when stdin closes, so it gets a pipe we never write to
        let mut c = self.command(&self.argv(config, cmd, false));
        c.stdin(if keep_stdin { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = match c.spawn() {
            Ok(ch) => ch,
            Err(_) => {
                exited.resolve(1);
                return StreamHandle::new(|| {}, exited);
            }
        };
        let stdout = child.stdout.take();
        let stdin = child.stdin.take();
        let pid = child.id();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let done = exited.clone();
        // `exited` is the process's own end, whether or not something it started still holds the output
        tokio::task::spawn_local(async move {
            let _stdin = stdin;
            let status = tokio::select! {
                s = child.wait() => s,
                _ = stopped => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            done.resolve(status.map(code_of).unwrap_or(1));
        });
        // split on newlines without copying the buffer per line (transcripts can hold MB-sized lines)
        if let Some(mut out) = stdout {
            tokio::task::spawn_local(async move {
                let mut carry: Vec<u8> = vec![];
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match out.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    carry.extend_from_slice(&buf[..n]);
                    let mut start = 0;
                    while let Some(i) = carry[start..].iter().position(|b| *b == b'\n') {
                        if i > 0 {
                            on_line(String::from_utf8_lossy(&carry[start..start + i]).into_owned());
                        }
                        start += i + 1;
                    }
                    carry.drain(..start);
                }
                if !carry.is_empty() {
                    on_line(String::from_utf8_lossy(&carry).into_owned());
                }
            });
        }
        let stop = RefCell::new(Some(stop));
        let kill = move || {
            #[cfg(unix)]
            if let Some(pid) = pid {
                if unsafe { libc::kill(-(pid as i32), libc::SIGTERM) } == 0 {
                    return;
                }
            }
            let _ = pid;
            if let Some(s) = stop.borrow_mut().take() {
                let _ = s.send(());
            }
        };
        StreamHandle::new(kill, exited)
    }

    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle> {
        crate::pty::open(&self.argv(config, cmd, true), &self.env, cols, rows, on_data)
    }

    // the master would otherwise stay up for ControlPersist's ten minutes after the machine is gone
    fn close(&self, config: &HostConfig) -> LocalBoxFuture<'static, ()> {
        if config.kind != HostKind::Ssh {
            return Box::pin(async {});
        }
        let mut args = self.ssh_args(config, false);
        args.truncate(args.len() - 2);
        args.extend(["-O".into(), "exit".into(), config.target.clone().unwrap_or_else(|| "undefined".into())]);
        let mut c = Command::new("ssh");
        for k in DROPPED {
            c.env_remove(k);
        }
        c.args(&args).envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        hide_window(&mut c);
        let spawned = c.spawn();
        Box::pin(async move {
            if let Ok(mut child) = spawned {
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            }
        })
    }
}
