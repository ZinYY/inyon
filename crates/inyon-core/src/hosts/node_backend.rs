//! Port of hosts/node-backend.ts: machines running inyon, whose commands travel over the node's
//! outbound WebSocket (node_link).

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use futures_util::future::LocalBoxFuture;
use inyon_protocol::HostConfig;
use serde_json::{json, Value};

use super::host::{ExecOptions, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use super::http_error::HubResult;
use super::node_link::{self, NodeOp};
use crate::js::json::Js;
use crate::platform::rt::{set_timeout, Deferred, Timer};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Clone, Copy, Default)]
pub struct NodeBackend;

/// The node backend (`nodeBackend` in the TS).
pub fn node_backend() -> Rc<dyn HostBackend> {
    Rc::new(NodeBackend)
}

fn ended(m: &Value) -> bool {
    matches!(Some(m).at("type").str(), Some("exit" | "error"))
}

/// `m.code ?? 1`
fn code(m: &Value) -> i32 {
    Some(m).at("code").num().map_or(1, |c| c as i32)
}

impl HostBackend for NodeBackend {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        // a machine that dropped a moment ago is most likely coming back: a message sent meanwhile
        // goes in once it's there, rather than failing
        if node_link::reconnecting(&config.id) {
            let (config, cmd) = (config.clone(), cmd.to_owned());
            return Box::pin(async move {
                node_link::back(&config.id).await;
                NodeBackend.exec(&config, &cmd, opts).await
            });
        }
        let result: Deferred<ExecResult> = Deferred::new();
        let op_cell: Rc<RefCell<Option<NodeOp>>> = Rc::default();
        let timer_cell: Rc<RefCell<Option<Timer>>> = Rc::default();
        let mut op = json!({ "op": "exec", "cmd": cmd });
        if let Some(input) = &opts.input {
            op["input"] = Value::String(B64.encode(input));
        }
        let timer = {
            let (op_cell, result) = (op_cell.clone(), result.clone());
            set_timeout(opts.timeout_ms.unwrap_or(20_000), move || {
                if let Some(op) = op_cell.borrow().clone() {
                    op.send(json!({ "op": "kill" }));
                    op.done();
                }
                result.resolve(ExecResult { code: 124, stdout: String::new(), stderr: "timed out".into() });
            })
        };
        *timer_cell.borrow_mut() = Some(timer.clone());
        let handler = {
            let (op_cell, timer_cell, result) = (op_cell.clone(), timer_cell.clone(), result.clone());
            move |m: &Value| {
                if !ended(m) {
                    return;
                }
                if let Some(t) = timer_cell.borrow_mut().take() {
                    t.clear();
                }
                if let Some(op) = op_cell.borrow().clone() {
                    op.done();
                }
                let raw = m;
                let m = Some(m);
                if m.at("type").str() == Some("error") {
                    result.resolve(ExecResult { code: 255, stdout: String::new(), stderr: m.at("error").string_or("node error") });
                } else {
                    result.resolve(ExecResult { code: code(raw), stdout: m.at("stdout").string_or_empty(), stderr: m.at("stderr").string_or_empty() });
                }
            }
        };
        match node_link::start(&config.id, op, handler) {
            Ok(op) => {
                // answered before start returned (a socket that replies at once): nothing more comes
                if result.is_settled() {
                    op.done();
                }
                *op_cell.borrow_mut() = Some(op);
            }
            Err(e) => {
                timer.clear();
                return Box::pin(async move { Err(e) });
            }
        }
        Box::pin(async move { Ok(result.wait().await) })
    }

    fn stream(&self, config: &HostConfig, cmd: &str, mut on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        let exited: Deferred<i32> = Deferred::new();
        let op_cell: Rc<RefCell<Option<NodeOp>>> = Rc::default();
        let handler = {
            let (exited, op_cell) = (exited.clone(), op_cell.clone());
            move |m: &Value| {
                let mm = Some(m);
                if mm.at("type").str() == Some("line") {
                    on_line(mm.at("data").to_js_string());
                } else if ended(m) {
                    if let Some(op) = op_cell.borrow().clone() {
                        op.done();
                    }
                    exited.resolve(code(m));
                }
            }
        };
        match node_link::start(&config.id, json!({ "op": "stream", "cmd": cmd, "keepStdin": keep_stdin }), handler) {
            Ok(op) => {
                if exited.is_settled() {
                    op.done();
                }
                *op_cell.borrow_mut() = Some(op.clone());
                let done = exited.clone();
                StreamHandle::new(
                    move || {
                        op.send(json!({ "op": "kill" }));
                        op.done();
                        done.resolve(143);
                    },
                    exited,
                )
            }
            Err(_) => {
                exited.resolve(255);
                StreamHandle::new(|| {}, exited)
            }
        }
    }

    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, mut on_data: OnData) -> HubResult<PtyHandle> {
        let exited: Deferred<i32> = Deferred::new();
        let op_cell: Rc<RefCell<Option<NodeOp>>> = Rc::default();
        let handler = {
            let (exited, op_cell) = (exited.clone(), op_cell.clone());
            move |m: &Value| {
                let mm = Some(m);
                if mm.at("type").str() == Some("data") {
                    on_data(B64.decode(mm.at("data").str().unwrap_or("")).unwrap_or_default());
                } else if ended(m) {
                    if let Some(op) = op_cell.borrow().clone() {
                        op.done();
                    }
                    exited.resolve(code(m));
                }
            }
        };
        let op = node_link::start(&config.id, json!({ "op": "pty", "cmd": cmd, "cols": cols, "rows": rows }), handler)?;
        if exited.is_settled() {
            op.done();
        }
        *op_cell.borrow_mut() = Some(op.clone());
        let (w, r, k, done) = (op.clone(), op.clone(), op, exited.clone());
        Ok(PtyHandle::new(
            move |d| w.send(json!({ "op": "write", "data": d })),
            move |c, rr| r.send(json!({ "op": "resize", "cols": c, "rows": rr })),
            move || {
                k.send(json!({ "op": "kill" }));
                k.done();
                done.resolve(143);
            },
            exited,
        ))
    }
}
