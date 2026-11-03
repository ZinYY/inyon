use std::io::{Read, Write};
use std::sync::mpsc as std_mpsc;
use std::sync::Mutex;

use inyon_core::platform::rt::install;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::*;
use crate::testing::{FakeSsh, TokioRuntime};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("inyon-phone-native-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn started(dir: &std::path::Path) -> PhoneHandle {
    start(Options { dir: dir.to_owned(), name: "Test phone".into(), ssh: Box::new(|| FakeSsh::new() as Rc<dyn SshPlugin>), log: None, offline: true })
}

/// The test's own tokio, on real time: what the app's screens run on.
fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

fn json_of(a: &Answer) -> Value {
    serde_json::from_slice(&a.body).unwrap_or(Value::Null)
}

#[test]
fn a_request_from_the_app_reads_as_the_hub_would_get_it() {
    let r = request_of("PUT", "/api/settings?x=1&y=2", Some(b"{}".to_vec()));
    assert_eq!(r.method, Method::PUT);
    assert_eq!((r.path.as_str(), r.query.as_str()), ("/api/settings", "x=1&y=2"));
    assert_eq!(r.headers.get("content-type").unwrap(), "application/json");
    assert!(r.body.is_some());
    let r = request_of("GET", "/api/session", None);
    assert_eq!((r.method, r.query.as_str(), r.headers.len()), (Method::GET, "", 0));
    assert!(r.body.is_none());
}

#[test]
fn the_core_answers_from_its_own_thread_and_keeps_its_records_as_files() {
    let dir = temp_dir("records");
    let phone = started(&dir);
    block_on(async {
        let s = phone.request("GET", "/api/session", None).await.unwrap();
        assert_eq!((s.status, json_of(&s)), (200, json!({ "authed": true, "phone": true })));
        let put = phone.request("PUT", "/api/settings", Some(br#"{"autoResume":true}"#.to_vec())).await.unwrap();
        assert_eq!(put.status, 200);
        let got = phone.request("GET", "/api/settings", None).await.unwrap();
        assert_eq!(json_of(&got)["autoResume"], json!(true));
        // the job runs where the phone is, and says when it isn't there
        assert_eq!(phone.with_phone(|_| async { 7 }).await, Some(7));
    });
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(saved["autoResume"], json!(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_screen_says_hello_follows_the_overview_and_stops_hearing_once_dropped() {
    let dir = temp_dir("screen");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hosts.json"), r#"[{"id":"box","name":"box","kind":"ssh","target":"me@box","hostKey":"SHA256:x"}]"#).unwrap();
    let phone = started(&dir);
    let (tx, rx) = std_mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let screen = phone.connect(move |f| {
        if let Frame::Text(t) = f {
            let _ = tx.lock().unwrap().send(t);
        }
    });
    // no compression asked for, so every frame is text
    screen.send(r#"{"t":"hello","v":2}"#.into());
    screen.send(r#"{"t":"sub","stream":"overview"}"#.into());
    let mut heard = vec![];
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !heard.iter().any(|m: &Value| m["t"] == "snapshot") {
        if let Ok(t) = rx.recv_timeout(Duration::from_millis(200)) {
            heard.push(serde_json::from_str(&t).unwrap());
        }
    }
    assert_eq!(heard[0]["t"], "hello", "{heard:?}");
    let snap = heard.iter().find(|m| m["t"] == "snapshot").unwrap_or_else(|| panic!("no snapshot in {heard:?}"));
    assert!(snap.to_string().contains("\"box\""), "{snap}");
    drop(screen);
    // the core lets go of it: the sender is dropped with the client
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            _ if std::time::Instant::now() > deadline => panic!("the screen was never let go"),
            _ => {}
        }
    }
    phone.background(true);
    phone.background(false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A runtime like the core's, on real time, for the socket and fetch tests.
fn on_real_core<F: Future<Output = ()>>(test: impl FnOnce() -> F) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async move {
        install(Rc::new(TokioRuntime));
        test().await;
    });
}

#[test]
fn a_socket_sends_what_was_written_before_it_opened_and_reports_its_close() {
    on_real_core(|| async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // echoes binary messages, then closes with the code it was sent as text
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            while let Some(Ok(m)) = ws.next().await {
                match m {
                    Message::Binary(b) => ws.send(Message::Binary(b)).await.unwrap(),
                    Message::Text(t) => {
                        let code: u16 = t.parse().unwrap();
                        ws.send(Message::Close(Some(CloseFrame { code: code.into(), reason: "bye".into() }))).await.unwrap();
                    }
                    _ => {}
                }
            }
        });
        let events = Rc::new(RefCell::new(Vec::<String>::new()));
        let done = inyon_core::platform::rt::Deferred::new();
        let (e, d) = (events.clone(), done.clone());
        let on: OnWire = Rc::new(move |ev| match ev {
            WireEvent::Message(m) => e.borrow_mut().push(String::from_utf8(m).unwrap()),
            WireEvent::Close(code, reason) => {
                e.borrow_mut().push(format!("closed {code} {reason}"));
                d.resolve(());
            }
        });
        let wire = NativeDialer.dial(&format!("ws://127.0.0.1:{port}/link"), on).unwrap();
        wire.send(b"one");
        wire.send(b"two");
        tokio::time::timeout(Duration::from_secs(10), async {
            while events.borrow().len() < 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        wire.close(4001, "done here");
        tokio::time::timeout(Duration::from_secs(10), done.wait()).await.unwrap();
        assert_eq!(*events.borrow(), vec!["one", "two", "closed 4001 done here"]);
    });
}

#[test]
fn a_socket_that_never_opens_closes_as_a_browser_reports_it() {
    on_real_core(|| async {
        // a port nothing listens on
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let done = inyon_core::platform::rt::Deferred::new();
        let d = done.clone();
        let wire = NativeDialer.dial(
            &format!("ws://127.0.0.1:{port}/"),
            Rc::new(move |ev| {
                if let WireEvent::Close(code, _) = ev {
                    d.resolve(code);
                }
            }),
        );
        assert!(wire.is_ok());
        assert_eq!(tokio::time::timeout(Duration::from_secs(10), done.wait()).await.unwrap(), 1006);
        assert!(NativeDialer.dial("not a url", Rc::new(|_| {})).is_err());
    });
}

#[test]
fn fetch_gets_any_status_with_its_body_and_sends_the_headers() {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let seen = std::thread::spawn(move || {
        let (mut s, _) = server.accept().unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap();
        s.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 7\r\nconnection: close\r\n\r\nmissing").unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    });
    on_real_core(|| async move {
        let r = NativeFetcher.fetch(FetchRequest::get(format!("http://127.0.0.1:{port}/latest.json"), 5_000).header("x-test", "yes")).await.unwrap();
        assert_eq!((r.status, r.text().as_str()), (404, "missing"));
    });
    let request = seen.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /latest.json "), "{request}");
    assert!(request.contains("x-test: yes"), "{request}");
}
