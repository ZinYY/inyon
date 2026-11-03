//! A control-mode connection over any byte stream, with one reader thread.
//!
//! Commands are written as lines and their replies matched in order (tmux answers a client's
//! commands in the order it sent them). Notifications go to an unbounded channel, so the reader
//! never waits on the consumer and tmux never sees a slow client because of us.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::proto::{Event, Notification, Parser, Reply};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// tmux answered with `%error`; the text is its message
    Command(String),
    /// the connection ended (after `%exit`, or EOF without one)
    Closed(Option<String>),
    Timeout,
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Command(m) => write!(f, "tmux: {m}"),
            Error::Closed(Some(r)) => write!(f, "control client exited: {r}"),
            Error::Closed(None) => write!(f, "control client exited"),
            Error::Timeout => write!(f, "no reply from tmux"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

type Waiter = Sender<Result<Reply, Error>>;

#[derive(Default)]
struct State {
    pending: VecDeque<Waiter>,
    closed: bool,
    exit: Option<String>,
    /// the command tmux was started with has answered: the client is attached
    ready: bool,
}

/// Counters for the load tests.
#[derive(Default, Debug)]
pub struct Stats {
    pub lines: AtomicU64,
    pub bytes: AtomicU64,
    pub stray: AtomicU64,
    pub unsolicited: AtomicU64,
}

pub struct Control {
    state: Arc<Mutex<State>>,
    ready: Arc<Condvar>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    reader: Option<JoinHandle<()>>,
    child: Mutex<Option<Child>>,
    pub stats: Arc<Stats>,
    pub timeout: Duration,
}

impl Control {
    /// Runs control mode over an existing stream (an ssh channel, a node-link stream). Returns the
    /// connection and the notification channel; the channel disconnects when the stream ends.
    pub fn over(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> (Self, Receiver<Notification>) {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(State::default()));
        let ready = Arc::new(Condvar::new());
        let stats = Arc::new(Stats::default());
        let handle = {
            let (state, ready, stats) = (state.clone(), ready.clone(), stats.clone());
            std::thread::spawn(move || read_loop(BufReader::with_capacity(1 << 16, reader), state, ready, stats, tx))
        };
        let c = Control { state, ready, writer: Mutex::new(Some(Box::new(writer))), reader: Some(handle), child: Mutex::new(None), stats, timeout: Duration::from_secs(10) };
        (c, rx)
    }

    /// Starts a local `tmux -C ...` (the command is built by the caller) and runs over its pipes.
    pub fn spawn(mut cmd: Command) -> io::Result<(Self, Receiver<Notification>)> {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let (c, rx) = Self::over(stdout, stdin);
        *c.child.lock().unwrap() = Some(child);
        Ok((c, rx))
    }

    /// Sends commands without waiting; each returned receiver gets that command's reply.
    pub fn send(&self, lines: &[String]) -> Vec<Receiver<Result<Reply, Error>>> {
        let mut out = Vec::with_capacity(lines.len());
        let mut writer = self.writer.lock().unwrap();
        // the queue and the write stay under one lock so replies line up with what was written
        let mut st = self.state.lock().unwrap();
        let mut buf = Vec::new();
        for l in lines {
            debug_assert!(!l.contains('\n'), "a command is one line");
            let (tx, rx) = mpsc::channel();
            if st.closed || writer.is_none() {
                let _ = tx.send(Err(Error::Closed(st.exit.clone())));
            } else {
                st.pending.push_back(tx);
                buf.extend_from_slice(l.as_bytes());
                buf.push(b'\n');
            }
            out.push(rx);
        }
        if let Some(w) = writer.as_mut() {
            if !buf.is_empty() {
                if let Err(e) = w.write_all(&buf).and_then(|_| w.flush()) {
                    for tx in st.pending.drain(..) {
                        let _ = tx.send(Err(Error::Io(e.to_string())));
                    }
                }
            }
        }
        out
    }

    /// Runs one command and waits for its reply. `%error` comes back as [`Error::Command`].
    pub fn cmd(&self, line: impl Into<String>) -> Result<Reply, Error> {
        let rx = self.send(&[line.into()]).pop().unwrap();
        self.wait(rx)
    }

    /// Runs several commands in one write and waits for all of them.
    pub fn cmds(&self, lines: &[String]) -> Vec<Result<Reply, Error>> {
        self.send(lines).into_iter().map(|rx| self.wait(rx)).collect()
    }

    fn wait(&self, rx: Receiver<Result<Reply, Error>>) -> Result<Reply, Error> {
        match rx.recv_timeout(self.timeout) {
            Ok(Ok(r)) if r.ok => Ok(r),
            Ok(Ok(r)) => Err(Error::Command(r.text())),
            Ok(Err(e)) => Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Closed(self.exit_reason())),
        }
    }

    /// Waits until tmux has answered the command it was started with (`attach-session`). A
    /// command sent before that can run before the client is attached: tmux 3.3 then answers
    /// "no current client", and a size read then is the size from before the attach.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let st = self.state.lock().unwrap();
        let (st, _) = self.ready.wait_timeout_while(st, timeout, |st| !st.ready && !st.closed).unwrap();
        st.ready
    }

    pub fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    pub fn exit_reason(&self) -> Option<String> {
        self.state.lock().unwrap().exit.clone()
    }

    /// Detaches the way tmux expects: closing stdin ends control mode.
    pub fn detach(&self) {
        self.writer.lock().unwrap().take();
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.detach();
        let Some(mut child) = self.child.lock().unwrap().take() else {
            // a stream we didn't start ends when its owner closes it; don't wait for that here
            return;
        };
        {
            for _ in 0..50 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

fn read_loop(mut r: impl BufRead, state: Arc<Mutex<State>>, ready: Arc<Condvar>, stats: Arc<Stats>, tx: Sender<Notification>) {
    let mut parser = Parser::new();
    let mut line = Vec::with_capacity(4096);
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                stats.lines.fetch_add(1, Ordering::Relaxed);
                stats.bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        match parser.line(&line) {
            None => {}
            Some(Event::Reply(reply)) => {
                // flags 0: the command tmux was started with (attach-session), not one of ours
                let waiter = if reply.flags == 0 {
                    state.lock().unwrap().ready = true;
                    ready.notify_all();
                    None
                } else {
                    state.lock().unwrap().pending.pop_front()
                };
                match waiter {
                    Some(w) => {
                        let _ = w.send(Ok(reply));
                    }
                    None => {
                        stats.unsolicited.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Some(Event::Notification(n)) => {
                if let Notification::Exit { reason } = &n {
                    state.lock().unwrap().exit = reason.clone();
                }
                let _ = tx.send(n);
            }
            Some(Event::Stray(_)) => {
                stats.stray.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let mut st = state.lock().unwrap();
    st.closed = true;
    ready.notify_all();
    let reason = st.exit.clone();
    for w in st.pending.drain(..) {
        let _ = w.send(Err(Error::Closed(reason.clone())));
    }
}
