//! Notifications while Inyon is closed, in a browser (apps/web/src/lib/push.ts; the Android app
//! has its own). Once notifications are allowed, the page subscribes with the browser's push
//! service and hands the subscription to the hub, which sends them itself. The push service sees
//! only ciphertext.
//!
//! A subscription is tied to one signing (VAPID) key, which this page makes once and keeps where
//! the web app keeps it, so both copies of the app share one subscription. The service worker that
//! shows the notifications is the web app's (`/sw.js`).

use serde::Serialize;

/// Where the browser keeps this page's VAPID key (the web app's key, so both apps share it).
pub const KEY: &str = "inyon.push-vapid";

/// What `POST /api/push` takes: the subscription, the key to sign with, and which notices to send.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushBody {
    pub subscription: serde_json::Value,
    pub vapid: Vapid,
    pub done: bool,
    pub pages: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vapid {
    pub private_jwk: serde_json::Value,
    pub subject: &'static str,
}

/// Base64url without padding, as a JWK writes its numbers.
fn unb64u(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The public half of a P-256 key, as the push service takes it: 0x04 ‖ x ‖ y.
pub fn raw_key(x: &str, y: &str) -> Option<Vec<u8>> {
    let (x, y) = (unb64u(x)?, unb64u(y)?);
    (x.len() == 32 && y.len() == 32).then(|| [&[4u8][..], &x, &y].concat())
}

/// The private key handed to the hub: only the fields a JWK for ECDSA P-256 needs.
pub fn private_jwk(saved: &serde_json::Value) -> Option<serde_json::Value> {
    let f = |k: &str| saved.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
    Some(serde_json::json!({ "kty": "EC", "crv": "P-256", "d": f("d")?, "x": f("x")?, "y": f("y")? }))
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array, JSON};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{private_jwk, raw_key, PushBody, Vapid, KEY};

    fn get(o: &JsValue, k: &str) -> JsValue {
        Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED)
    }

    fn call(o: &JsValue, method: &str, args: &[&JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(o, method).dyn_into()?;
        f.apply(o, &args.iter().copied().collect::<Array>())
    }

    async fn wait(v: JsValue) -> Result<JsValue, JsValue> {
        JsFuture::from(v.dyn_into::<Promise>()?).await
    }

    fn why(e: &JsValue) -> String {
        get(e, "message").as_string().or_else(|| e.as_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| "failed".into())
    }

    fn navigator() -> JsValue {
        web_sys::window().map(|w| w.navigator().into()).unwrap_or(JsValue::UNDEFINED)
    }

    /// Push needs HTTPS and a service worker; Safari has it only for an app on the Home Screen.
    pub fn supported() -> bool {
        let g = js_sys::global();
        crate::dom::secure()
            && Reflect::has(&navigator(), &"serviceWorker".into()).unwrap_or(false)
            && Reflect::has(&g, &"PushManager".into()).unwrap_or(false)
            && Reflect::has(&g, &"Notification".into()).unwrap_or(false)
    }

    /// The service worker that shows the notifications, registered at the hub's root as the web
    /// app does; none after 10 seconds.
    async fn worker() -> Option<JsValue> {
        let sw = get(&navigator(), "serviceWorker");
        let opts = Object::new();
        let _ = Reflect::set(&opts, &"scope".into(), &"/".into());
        if let Ok(p) = call(&sw, "register", &[&"/sw.js".into(), &opts]) {
            // already registered by the web app, or not allowed here: `ready` says which
            let _ = wait(p).await;
        }
        let ready = get(&sw, "ready");
        let timeout = Promise::new(&mut |resolve, _| {
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 10_000);
            }
        });
        let reg = JsFuture::from(Promise::race(&Array::of2(&ready, &timeout))).await.ok()?;
        (!reg.is_undefined() && !reg.is_null()).then_some(reg)
    }

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    /// This browser's VAPID key, made once: the private JWK and the public key's raw bytes.
    async fn vapid_key() -> Result<(serde_json::Value, Vec<u8>), String> {
        let saved: Option<serde_json::Value> = storage().and_then(|s| s.get_item(KEY).ok().flatten()).and_then(|t| serde_json::from_str(&t).ok());
        let jwk = match saved.as_ref().and_then(private_jwk) {
            Some(j) => j,
            None => {
                let subtle = get(&get(&js_sys::global(), "crypto"), "subtle");
                let algo = Object::new();
                let _ = Reflect::set(&algo, &"name".into(), &"ECDSA".into());
                let _ = Reflect::set(&algo, &"namedCurve".into(), &"P-256".into());
                let uses = Array::of1(&"sign".into());
                let pair = wait(call(&subtle, "generateKey", &[&algo, &JsValue::TRUE, &uses]).map_err(|e| why(&e))?).await.map_err(|e| why(&e))?;
                let exported = wait(call(&subtle, "exportKey", &[&"jwk".into(), &get(&pair, "privateKey")]).map_err(|e| why(&e))?).await.map_err(|e| why(&e))?;
                let text = JSON::stringify(&exported).ok().and_then(|t| t.as_string()).ok_or("couldn't read the key")?;
                if let Some(s) = storage() {
                    let _ = s.set_item(KEY, &text);
                }
                serde_json::from_str::<serde_json::Value>(&text).ok().as_ref().and_then(private_jwk).ok_or("couldn't read the key")?
            }
        };
        let raw = raw_key(jwk["x"].as_str().unwrap_or(""), jwk["y"].as_str().unwrap_or("")).ok_or("the saved key is broken")?;
        Ok((jwk, raw))
    }

    thread_local! {
        static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Subscribes (or keeps the subscription) and hands it to the hub; its endpoint, or why not.
    /// One at a time, so an older hand-over landing last can't put back a switch just turned off.
    pub async fn enable(done: bool, pages: bool) -> Result<String, String> {
        while BUSY.with(|b| b.replace(true)) {
            crate::platform::sleep(50).await;
        }
        let out = register(done, pages).await;
        BUSY.with(|b| b.set(false));
        out
    }

    async fn register(done: bool, pages: bool) -> Result<String, String> {
        if !supported() {
            return Err("unsupported".into());
        }
        if crate::dom::notice_permission() != crate::settings::Permission::Granted {
            return Err("not allowed".into());
        }
        let reg = worker().await.ok_or("no service worker")?;
        let (jwk, raw) = vapid_key().await?;
        let pm = get(&reg, "pushManager");
        let mut sub = wait(call(&pm, "getSubscription", &[]).map_err(|e| why(&e))?).await.map_err(|e| why(&e))?;
        // one made with another key can't be signed for: start over
        if !sub.is_null() && !sub.is_undefined() {
            let key = get(&get(&sub, "options"), "applicationServerKey");
            let same = !key.is_null() && !key.is_undefined() && Uint8Array::new(&key).to_vec() == raw;
            if !same {
                if let Ok(p) = call(&sub, "unsubscribe", &[]) {
                    let _ = wait(p).await;
                }
                sub = JsValue::NULL;
            }
        }
        if sub.is_null() || sub.is_undefined() {
            let opts = Object::new();
            let _ = Reflect::set(&opts, &"userVisibleOnly".into(), &JsValue::TRUE);
            let _ = Reflect::set(&opts, &"applicationServerKey".into(), &Uint8Array::from(&raw[..]));
            sub = wait(call(&pm, "subscribe", &[&opts]).map_err(|e| why(&e))?).await.map_err(|e| why(&e))?;
        }
        let json = call(&sub, "toJSON", &[]).map_err(|e| why(&e))?;
        let text = JSON::stringify(&json).ok().and_then(|t| t.as_string()).ok_or("couldn't read the subscription")?;
        let subscription: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let endpoint = subscription["endpoint"].as_str().unwrap_or_default().to_string();
        let body = PushBody { subscription, vapid: Vapid { private_jwk: jwk, subject: inyon_protocol::WEB_URL }, done, pages };
        crate::api::push(&body).await.map_err(|e| e.message)?;
        Ok(endpoint)
    }

    /// The service worker brings a tapped notification to this page: go to its session.
    pub fn listen() {
        let sw = get(&navigator(), "serviceWorker");
        let on = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let data = get(&e, "data");
            if get(&data, "t").as_string().as_deref() != Some("open") {
                return;
            }
            let Some(href) = get(&data, "href").as_string().filter(|h| h.starts_with('/') && !h.starts_with("//") && !h.starts_with("/\\")) else { return };
            let Some(w) = web_sys::window() else { return };
            if let Ok(h) = w.history() {
                let _ = h.push_state_with_url(&JsValue::NULL, "", Some(&format!("{}{href}", crate::route::BASE)));
            }
            if let Ok(e) = web_sys::Event::new("popstate") {
                let _ = w.dispatch_event(&e);
            }
        });
        let _ = call(&sw, "addEventListener", &[&"message".into(), on.as_ref()]);
        on.forget();
    }
}

#[cfg(target_arch = "wasm32")]
pub use imp::{enable, listen, supported};

#[cfg(not(target_arch = "wasm32"))]
pub fn supported() -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn enable(_done: bool, _pages: bool) -> Result<String, String> {
    Err("unsupported".into())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn listen() {}

/// A test through the push service: how many senders delivered it (none when push isn't on).
pub async fn test(done: bool, pages: bool) -> u32 {
    let Ok(endpoint) = enable(done, pages).await else { return 0 };
    crate::api::push_test(&endpoint).await.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_key_is_four_then_x_then_y() {
        // 32 bytes of 0x01 and 0x02, base64url without padding
        let x = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE";
        let y = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI";
        let raw = raw_key(x, y).unwrap();
        assert_eq!(raw.len(), 65);
        assert_eq!((raw[0], raw[1], raw[64]), (4, 1, 2));
        assert_eq!(raw_key("AQ", y), None);
        assert_eq!(raw_key("A*", y), None);
    }

    #[test]
    fn only_a_whole_key_is_handed_over() {
        let saved = serde_json::json!({ "kty": "EC", "crv": "P-256", "d": "dd", "x": "xx", "y": "yy", "ext": true, "key_ops": ["sign"] });
        assert_eq!(private_jwk(&saved), Some(serde_json::json!({ "kty": "EC", "crv": "P-256", "d": "dd", "x": "xx", "y": "yy" })));
        assert_eq!(private_jwk(&serde_json::json!({ "x": "xx", "y": "yy" })), None);
    }

    #[test]
    fn the_hub_gets_the_web_apps_body() {
        let body = PushBody {
            subscription: serde_json::json!({ "endpoint": "https://push.example/1" }),
            vapid: Vapid { private_jwk: serde_json::json!({}), subject: "https://inyon.tech/" },
            done: true,
            pages: false,
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({ "subscription": { "endpoint": "https://push.example/1" }, "vapid": { "privateJwk": {}, "subject": "https://inyon.tech/" }, "done": true, "pages": false })
        );
    }
}
