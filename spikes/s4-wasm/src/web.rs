//! The browser side: wasm-bindgen exports that run the link over a WebSocket the Rust code owns.

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Function, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{BinaryType, CloseEvent, MessageEvent, WebSocket};

use crate::identity::{noise_key_of, Identity};
use crate::link::{Channel, Initiator};

fn seed32(seed: &[u8]) -> Result<[u8; 32], JsError> {
    seed.try_into().map_err(|_| JsError::new("a seed is 32 bytes"))
}

/// The machine id (z-base-32 public key) for a seed.
#[wasm_bindgen(js_name = identityId)]
pub fn identity_id(seed: &[u8]) -> Result<String, JsError> {
    Ok(Identity::from_seed(&seed32(seed)?).id)
}

#[wasm_bindgen(js_name = verify)]
pub fn verify_js(id: &str, msg: &[u8], sig: &[u8]) -> bool {
    crate::identity::verify(id, msg, sig)
}

/// Runs `n` full handshakes (phone and machine both in wasm) plus a 64 KB message each way, for
/// timing the crypto without the network. Returns how many succeeded.
#[wasm_bindgen(js_name = benchHandshakes)]
pub fn bench_handshakes(n: u32) -> u32 {
    let phone = Identity::from_seed(&[1; 32]);
    let machine = Identity::from_seed(&[2; 32]);
    let body = format!("{{\"data\":\"{}\"}}", "x".repeat(64 * 1024));
    let mut ok = 0;
    for _ in 0..n {
        let run = || -> Result<bool, crate::link::LinkError> {
            let mut i = Initiator::new(&phone.noise_secret, &machine.noise_public)?;
            let mut r = crate::link::Responder::new(&machine.noise_secret)?;
            r.message1(&i.message1("{}")?)?;
            let (m2, mut rc) = r.message2(r#"{"ok":true}"#)?;
            let (_, mut ic) = i.message2(&m2)?;
            let mut got = None;
            for m in ic.seal(&body)? {
                got = rc.open(&m)?;
            }
            Ok(got.as_deref() == Some(body.as_str()))
        };
        if run().unwrap_or(false) {
            ok += 1;
        }
    }
    ok
}

struct Inner {
    ws: WebSocket,
    chan: Channel,
    identity: Identity,
    onmessage: Option<Function>,
    onclose: Option<Function>,
    closed: bool,
    // kept so the callbacks live as long as the link
    _handlers: Vec<Closure<dyn FnMut(JsValue)>>,
}

/// An open, encrypted link to a machine.
#[wasm_bindgen]
pub struct Link {
    inner: Rc<RefCell<Inner>>,
    welcome: String,
}

#[wasm_bindgen]
impl Link {
    /// The machine's welcome (JSON).
    #[wasm_bindgen(getter)]
    pub fn welcome(&self) -> String {
        self.welcome.clone()
    }

    /// The handshake hash, the same on both ends.
    #[wasm_bindgen(getter)]
    pub fn binding(&self) -> Vec<u8> {
        self.inner.borrow().chan.binding().to_vec()
    }

    /// The phone's ed25519 signature over the handshake hash: proves the identity on this channel.
    #[wasm_bindgen(js_name = signBinding)]
    pub fn sign_binding(&self) -> Vec<u8> {
        let i = self.inner.borrow();
        i.identity.sign(i.chan.binding()).to_vec()
    }

    /// Sends one JSON text (chunked and encrypted).
    pub fn send(&self, json: &str) -> Result<(), JsError> {
        let mut i = self.inner.borrow_mut();
        if i.closed {
            return Err(JsError::new("link closed"));
        }
        for m in i.chan.seal(json).map_err(|e| JsError::new(&e.to_string()))? {
            i.ws.send_with_u8_array(&m).map_err(|_| JsError::new("send failed"))?;
        }
        Ok(())
    }

    /// `f(json)` for each whole message the machine sends.
    #[wasm_bindgen(js_name = setOnMessage)]
    pub fn set_onmessage(&self, f: Function) {
        self.inner.borrow_mut().onmessage = Some(f);
    }

    #[wasm_bindgen(js_name = setOnClose)]
    pub fn set_onclose(&self, f: Function) {
        self.inner.borrow_mut().onclose = Some(f);
    }

    pub fn close(&self) {
        let mut i = self.inner.borrow_mut();
        i.closed = true;
        let _ = i.ws.close_with_code(1000);
    }
}

/// Opens a WebSocket to `url` and runs the phone's side of the handshake against the machine
/// `machine_id`, sending `hello` (JSON) inside message 1.
#[wasm_bindgen]
pub async fn connect(url: String, machine_id: String, seed: Vec<u8>, hello: String) -> Result<Link, JsError> {
    let identity = Identity::from_seed(&seed32(&seed)?);
    let machine_key = noise_key_of(&machine_id).ok_or_else(|| JsError::new("Not an Inyon machine id"))?;
    let mut hs = Initiator::new(&identity.noise_secret, &machine_key).map_err(|e| JsError::new(&e.to_string()))?;
    let msg1 = hs.message1(&hello).map_err(|e| JsError::new(&e.to_string()))?;

    let ws = WebSocket::new(&url).map_err(|_| JsError::new("bad url"))?;
    ws.set_binary_type(BinaryType::Arraybuffer);

    // message 1 goes out on open; the first message back is message 2
    let first = {
        let ws = ws.clone();
        Promise::new(&mut move |resolve, reject| {
            let ws2 = ws.clone();
            let msg1 = msg1.clone();
            let on_open = Closure::once_into_js(move || {
                let _ = ws2.send_with_u8_array(&msg1);
            });
            ws.set_onopen(Some(on_open.unchecked_ref()));
            let on_msg = Closure::once_into_js(move |e: MessageEvent| {
                let _ = resolve.call1(&JsValue::NULL, &e.data());
            });
            ws.set_onmessage(Some(on_msg.unchecked_ref()));
            let on_close = Closure::once_into_js(move |e: CloseEvent| {
                let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&format!("closed ({}) {}", e.code(), e.reason())));
            });
            ws.set_onclose(Some(on_close.unchecked_ref()));
        })
    };
    let data = JsFuture::from(first).await.map_err(|e| JsError::new(&e.as_string().unwrap_or_else(|| "connection failed".into())))?;
    let msg2 = Uint8Array::new(&data).to_vec();
    let (welcome, chan) = match hs.message2(&msg2) {
        Ok(v) => v,
        Err(e) => {
            let _ = ws.close_with_code_and_reason(4401, "bad handshake");
            return Err(JsError::new(&e.to_string()));
        }
    };

    let inner = Rc::new(RefCell::new(Inner { ws: ws.clone(), chan, identity, onmessage: None, onclose: None, closed: false, _handlers: Vec::new() }));
    let on_msg = {
        let weak = Rc::downgrade(&inner);
        Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let Some(inner) = weak.upgrade() else { return };
            let e: MessageEvent = e.unchecked_into();
            let bytes = Uint8Array::new(&e.data()).to_vec();
            let opened = inner.borrow_mut().chan.open(&bytes);
            match opened {
                Ok(Some(json)) => {
                    let f = inner.borrow().onmessage.clone();
                    if let Some(f) = f {
                        let _ = f.call1(&JsValue::NULL, &JsValue::from_str(&json));
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    let mut i = inner.borrow_mut();
                    i.closed = true;
                    let _ = i.ws.close_with_code_and_reason(4400, "bad message");
                }
            }
        })
    };
    let on_close = {
        let weak = Rc::downgrade(&inner);
        Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let Some(inner) = weak.upgrade() else { return };
            let e: CloseEvent = e.unchecked_into();
            inner.borrow_mut().closed = true;
            let f = inner.borrow().onclose.clone();
            if let Some(f) = f {
                let _ = f.call2(&JsValue::NULL, &JsValue::from(e.code()), &JsValue::from_str(&e.reason()));
            }
        })
    };
    ws.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    inner.borrow_mut()._handlers = vec![on_msg, on_close];
    Ok(Link { inner, welcome })
}
