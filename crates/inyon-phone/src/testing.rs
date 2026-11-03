//! What the native tests share: tokio as the core's runtime, and a stand-in for the SSH plugin.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::LocalBoxFuture;
use inyon_core::platform::rt::{install, set_timeout, Deferred, Runtime};
use inyon_link::{Accepting, Channel, Identity, LinkError};
use serde_json::{json, Value};

use crate::link::{Dialer, OnWire, Reach, Wire, WireEvent};
use crate::ssh::{OnEvent, PluginError, PluginResult, SshPlugin};

pub struct TokioRuntime;

impl Runtime for TokioRuntime {
    fn spawn(&self, fut: LocalBoxFuture<'static, ()>) {
        tokio::task::spawn_local(fut);
    }
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(Duration::from_millis(ms)))
    }
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
}

/// Runs `test` on a fresh core thread, with tokio's clock paused (sleeps pass at once).
pub fn on_core<F: std::future::Future<Output = ()>>(test: impl FnOnce() -> F) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async move {
        install(Rc::new(TokioRuntime));
        test().await;
    });
}

pub const FINGERPRINT: &str = "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";

/// The native plugin, played back: calls are recorded, secrets kept in memory, and every server
/// refuses commands unless a test says otherwise.
#[derive(Default)]
pub struct FakeSsh {
    pub calls: RefCell<Vec<(String, Value)>>,
    pub secrets: RefCell<HashMap<String, String>>,
    listeners: RefCell<Vec<OnEvent>>,
    /// methods that fail, and how
    pub fail: RefCell<HashMap<&'static str, PluginError>>,
    pub exec_answer: RefCell<Option<Value>>,
    /// while set, connecting waits for it: the plugin still dialing
    pub dialing: RefCell<Option<Deferred<()>>>,
    /// any other call waits for the one held for its method
    pub held: RefCell<HashMap<&'static str, Deferred<()>>>,
}

impl FakeSsh {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    /// What the plugin was asked to do with `method`, in order.
    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        self.calls.borrow().iter().filter(|(m, _)| m == method).map(|(_, a)| a.clone()).collect()
    }

    /// The plugin sending an event, as the native side does.
    pub fn emit(&self, event: &str, e: Value) {
        let listeners = self.listeners.borrow().clone();
        for l in listeners {
            l(event, &e);
        }
    }
}

impl SshPlugin for FakeSsh {
    fn call(&self, method: &'static str, args: Value) -> PluginResult {
        self.calls.borrow_mut().push((method.to_owned(), args.clone()));
        let failed = self.fail.borrow().get(method).cloned();
        let got = match failed {
            Some(e) => Err(e),
            None => Ok(match method {
                "probe" => json!({ "fingerprint": FINGERPRINT, "keyType": "ssh-ed25519" }),
                "exec" => self.exec_answer.borrow().clone().unwrap_or_else(|| json!({ "code": 1, "stdout": "", "stderr": "not in this test" })),
                "setSecret" => {
                    self.secrets.borrow_mut().insert(args["key"].as_str().unwrap_or_default().to_owned(), args["value"].as_str().unwrap_or_default().to_owned());
                    Value::Null
                }
                "getSecret" => json!({ "value": self.secrets.borrow().get(args["key"].as_str().unwrap_or_default()) }),
                "deleteSecret" => {
                    self.secrets.borrow_mut().remove(args["key"].as_str().unwrap_or_default());
                    Value::Null
                }
                _ => Value::Null,
            }),
        };
        let dialing = if method == "connect" { self.dialing.borrow().clone() } else { self.held.borrow().get(method).cloned() };
        Box::pin(async move {
            if let Some(d) = dialing {
                d.wait().await;
            }
            got
        })
    }

    fn listen(&self, on: OnEvent) {
        self.listeners.borrow_mut().push(on);
    }
}

// ---------- the network, for the link ----------

/// A Reach that never gets anywhere: every dial fails, and nothing is published.
pub fn offline() -> Reach {
    struct Nowhere;
    impl Dialer for Nowhere {
        fn dial(&self, _: &str, _: OnWire) -> Result<Rc<dyn Wire>, String> {
            Err("no network in this test".into())
        }
    }
    Reach { dialer: Rc::new(Nowhere), resolve: Rc::new(|_| Box::pin(async { None })), plain_ws: true }
}

/// What answers at an address.
#[derive(Clone)]
pub enum Answer {
    Machine(Rc<FakeMachine>),
    /// something that isn't the computer answers with bytes that aren't a handshake
    Garbage,
    /// accepts the socket and never says anything
    Silent,
    /// nothing there: the socket closes
    Down,
}

/// Addresses and what answers at each; anything else is down.
#[derive(Default)]
pub struct FakeNet {
    pub at: RefCell<HashMap<String, Answer>>,
    pub dialed: RefCell<Vec<String>>,
    /// what pkarr has for each id
    pub published: RefCell<HashMap<String, inyon_link::Record>>,
    /// sockets the phone closed, with the code and reason
    pub closed: RefCell<Vec<(String, u16, String)>>,
}

impl FakeNet {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    pub fn put(&self, url: &str, a: Answer) {
        self.at.borrow_mut().insert(url.to_owned(), a);
    }

    pub fn reach(self: &Rc<Self>) -> Reach {
        let me = self.clone();
        Reach {
            dialer: self.clone(),
            resolve: Rc::new(move |id| {
                let got = me.published.borrow().get(&id).cloned();
                Box::pin(async move { got })
            }),
            plain_ws: true,
        }
    }
}

/// Delivers on a later turn, as a socket does.
fn later(on: &OnWire, e: WireEvent) {
    let on = on.clone();
    set_timeout(0, move || on(e));
}

struct FakeWire {
    url: String,
    net: Rc<FakeNet>,
    to_phone: OnWire,
    answer: Answer,
    closed: Cell<bool>,
    /// the machine's end, once the handshake is done
    session: RefCell<Option<Rc<Session>>>,
}

impl Wire for FakeWire {
    fn send(&self, data: &[u8]) {
        if self.closed.get() {
            return;
        }
        match &self.answer {
            Answer::Machine(m) => {
                let session = self.session.borrow().clone();
                match session {
                    Some(s) => m.frame(&s, data),
                    None => {
                        let s = m.handshake(data, self.to_phone.clone());
                        *self.session.borrow_mut() = s;
                    }
                }
            }
            Answer::Garbage => later(&self.to_phone, WireEvent::Message(vec![7; 80])),
            Answer::Silent | Answer::Down => {}
        }
    }
    fn close(&self, code: u16, reason: &str) {
        if self.closed.replace(true) {
            return;
        }
        self.net.closed.borrow_mut().push((self.url.clone(), code, reason.to_owned()));
        if let (Answer::Machine(m), Some(s)) = (&self.answer, self.session.borrow().as_ref()) {
            m.sessions.borrow_mut().retain(|x| !Rc::ptr_eq(x, s));
        }
        later(&self.to_phone, WireEvent::Close(code, reason.to_owned()));
    }
}

impl Dialer for FakeNet {
    fn dial(&self, url: &str, on: OnWire) -> Result<Rc<dyn Wire>, String> {
        self.dialed.borrow_mut().push(url.to_owned());
        let answer = self.at.borrow().get(url).cloned().unwrap_or(Answer::Down);
        if matches!(answer, Answer::Down) {
            later(&on, WireEvent::Close(1006, String::new()));
        }
        let net = NET.with(|n| n.borrow().clone()).unwrap_or_default();
        Ok(Rc::new(FakeWire { url: url.to_owned(), net, to_phone: on, answer, closed: Cell::new(false), session: RefCell::default() }))
    }
}

thread_local! {
    /// The test's network, for the wires to report closes to.
    pub static NET: RefCell<Option<Rc<FakeNet>>> = const { RefCell::new(None) };
}

/// A test network on this thread.
pub fn net() -> Rc<FakeNet> {
    let n = FakeNet::new();
    NET.with(|c| *c.borrow_mut() = Some(n.clone()));
    n
}

/// One phone's open channel, the machine's end.
pub struct Session {
    channel: RefCell<Channel>,
    to_phone: OnWire,
    pub phone_key: [u8; 32],
}

/// A computer running `inyon link`: it lets in phones it paired with, pairs with the right secret,
/// answers pings and runs `exec` as "ran <cmd>", or for real in `sh` when it has a `home`.
pub struct FakeMachine {
    pub id: Identity,
    pub secret: String,
    pub paired: RefCell<Vec<[u8; 32]>>,
    pub sessions: RefCell<Vec<Rc<Session>>>,
    pub heard: RefCell<Vec<Value>>,
    pub hellos: RefCell<Vec<Value>>,
    pub answers_pings: Cell<bool>,
    /// inyon before 0.17, which ignores being told to forget the phone
    pub ignores_unpair: Cell<bool>,
    /// the addresses it lists in its welcome
    pub urls: RefCell<Vec<String>>,
    /// set: `exec` runs in `sh` with this HOME, its input handed on stdin
    pub home: RefCell<Option<String>>,
}

impl FakeMachine {
    pub fn new(seed: u8) -> Rc<Self> {
        Rc::new(FakeMachine {
            id: Identity::from_seed([seed; 32]),
            secret: "pair-secret".into(),
            paired: RefCell::default(),
            sessions: RefCell::default(),
            heard: RefCell::default(),
            hellos: RefCell::default(),
            answers_pings: Cell::new(true),
            ignores_unpair: Cell::new(false),
            urls: RefCell::default(),
            home: RefCell::default(),
        })
    }

    fn handshake(self: &Rc<Self>, msg1: &[u8], to_phone: OnWire) -> Option<Rc<Session>> {
        let a: Accepting = inyon_link::accept(&self.id.noise, msg1).ok()?;
        let hello: Value = serde_json::from_slice(a.hello()).unwrap_or(Value::Null);
        self.hellos.borrow_mut().push(hello.clone());
        let key = *a.remote_static();
        if hello["pair"].as_str() == Some(self.secret.as_str()) && !self.paired.borrow().contains(&key) {
            self.paired.borrow_mut().push(key);
        }
        if !self.paired.borrow().contains(&key) {
            let refusal = a.refuse(&LinkError::NotPaired("This phone isn't paired with this computer any more. Run `inyon pair` there and scan the code again.".into())).ok()?;
            later(&to_phone, WireEvent::Message(refusal));
            return None;
        }
        let welcome = json!({ "ok": true, "hostname": "laptop", "os": "linux", "version": "0.23.0", "urls": *self.urls.borrow(), "relays": [] });
        let (channel, msg2) = a.welcome(welcome.to_string().as_bytes()).ok()?;
        later(&to_phone, WireEvent::Message(msg2));
        let s = Rc::new(Session { channel: RefCell::new(channel), to_phone, phone_key: key });
        self.sessions.borrow_mut().push(s.clone());
        Some(s)
    }

    fn frame(&self, s: &Rc<Session>, data: &[u8]) {
        let Ok(Some(body)) = s.channel.borrow_mut().open(data) else { return };
        let Ok(mut msg) = serde_json::from_slice::<Value>(&body) else { return };
        // what came on stdin is kept as its size: an upload's chunks would be megabytes each
        let input = msg.as_object_mut().and_then(|m| m.remove("input"));
        if let Some(i) = &input {
            msg["inputLength"] = json!(i.as_str().map_or(0, str::len));
        }
        self.heard.borrow_mut().push(msg.clone());
        match msg["op"].as_str() {
            Some("ping") if self.answers_pings.get() => self.send(s, &json!({ "type": "pong", "t": msg["t"] })),
            Some("unpair") if !self.ignores_unpair.get() => self.paired.borrow_mut().retain(|k| *k != s.phone_key),
            Some("exec") if self.home.borrow().is_some() => {
                use base64::Engine;
                let stdin = input.and_then(|i| base64::engine::general_purpose::STANDARD.decode(i.as_str()?).ok());
                let (code, stdout, stderr) = run_sh(msg["cmd"].as_str().unwrap_or_default(), stdin, self.home.borrow().as_deref().unwrap_or_default());
                self.send(s, &json!({ "id": msg["id"], "type": "exit", "code": code, "stdout": stdout, "stderr": stderr }))
            }
            Some("exec") => {
                self.send(s, &json!({ "id": msg["id"], "type": "exit", "code": 0, "stdout": format!("ran {}", msg["cmd"].as_str().unwrap_or_default()), "stderr": "" }))
            }
            _ => {}
        }
    }

    pub fn send(&self, s: &Session, msg: &Value) {
        for f in s.channel.borrow_mut().seal(msg.to_string().as_bytes()).unwrap() {
            later(&s.to_phone, WireEvent::Message(f));
        }
    }

    /// The machine goes away: every open channel drops without a goodbye.
    pub fn vanish(&self) {
        for s in self.sessions.borrow_mut().drain(..) {
            later(&s.to_phone, WireEvent::Close(1006, String::new()));
        }
    }
}

/// `sh -c cmd` with only `home` as HOME, so nothing a test runs can reach the real one.
fn run_sh(cmd: &str, stdin: Option<Vec<u8>>, home: &str) -> (i32, String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&stdin.unwrap_or_default());
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}
