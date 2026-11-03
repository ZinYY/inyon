//! Port of the TypeScript phone core's ssh-backend.ts (removed in 0.23): machines reached straight from the phone over SSH,
//! through the native InyonSsh plugin. A connection opens on first use and again after it drops;
//! the plugin pools connections per server.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use futures_util::future::LocalBoxFuture;
use inyon_core::clock::now_ms;
use inyon_core::hosts::host::{shq, ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use inyon_core::hosts::http_error::HubResult;
use inyon_core::js::json::Js;
use inyon_core::platform::rt::{spawn, Deferred};
use inyon_protocol::{HostAuth, HostConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// A refusal from the plugin: its code (`HOST_KEY_MISMATCH`, `AUTH_FAILED`, `UNREACHABLE`) and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub code: Option<String>,
    pub message: String,
}

impl PluginError {
    pub fn plain(message: impl Into<String>) -> Self {
        PluginError { code: None, message: message.into() }
    }
}

pub type PluginResult = LocalBoxFuture<'static, Result<Value, PluginError>>;
/// Called with each event's name and what it carries.
pub type OnEvent = Rc<dyn Fn(&str, &Value)>;

/// The native InyonSsh plugin (SshPlugin in apps/web/src/lib/phone-types.ts): each method takes and
/// gives the same JSON the plugin does.
pub trait SshPlugin {
    fn call(&self, method: &'static str, args: Value) -> PluginResult;
    /// Hands `on` each event the plugin sends: streamLines, streamExit, ptyData, ptyExit, closed.
    fn listen(&self, on: OnEvent);
}

/// How a server signs in, read from the phone's keystore when a connection opens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ed25519_seed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ed25519_public: Option<String>,
}

pub type CredentialsFor = Rc<dyn Fn(&HostConfig) -> LocalBoxFuture<'static, Result<Credentials, String>>>;

/// "user@host" → its parts (the phone has no ~/.ssh/config, so the user is always given).
pub fn split_target(target: &str) -> (String, String) {
    match target.rfind('@') {
        Some(at) if at > 0 => (target[..at].to_owned(), target[at + 1..].to_owned()),
        _ => (String::new(), target.to_owned()),
    }
}

// a server that just refused us isn't asked again on every poll
const RETRY_MS: u64 = 8000;

type Lines = Rc<RefCell<OnLine>>;
type Data = Rc<RefCell<OnData>>;

struct Inner {
    plugin: Rc<dyn SshPlugin>,
    credentials: CredentialsFor,
    conns: RefCell<HashMap<String, Deferred<Result<(), String>>>>,
    failed: RefCell<HashMap<String, (u64, String)>>,
    streams: RefCell<HashMap<String, (Lines, Deferred<i32>)>>,
    ptys: RefCell<HashMap<String, (Data, Deferred<i32>)>>,
    seq: Cell<u64>,
}

/// The backend for servers the phone dials itself; `forget` closes one's connection.
#[derive(Clone)]
pub struct SshBackend(Rc<Inner>);

impl SshBackend {
    pub fn new(plugin: Rc<dyn SshPlugin>, credentials: CredentialsFor) -> Self {
        let inner = Rc::new(Inner {
            plugin: plugin.clone(),
            credentials,
            conns: RefCell::default(),
            failed: RefCell::default(),
            streams: RefCell::default(),
            ptys: RefCell::default(),
            seq: Cell::new(0),
        });
        let me = Rc::downgrade(&inner);
        plugin.listen(Rc::new(move |event, e| {
            if let Some(me) = me.upgrade() {
                me.event(event, e);
            }
        }));
        SshBackend(inner)
    }

    /// Closes a server's connections (it was removed, or its settings changed).
    pub async fn forget(&self, id: &str) {
        self.0.conns.borrow_mut().remove(id);
        self.0.failed.borrow_mut().remove(id);
        let _ = self.0.plugin.call("disconnect", json!({ "id": id })).await;
    }
}

impl Inner {
    fn event(&self, event: &str, e: &Value) {
        let e = Some(e);
        match event {
            "streamLines" => {
                let Some(on) = self.streams.borrow().get(e.at("streamId").str().unwrap_or_default()).map(|s| s.0.clone()) else { return };
                // the hub's own streams skip empty lines, so these do too
                for line in e.at("lines").arr().into_iter().flatten().filter_map(|l| l.as_str()).filter(|l| !l.is_empty()) {
                    (on.borrow_mut())(line.to_owned());
                }
            }
            "streamExit" => finish(&self.streams, e.at("streamId").str().unwrap_or_default(), e.at("code").num().unwrap_or(1.0) as i32),
            "ptyData" => {
                let Some(on) = self.ptys.borrow().get(e.at("ptyId").str().unwrap_or_default()).map(|p| p.0.clone()) else { return };
                if let Ok(bytes) = B64.decode(e.at("data").str().unwrap_or_default()) {
                    (on.borrow_mut())(bytes);
                }
            }
            "ptyExit" => finish(&self.ptys, e.at("ptyId").str().unwrap_or_default(), e.at("code").num().unwrap_or(1.0) as i32),
            "closed" => {
                self.conns.borrow_mut().remove(e.at("id").str().unwrap_or_default());
            }
            _ => {}
        }
    }

    fn next(&self, prefix: char) -> String {
        self.seq.set(self.seq.get() + 1);
        format!("{prefix}{}", self.seq.get())
    }

    /// The server's connection, opened if it isn't; a described error when it can't be.
    fn connect(self: &Rc<Self>, config: &HostConfig) -> LocalBoxFuture<'static, Result<(), String>> {
        if let Some(p) = self.conns.borrow().get(&config.id) {
            return Box::pin(p.wait());
        }
        if let Some((at, error)) = self.failed.borrow().get(&config.id) {
            if now_ms().saturating_sub(*at) < RETRY_MS {
                let error = error.clone();
                return Box::pin(async move { Err(error) });
            }
        }
        let p = Deferred::new();
        self.conns.borrow_mut().insert(config.id.clone(), p.clone());
        let (me, config, out) = (self.clone(), config.clone(), p.clone());
        spawn(async move {
            match me.open(&config).await {
                Ok(()) => {
                    me.failed.borrow_mut().remove(&config.id);
                    out.resolve(Ok(()));
                }
                Err(err) => {
                    let error = describe(&err, Some(&config));
                    // an attempt forget() dropped (the settings changed meanwhile) says nothing
                    // about the next one
                    if me.conns.borrow().get(&config.id).is_some_and(|c| c.same(&out)) {
                        me.conns.borrow_mut().remove(&config.id);
                        me.failed.borrow_mut().insert(config.id.clone(), (now_ms(), error.clone()));
                    }
                    out.resolve(Err(error));
                }
            }
        });
        Box::pin(p.wait())
    }

    async fn open(&self, config: &HostConfig) -> Result<(), PluginError> {
        let (username, host) = split_target(config.target.as_deref().unwrap_or_default());
        if username.is_empty() {
            return Err(PluginError::plain("Add the user name, like you@server"));
        }
        let Some(host_key) = config.host_key.as_deref().filter(|k| !k.is_empty()) else {
            return Err(PluginError::plain("This server's key hasn't been confirmed. Edit it and check the fingerprint."));
        };
        let creds = (self.credentials)(config).await.map_err(PluginError::plain)?;
        let mut args = json!({ "id": config.id, "host": host, "port": config.port.unwrap_or(22), "username": username, "hostKey": host_key });
        if let (Some(a), Ok(Value::Object(c))) = (args.as_object_mut(), serde_json::to_value(creds)) {
            a.extend(c);
        }
        self.plugin.call("connect", args).await.map(|_| ())
    }
}

/// Ends a stream or terminal: its exit code settles, and it leaves the list.
fn finish<T>(list: &RefCell<HashMap<String, (T, Deferred<i32>)>>, id: &str, code: i32) {
    let gone = list.borrow_mut().remove(id);
    if let Some((_, exited)) = gone {
        exited.resolve(code);
    }
}

fn sh(cmd: &str) -> String {
    format!("sh -c {}", shq(cmd))
}

impl HostBackend for SshBackend {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        let (me, config, command) = (self.0.clone(), config.clone(), sh(cmd));
        Box::pin(async move {
            // ssh's own "couldn't connect" code, so callers treat it like the hub's ssh failing
            let failed = |stderr: String| Ok(ExecResult { code: 255, stdout: String::new(), stderr });
            if let Err(e) = me.connect(&config).await {
                return failed(e);
            }
            let mut args = json!({ "id": config.id, "command": command, "timeoutMs": opts.timeout_ms.unwrap_or(20_000) });
            if let Some(input) = &opts.input {
                args["stdin"] = Value::String(B64.encode(input));
            }
            match me.plugin.call("exec", args).await {
                Ok(r) => {
                    let r = Some(&r);
                    Ok(ExecResult {
                        code: r.at("code").num().unwrap_or(255.0) as i32,
                        stdout: r.at("stdout").str().unwrap_or_default().to_owned(),
                        stderr: r.at("stderr").str().unwrap_or_default().to_owned(),
                    })
                }
                Err(e) => failed(describe(&e, None)),
            }
        })
    }

    fn stream(&self, config: &HostConfig, cmd: &str, on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        let me = self.0.clone();
        let stream_id = me.next('s');
        let exited = Deferred::new();
        me.streams.borrow_mut().insert(stream_id.clone(), (Rc::new(RefCell::new(on_line)), exited.clone()));
        let (killed, started) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let stop = {
            let (me, id) = (me.clone(), stream_id.clone());
            Rc::new(move || {
                let (me, id) = (me.clone(), id.clone());
                spawn(async move {
                    if me.plugin.call("streamKill", json!({ "streamId": id })).await.is_err() {
                        finish(&me.streams, &id, 143);
                    }
                });
            })
        };
        {
            let (killed, started, stop, config) = (killed.clone(), started.clone(), stop.clone(), config.clone());
            let args = json!({ "id": config.id, "streamId": stream_id, "command": sh(cmd), "keepStdin": keep_stdin });
            spawn(async move {
                if me.connect(&config).await.is_err() {
                    return finish(&me.streams, &stream_id, 255);
                }
                if killed.get() {
                    return finish(&me.streams, &stream_id, 143);
                }
                if me.plugin.call("stream", args).await.is_err() {
                    return finish(&me.streams, &stream_id, 255);
                }
                // the plugin only knows the stream once that call returns, so a kill sent earlier missed it
                started.set(true);
                if killed.get() {
                    stop();
                }
            });
        }
        StreamHandle::new(
            move || {
                killed.set(true);
                if started.get() {
                    stop();
                }
            },
            exited,
        )
    }

    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle> {
        let me = self.0.clone();
        let pty_id = me.next('p');
        let exited = Deferred::new();
        let on_data: Data = Rc::new(RefCell::new(on_data));
        me.ptys.borrow_mut().insert(pty_id.clone(), (on_data.clone(), exited.clone()));
        let (killed, opening, started) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
        let send = {
            let me = me.clone();
            move |method: &'static str, args: Value| {
                let me = me.clone();
                spawn(async move {
                    let _ = me.plugin.call(method, args).await;
                });
            }
        };
        let stop = {
            let (me, id) = (me.clone(), pty_id.clone());
            Rc::new(move || {
                let (me, id) = (me.clone(), id.clone());
                spawn(async move {
                    if me.plugin.call("ptyKill", json!({ "ptyId": id })).await.is_err() {
                        finish(&me.ptys, &id, 143);
                    }
                });
            })
        };
        {
            let (killed, opening, started, stop, config) = (killed.clone(), opening.clone(), started.clone(), stop.clone(), config.clone());
            let args = json!({ "id": config.id, "ptyId": pty_id, "command": sh(cmd), "cols": cols, "rows": rows });
            let pty_id = pty_id.clone();
            spawn(async move {
                let opened = async {
                    me.connect(&config).await?;
                    if killed.get() {
                        return Ok(false);
                    }
                    opening.set(true);
                    me.plugin.call("pty", args).await.map_err(|e| describe(&e, None))?;
                    Ok::<_, String>(true)
                };
                match opened.await {
                    Ok(false) => finish(&me.ptys, &pty_id, 143),
                    Ok(true) => {
                        started.set(true);
                        if killed.get() {
                            stop();
                        }
                    }
                    Err(e) => {
                        (on_data.borrow_mut())(format!("\r\n{e}\r\n").into_bytes());
                        finish(&me.ptys, &pty_id, 255);
                    }
                }
            });
        }
        let (write, resize) = (send.clone(), send);
        let (o1, o2, id1, id2) = (opening.clone(), opening, pty_id.clone(), pty_id);
        Ok(PtyHandle::new(
            move |data| {
                if o1.get() {
                    write("ptyWrite", json!({ "ptyId": id1, "data": data }));
                }
            },
            move |c, r| {
                if o2.get() {
                    resize("ptyResize", json!({ "ptyId": id2, "cols": c, "rows": r }));
                }
            },
            move || {
                killed.set(true);
                if started.get() {
                    stop();
                }
            },
            exited,
        ))
    }
}

/// The plugin's error codes as something a person can act on.
pub fn describe(e: &PluginError, config: Option<&HostConfig>) -> String {
    let msg = &e.message;
    match e.code.as_deref() {
        Some("HOST_KEY_MISMATCH") => format!("The server's key changed. If you reinstalled it, edit the server and confirm the new fingerprint. ({msg})"),
        Some("AUTH_FAILED") => {
            let check = match config.and_then(|c| c.auth) {
                Some(HostAuth::Password) => "the password",
                Some(HostAuth::Key) => "that its public key is in ~/.ssh/authorized_keys",
                _ => "that this phone's key is in ~/.ssh/authorized_keys",
            };
            format!("The server didn't accept the sign-in. Check the user name and {check}. ({msg})")
        }
        Some("UNREACHABLE") => format!("Can't reach the server. Check the address and that this phone is on the same network or VPN. ({msg})"),
        _ => msg.clone(),
    }
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
