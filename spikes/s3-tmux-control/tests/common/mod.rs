//! A private tmux server per test (`tmux -L inyon-s3-<test>`), killed when the test ends. The
//! binary comes from TMUX_BIN (CI points it at each built version), else `tmux` on PATH.
#![allow(dead_code)]

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use s3_tmux_control::tmux::{Tmux, Version};
use s3_tmux_control::{Control, Notification};

pub struct Server {
    pub tmux: Tmux,
    pub dir: PathBuf,
}

impl Server {
    /// Starts a server with one session, `main`, at 120x40, running /bin/sh with no config.
    pub fn start(name: &str) -> Server {
        let mut tmux = Tmux::new(std::env::var_os("TMUX_BIN").map(PathBuf::from).unwrap_or_else(|| "tmux".into()));
        tmux.socket = Some(format!("inyon-s3-{name}"));
        tmux.config = Some("/dev/null".into());
        let dir = std::env::temp_dir().join(format!("inyon-s3-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = Server { tmux, dir };
        // a server left over from a crashed run of the same test
        let _ = s.tmux.run(["kill-server"]);
        // kill-server returns before the old server is gone, and a new-session that reaches the
        // dying one fails with "server exited unexpectedly": try again
        let mut out = None;
        for _ in 0..20 {
            // tmux 3.2 resizes a detached session to default-size (80x24) whatever new-session's
            // -x/-y said, so set that first; -q keeps 3.0 (no such option) quiet
            let o = s
                .tmux
                .command()
                .env("SHELL", "/bin/sh")
                .args(["start-server", ";", "set-option", "-gq", "default-size", "120x40", ";", "new-session", "-d", "-s", "main", "-x", "120", "-y", "40"])
                .output()
                .unwrap();
            let ok = o.status.success();
            out = Some(o);
            if ok {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let out = out.unwrap();
        assert!(out.status.success(), "new-session: {}", String::from_utf8_lossy(&out.stderr));
        s.fit("main:");
        s
    }

    /// tmux 3.2 sizes a window made without a terminal from the client that made it (80x24
    /// for a one-off command or a control client), whatever new-session's -x/-y said. This sets
    /// 120x40 the way the hub's psmux mirror does: resize, then back to automatic sizing.
    pub fn fit(&self, target: &str) {
        for args in [&["resize-window", "-t", target, "-x", "120", "-y", "40"][..], &["set-option", "-w", "-t", target, "-u", "window-size"]] {
            let o = self.tmux.run(args).unwrap();
            assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        }
    }

    pub fn version(&self) -> Version {
        self.tmux.version().expect("tmux -V")
    }

    pub fn attach(&self, read_only: bool) -> (Control, Receiver<Notification>) {
        self.tmux.attach("main", read_only).unwrap()
    }

    /// Writes a shell script into the test folder and returns its path.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    /// Opens a window in `main` running a script; returns its pane id. Made with a one-off tmux
    /// command, as the hub does, and sized with `fit`.
    pub fn window(&self, name: &str, body: &str) -> String {
        let script = self.script(&format!("{name}.sh"), body);
        let out = self.tmux.run(["new-window", "-d", "-t", "main:", "-n", name, "-P", "-F", "#{pane_id}", &format!("sh {}", script.display())]).unwrap();
        assert!(out.status.success(), "new-window: {}", String::from_utf8_lossy(&out.stderr));
        let pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
        self.fit(&pane);
        pane
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // this test's server only (-L inyon-s3-<test>), never the real one
        let _ = self.tmux.run(["kill-server"]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Collects a pane's output until `done` says so or the time runs out.
pub fn collect(rx: &Receiver<Notification>, pane: &str, timeout: Duration, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let end = Instant::now() + timeout;
    let mut buf = Vec::new();
    while !done(&buf) {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Notification::Output { pane: p, data } | Notification::ExtendedOutput { pane: p, data, .. }) if p == pane => buf.extend(data),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("timed out; got {} bytes: {:?}", buf.len(), String::from_utf8_lossy(&buf[buf.len().saturating_sub(300)..])),
            Err(RecvTimeoutError::Disconnected) => panic!("control client exited"),
        }
    }
    buf
}

/// Waits for the first notification `pick` accepts.
pub fn wait_for<T>(rx: &Receiver<Notification>, timeout: Duration, mut pick: impl FnMut(&Notification) -> Option<T>) -> T {
    let end = Instant::now() + timeout;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(n) => {
                if std::env::var_os("S3_DEBUG").is_some() && !matches!(n, Notification::Output { .. } | Notification::ExtendedOutput { .. }) {
                    eprintln!("notification: {n:?}");
                }
                if let Some(t) = pick(&n) {
                    return t;
                }
            }
            Err(e) => panic!("no matching notification: {e:?}"),
        }
    }
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Waits until a file satisfies `ok`, returning its bytes.
pub fn wait_file(path: &std::path::Path, timeout: Duration, ok: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let end = Instant::now() + timeout;
    loop {
        let b = std::fs::read(path).unwrap_or_default();
        if ok(&b) {
            return b;
        }
        if Instant::now() > end {
            panic!("{} never matched; has {:?}", path.display(), String::from_utf8_lossy(&b));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A reader that stops reading once for `stall` after `after` bytes: a client that falls behind.
pub struct Stall<R> {
    pub inner: R,
    pub after: usize,
    pub stall: Duration,
    pub seen: usize,
    pub done: bool,
}

impl<R: Read> Read for Stall<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.done && self.seen >= self.after {
            self.done = true;
            std::thread::sleep(self.stall);
        }
        let n = self.inner.read(buf)?;
        self.seen += n;
        Ok(n)
    }
}
