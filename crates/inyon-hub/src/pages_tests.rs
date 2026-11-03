//! features/pages.test.ts and pages-hub.test.ts: the list file, epochs, the API, and every `/v/`
//! answer checked against fixtures/pages/responses.json over a real socket, byte for byte. The
//! tokens in fixtures/pages/tokens.json were minted by the Bun hub's code; the same session secret
//! makes this hub mint and accept them too.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use axum::http::Method;
use inyon_core::features::pages_core::{error_page, pages_key, verify_token, VerifyContext, CANT_WRITE_LIST, GIT_TIMEOUT_MS, NO_INDEX, PAGES_LIST_MAX, PRIVATE_FOLDER, TOO_WIDE};
use inyon_core::platform::store::kv;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::server::{dispatch, install_ctx, test_req, CtxInit};

const SECRET: &str = "fixture-session-secret: not a real one";
const HOST: &str = "hub.test:4777";
const PHONE_TOKEN: &str = "phone-bearer-token";

fn fixture(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pages").join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pages")
}

/// A temp folder, gone at the end of the test.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("inyon-hub-pages-{name}-{}-{}", std::process::id(), crate::http::random_hex(4)));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn set_mtime(p: &Path, at: std::time::SystemTime) {
    std::fs::OpenOptions::new().write(true).open(p).unwrap().set_modified(at).unwrap();
}

/// A site folder: the fixture site copied, or only an index.html.
fn site(t: &Tmp, name: &str, copy: bool) -> String {
    let dir = t.join(&format!("sites/{name}-{}", crate::http::random_hex(3)));
    if copy {
        copy_dir(&fixtures_dir().join("site"), &dir);
    } else {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>x</title>").unwrap();
    }
    s(&dir)
}

/// A FIFO at `p`, and a writer that opens it after two seconds and closes it at once: a hub that
/// opened it as a plain file waits that long (and then reads nothing), instead of hanging the test
/// for good. The writer is stopped when this is dropped, since a hub that never opens it leaves the
/// writer waiting.
struct Fifo(std::process::Child);

impl Fifo {
    fn at(p: &Path) -> Fifo {
        Fifo::after(p, 2)
    }
    /// The writer opens it after `secs` seconds.
    fn after(p: &Path, secs: u32) -> Fifo {
        let _ = std::fs::remove_file(p);
        assert!(std::process::Command::new("mkfifo").arg(p).status().unwrap().success());
        Fifo(std::process::Command::new("sh").args(["-c", &format!("sleep {secs}; : > \"$1\""), "sh"]).arg(p).spawn().unwrap())
    }
}

impl Drop for Fifo {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn record(slug: &str, path: &str) -> Value {
    json!({ "slug": slug, "name": slug, "path": path, "project": path, "by": "cli", "added": 1 })
}

fn write_list(file: &Path, pages: Value) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, serde_json::to_string(&json!({ "v": 1, "pages": pages })).unwrap()).unwrap();
}

fn read_list(file: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

/// The hub's records as a test hub has them: the fixture secret, a paired phone `ph1` (whose
/// bearer is PHONE_TOKEN), the fixture epochs, pages.json under `home`, and the routes.
fn hub(home: &Path, now: Rc<Cell<u64>>) {
    let t = fixture("tokens.json");
    kv::set_value("hub", &json!({ "secret": SECRET })).unwrap();
    crate::password::init();
    crate::auth::init();
    kv::set_value("phones", &json!([{ "id": "ph1", "name": "Pixel", "tokenHash": crate::http::sha256(PHONE_TOKEN), "pairedAt": 1, "lastSeen": 1 }])).unwrap();
    crate::phones::init();
    kv::set_value(EPOCHS_KEY, &t["epochs"]).unwrap();
    install(Pages::new(home.to_owned(), Rc::new(move || now.get())));
    let mut routes = crate::server::hub_routes();
    add(&mut routes);
    install_ctx(
        CtxInit {
            data_dir: home.join("data"),
            bind: "127.0.0.1".into(),
            port: 0,
            tls: false,
            allowed_hosts: Some("hub.test".into()),
            allowed_origins: None,
            hostname: "test-box".into(),
            setup_code: None,
            local_token: "local".into(),
            hub_commit: String::new(),
            core: inyon_backends::core_here(),
        },
        routes,
    );
}

fn clock(at: u64) -> Rc<Cell<u64>> {
    Rc::new(Cell::new(at))
}

fn now_ms() -> u64 {
    inyon_core::clock::now_ms()
}

async fn body_json(r: Resp) -> Value {
    let b = r.into_body().bytes().await.unwrap();
    serde_json::from_slice(&b).unwrap_or(Value::Null)
}

type Handler = fn(Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>>;

/// A handler called as its route would call it: the result, or the error's status and message.
async fn call(f: Handler, method: &str, params: &[(&str, &str)], body: Option<Value>, headers: &[(&str, &str)]) -> Result<Value, (u16, String)> {
    let mut req = test_req(method, "/api/pages", headers, &body.map(|b| b.to_string()).unwrap_or_default());
    req.params = params.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    match f(req).await {
        Ok(r) => Ok(body_json(r).await),
        Err(e) => Err((e.status.unwrap_or(500), e.message)),
    }
}

fn h_list(r: Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>> {
    Box::pin(list(r))
}
fn h_open(r: Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>> {
    Box::pin(open(r))
}
fn h_revoke(r: Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>> {
    Box::pin(revoke(r))
}
fn h_register(r: Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>> {
    Box::pin(register(r))
}
fn h_remove(r: Req) -> futures_util::future::LocalBoxFuture<'static, HubResult<Resp>> {
    Box::pin(remove(r))
}

async fn open_url(slug: &str) -> String {
    call(h_open, "POST", &[("host", "local"), ("slug", slug)], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned()
}

async fn add_page(body: Value) -> Result<Value, (u16, String)> {
    call(h_register, "POST", &[("host", "local")], Some(body), &[]).await
}

/// `/v/…` through the hub's dispatch, as the app's iframe asks for it.
async fn view_status(url: &str) -> u16 {
    dispatch(test_req("GET", url, &[("host", HOST)], "")).await.status().as_u16()
}

// ---------- the /v/ answer, against the fixture, over a socket ----------

struct Raw {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Raw {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// One request written to the socket as is (a client would resolve the dot segments itself), and
/// its answer, with repeated headers joined as the Bun test joins them.
async fn raw(addr: SocketAddr, method: &str, target: &str, headers: &[(String, String)]) -> Raw {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("connection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut all = vec![];
    sock.read_to_end(&mut all).await.unwrap();
    let end = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&all[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let mut out: Vec<(String, String)> = vec![];
    for line in lines {
        let (k, v) = line.split_once(':').unwrap();
        let (k, v) = (k.trim().to_lowercase(), v.trim().to_owned());
        match out.iter_mut().find(|(n, _)| *n == k) {
            Some((_, old)) => *old = format!("{old}, {v}"),
            None => out.push((k, v)),
        }
    }
    let mut body = all[end + 4..].to_vec();
    if out.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked") {
        let mut plain = vec![];
        let mut i = 0;
        loop {
            let eol = i + body[i..].windows(2).position(|w| w == b"\r\n").unwrap();
            let size = usize::from_str_radix(std::str::from_utf8(&body[i..eol]).unwrap().trim(), 16).unwrap();
            if size == 0 {
                break;
            }
            plain.extend_from_slice(&body[eol + 2..eol + 2 + size]);
            i = eol + 2 + size + 2;
        }
        body = plain;
    }
    Raw { status, headers: out, body }
}

/// responses.json's `setup` under `dir`. The site's folder.
fn lay_out(dir: &Path, setup: &Value) -> PathBuf {
    let site = dir.join("site");
    copy_dir(&fixtures_dir().join(setup["copy"].as_str().unwrap()), &site);
    std::fs::create_dir_all(dir.join("outside")).unwrap();
    for (name, text) in setup["outside"].as_object().unwrap() {
        std::fs::write(dir.join("outside").join(name), text.as_str().unwrap()).unwrap();
    }
    for (name, target) in setup["symlinks"].as_object().unwrap() {
        std::os::unix::fs::symlink(target.as_str().unwrap(), site.join(name)).unwrap();
    }
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(setup["mtimeMs"].as_u64().unwrap());
    fn touch(p: &Path, at: std::time::SystemTime) {
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            let ty = e.file_type().unwrap();
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                touch(&e.path(), at);
            } else {
                set_mtime(&e.path(), at);
            }
        }
    }
    touch(dir, at);
    site
}

/// The hub listening on a free port, as start.rs serves it.
async fn listen() -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    crate::server::serve(l, None, crate::server::ctx().core.clone());
    addr
}

#[test]
fn every_view_request_in_responses_json_gets_exactly_its_status_headers_and_body() {
    let r = fixture("responses.json");
    let t = Tmp::new("responses");
    let site_dir = lay_out(&t.join("fixture"), &r["setup"]);
    let home = t.join("home");
    write_list(&pages_file(&home), json!([record(r["context"]["site"]["s"].as_str().unwrap(), &s(&site_dir))]));
    crate::test_util::block(async {
        hub(&home, clock(r["context"]["now"].as_u64().unwrap()));
        let addr = listen().await;
        let ignored: Vec<&str> = r["ignoredHeaders"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
        for c in r["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let q = &c["request"];
            let headers: Vec<(String, String)> = q["headers"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect();
            let res = raw(addr, q["method"].as_str().unwrap(), q["target"].as_str().unwrap(), &headers).await;
            let want = &c["response"];
            assert_eq!(u64::from(res.status), want["status"].as_u64().unwrap(), "{name}");
            let got: serde_json::Map<String, Value> = res.headers.iter().filter(|(k, _)| !ignored.contains(&k.as_str())).map(|(k, v)| (k.clone(), json!(v))).collect();
            assert_eq!(Value::Object(got), want["headers"], "{name}");
            let body: Vec<u8> = match &want["body"] {
                Value::Null => vec![],
                b if b["file"].is_string() => std::fs::read(fixtures_dir().join(b["file"].as_str().unwrap())).unwrap(),
                b => r["errorPages"][b["page"].to_string()].as_str().unwrap().as_bytes().to_vec(),
            };
            assert_eq!(String::from_utf8_lossy(&res.body), String::from_utf8_lossy(&body), "{name}");
        }

        // a Range request still gets the whole file with a 200, and nothing sets a cookie
        let css = r["cases"].as_array().unwrap().iter().find(|c| c["name"] == "stylesheet").unwrap();
        let target = css["request"]["target"].as_str().unwrap();
        let res = raw(addr, "GET", target, &[("host".into(), HOST.into()), ("range".into(), "bytes=0-3".into()), ("cookie".into(), "inyon_session=x.y".into())]).await;
        assert_eq!(res.status, 200);
        assert_eq!(res.header("content-range"), None);
        assert_eq!(res.header("set-cookie"), None);
        assert_eq!(res.body, std::fs::read(fixtures_dir().join("site/style.css")).unwrap());
        // HEAD names the size and sends nothing
        let res = raw(addr, "HEAD", target, &[("host".into(), HOST.into())]).await;
        assert_eq!(
            (res.status, res.header("content-length"), res.body.len()),
            (200, Some(std::fs::metadata(fixtures_dir().join("site/style.css")).unwrap().len().to_string().as_str()), 0)
        );
    });
}

#[test]
fn routes_match_the_path_as_it_came_so_a_dot_segment_into_api_is_no_route_as_in_bun() {
    let t = Tmp::new("dots");
    let home = t.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let addr = listen().await;
        let cookie = crate::auth::issue_cookie(false).split(';').next().unwrap().to_owned();
        let h = |extra: &[(&str, &str)]| -> Vec<(String, String)> { [("host", HOST)].iter().chain(extra).map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect() };
        // Bun's router matches the raw path, and its fetch fallback answers /api with a 404
        for target in ["/api/nothing/../pages", "/api/%2e%2E/api/pages", "/api/./pages"] {
            let res = raw(addr, "GET", target, &h(&[("cookie", &cookie)])).await;
            assert_eq!((res.status, String::from_utf8_lossy(&res.body).into_owned()), (404, r#"{"error":"Not found"}"#.to_owned()), "{target}");
        }
        let res = raw(addr, "GET", "/v/x/../../api/pages", &h(&[])).await;
        assert_eq!(res.status, 404);
        let res = raw(addr, "GET", "/api/pages", &h(&[("cookie", &cookie)])).await;
        assert_eq!(res.status, 200);
    });
}

/// The disk as it is, except that the last look decide_view takes at `watch` swaps `folder` for a
/// symlink to `to` right after, as an agent renaming its own folders in a loop would.
struct SwapFs {
    watch: PathBuf,
    folder: PathBuf,
    to: PathBuf,
    swapped: Cell<bool>,
}

impl ViewFs for SwapFs {
    fn stat<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<FileStat>> {
        Box::pin(async move {
            let st = DiskFs.stat(p).await;
            if !self.swapped.get() && p == self.watch {
                self.swapped.set(true);
                let mut aside = self.folder.as_os_str().to_owned();
                aside.push("-real");
                std::fs::rename(&self.folder, PathBuf::from(aside)).unwrap();
                std::os::unix::fs::symlink(&self.to, &self.folder).unwrap();
            }
            st
        })
    }
    fn realpath<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<PathBuf>> {
        DiskFs.realpath(p)
    }
}

#[test]
fn a_folder_swapped_for_a_symlink_after_the_check_is_not_followed_out_of_the_site() {
    let tmp = Tmp::new("swap");
    let home = tmp.join("home");
    let dir = PathBuf::from(site(&tmp, "swap", true));
    let outside = tmp.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("index.html"), "SECRET KEY").unwrap();
    write_list(&pages_file(&home), json!([record("swap", &s(&dir))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let real = std::fs::canonicalize(&dir).unwrap();
        let swap = Rc::new(SwapFs { watch: real.join("sub").join("index.html"), folder: dir.join("sub"), to: outside.clone(), swapped: Cell::new(false) });
        let mut p = Pages::new(home.clone(), Rc::new(now_ms));
        p.fs = swap.clone();
        install(p);
        let addr = listen().await;
        let url = open_url("swap").await;
        let res = raw(addr, "GET", &format!("{url}sub/index.html"), &[("host".into(), HOST.into())]).await;
        assert!(swap.swapped.get());
        assert_eq!(res.status, 404);
        assert!(!String::from_utf8_lossy(&res.body).contains("SECRET"));
        // with nothing swapped the same file is served
        let res = raw(addr, "GET", &format!("{url}style.css"), &[("host".into(), HOST.into())]).await;
        assert_eq!(res.status, 200);
    });
}

#[test]
fn a_file_swapped_for_a_fifo_after_the_check_is_a_404_at_once_not_an_open_that_waits_for_a_writer() {
    let tmp = Tmp::new("fifo");
    let home = tmp.join("home");
    let dir = PathBuf::from(site(&tmp, "fifo", true));
    write_list(&pages_file(&home), json!([record("fifo", &s(&dir))]));
    /// decide_view's last look at the file swaps it for a FIFO right after
    struct FifoFs(PathBuf, Cell<bool>);
    impl ViewFs for FifoFs {
        fn stat<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<FileStat>> {
            Box::pin(async move {
                let st = DiskFs.stat(p).await;
                if !self.1.get() && p == self.0 {
                    self.1.set(true);
                    std::fs::remove_file(&self.0).unwrap();
                    assert!(std::process::Command::new("mkfifo").arg(&self.0).status().unwrap().success());
                }
                st
            })
        }
        fn realpath<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<PathBuf>> {
            DiskFs.realpath(p)
        }
    }
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let fifo = Rc::new(FifoFs(std::fs::canonicalize(&dir).unwrap().join("style.css"), Cell::new(false)));
        let mut p = Pages::new(home.clone(), Rc::new(now_ms));
        p.fs = fifo.clone();
        install(p);
        let addr = listen().await;
        let url = open_url("fifo").await;
        let (target, headers) = (format!("{url}style.css"), [("host".to_string(), HOST.to_string())]);
        let get = || raw(addr, "GET", &target, &headers);
        let res = tokio::time::timeout(std::time::Duration::from_secs(3), get()).await.expect("the open waited for a writer");
        assert!(fifo.1.get());
        assert_eq!(res.status, 404);
        assert_eq!(String::from_utf8_lossy(&res.body), error_page(404));
        // and a FIFO there from the start is a 404 too
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(3), get()).await.unwrap().status, 404);
    });
}

#[test]
fn a_file_replaced_after_the_check_is_sent_whole_with_the_length_of_the_file_opened() {
    let tmp = Tmp::new("replace");
    let home = tmp.join("home");
    let dir = PathBuf::from(site(&tmp, "replace", false));
    std::fs::write(dir.join("progress.json"), r#"{"a":1}"#).unwrap();
    write_list(&pages_file(&home), json!([record("replace", &s(&dir))]));
    /// progress.py's atomic replace, right after decide_view's last look at the file
    struct ReplaceFs(PathBuf, Cell<bool>);
    impl ViewFs for ReplaceFs {
        fn stat<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<FileStat>> {
            Box::pin(async move {
                let st = DiskFs.stat(p).await;
                if !self.1.get() && p == self.0 {
                    self.1.set(true);
                    let tmp = self.0.with_extension("tmp");
                    std::fs::write(&tmp, r#"{"a":1,"b":"a longer file than before"}"#).unwrap();
                    std::fs::rename(&tmp, &self.0).unwrap();
                }
                st
            })
        }
        fn realpath<'a>(&'a self, p: &'a Path) -> futures_util::future::LocalBoxFuture<'a, Option<PathBuf>> {
            DiskFs.realpath(p)
        }
    }
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let mut p = Pages::new(home.clone(), Rc::new(now_ms));
        p.fs = Rc::new(ReplaceFs(std::fs::canonicalize(&dir).unwrap().join("progress.json"), Cell::new(false)));
        install(p);
        let addr = listen().await;
        let url = open_url("replace").await;
        let res = raw(addr, "GET", &format!("{url}progress.json"), &[("host".into(), HOST.into())]).await;
        assert_eq!(res.status, 200);
        let body = String::from_utf8_lossy(&res.body).into_owned();
        assert_eq!(body, r#"{"a":1,"b":"a longer file than before"}"#);
        assert_eq!(res.header("content-length"), Some(body.len().to_string().as_str()));
    });
}

// ---------- tokens ----------

#[test]
fn the_hub_mints_the_tokens_in_tokens_json_phone_bound_when_a_phone_asks() {
    let t = fixture("tokens.json");
    let tmp = Tmp::new("mint");
    let home = tmp.join("home");
    let folder = site(&tmp, "mint", false);
    write_list(&pages_file(&home), json!([record("fixture", &folder)]));
    crate::test_util::block(async {
        let now = clock(0);
        hub(&home, now.clone());
        assert_eq!(crate::http::hex(&pages_key(&crate::password::session_secret())), t["key"].as_str().unwrap());
        let mut n = 0;
        for m in t["mint"].as_array().unwrap().iter().filter(|m| m["site"]["h"] == "local" && m["site"]["s"] == "fixture") {
            now.set(m["now"].as_u64().unwrap());
            let bearer = format!("Bearer {PHONE_TOKEN}");
            let headers: Vec<(&str, &str)> = if m["site"]["p"].is_string() { vec![("authorization", bearer.as_str())] } else { vec![] };
            assert_eq!(m["site"]["p"].as_str().unwrap_or("ph1"), "ph1");
            let opened = call(h_open, "POST", &[("host", "local"), ("slug", "fixture")], None, &headers).await.unwrap();
            assert_eq!(opened, json!({ "url": format!("/v/{}/", m["token"].as_str().unwrap()), "expires": m["expires"], "ttl": 43_200_000 }));
            n += 1;
        }
        assert_eq!(n, 2);
    });
}

#[test]
fn every_verify_vector_with_the_epochs_read_from_the_hub_data_folder() {
    let t = fixture("tokens.json");
    crate::test_util::block(async {
        let mut epochs = t["epochs"].clone();
        epochs["local/junk"] = json!(-1);
        epochs["local/frac"] = json!(1.5);
        kv::set_value(EPOCHS_KEY, &epochs).unwrap();
        let e = Epochs::load();
        assert_eq!((e.of("local", "junk"), e.of("local", "frac"), e.of("local", "fixture")), (0, 0, 3));
        let phones: Vec<&str> = t["phones"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        let key = pages_key(t["secret"].as_str().unwrap());
        let epoch_of = |h: &str, s: &str| e.of(h, s);
        let phone_paired = |id: &str| phones.contains(&id);
        let ctx = VerifyContext { key: &key, now: t["now"].as_u64().unwrap(), epoch_of: &epoch_of, phone_paired: &phone_paired };
        for c in t["verify"].as_array().unwrap() {
            let got = verify_token(c["token"].as_str().unwrap(), &ctx).map_or_else(|v| v.as_str(), |_| "ok");
            assert_eq!(got, c["verdict"].as_str().unwrap(), "{}", c["name"]);
        }
    });
}

#[test]
fn revoking_moves_the_epoch_on_and_survives_a_restart_links_made_before_stop_working() {
    let tmp = Tmp::new("revoke");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("plan", &site(&tmp, "plan", true))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let before = open_url("plan").await;
        assert_eq!(view_status(&before).await, 200);
        assert_eq!(call(h_revoke, "POST", &[("host", "local"), ("slug", "plan")], None, &[]).await, Ok(json!({ "ok": true })));
        assert_eq!(kv::get_value(EPOCHS_KEY).unwrap()["local/plan"], json!(1));
        assert_eq!(Epochs::load().of("local", "plan"), 1);
        assert_eq!(view_status(&before).await, 403);
        let after = open_url("plan").await;
        assert_eq!(view_status(&after).await, 200);
        assert_eq!(call(h_revoke, "POST", &[("host", "local"), ("slug", "nope")], None, &[]).await.unwrap_err().0, 404);
        assert_eq!(call(h_open, "POST", &[("host", "laptop"), ("slug", "plan")], None, &[]).await.unwrap_err().0, 404);
    });
}

/// The hub's records in memory, with writes to one key failing once `fail` is set (a full disk).
struct FailingStore {
    inner: inyon_core::platform::store::MemoryStore,
    key: &'static str,
    fail: Cell<bool>,
}

impl inyon_core::platform::store::Store for FailingStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        if self.fail.get() && key == self.key {
            return Err("No space left on device".into());
        }
        self.inner.set(key, value)
    }
    fn del(&self, key: &str) {
        self.inner.del(key)
    }
    fn keys(&self, prefix: &str) -> Vec<String> {
        self.inner.keys(prefix)
    }
}

#[test]
fn a_revoke_that_cannot_be_saved_says_so_and_still_ends_the_links_until_a_restart() {
    let tmp = Tmp::new("revoke-full");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("plan", &site(&tmp, "plan", true))]));
    let store = Rc::new(FailingStore { inner: Default::default(), key: EPOCHS_KEY, fail: Cell::new(false) });
    inyon_core::platform::store::use_store(store.clone());
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let before = open_url("plan").await;
        store.fail.set(true);
        assert_eq!(call(h_revoke, "POST", &[("host", "local"), ("slug", "plan")], None, &[]).await, Err((500, NOT_SAVED.to_owned())));
        assert_eq!(view_status(&before).await, 403);
    });
}

#[test]
fn a_list_that_cannot_be_saved_is_still_served() {
    let tmp = Tmp::new("listed-full");
    let home = tmp.join("home");
    let store = Rc::new(FailingStore { inner: Default::default(), key: LISTED_KEY, fail: Cell::new(false) });
    inyon_core::platform::store::use_store(store.clone());
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        store.fail.set(true);
        write_list(&pages_file(&home), json!([record("plan", &site(&tmp, "plan", true))]));
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"].as_array().unwrap().len(), 1);
        assert_eq!(view_status(&open_url("plan").await).await, 200);
    });
}

#[test]
fn a_new_password_ends_every_link() {
    let tmp = Tmp::new("password");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("plan", &site(&tmp, "plan", true))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let url = open_url("plan").await;
        assert_eq!(view_status(&url).await, 200);
        crate::password::set_password("a brand new password").unwrap();
        assert_eq!(view_status(&url).await, 403);
    });
}

#[test]
fn a_phones_link_is_bound_to_it_and_stops_when_the_phone_is_forgotten() {
    let tmp = Tmp::new("phone");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("plan", &site(&tmp, "plan", true))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let bearer = format!("Bearer {PHONE_TOKEN}");
        let opened = call(h_open, "POST", &[("host", "local"), ("slug", "plan")], None, &[("authorization", &bearer)]).await.unwrap();
        let url = opened["url"].as_str().unwrap().to_owned();
        let payload = url.split('/').nth(2).unwrap().split('.').nth(1).unwrap().to_owned();
        use base64::Engine;
        let claims: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(claims["p"], "ph1");
        assert_eq!(view_status(&url).await, 200);
        assert!(crate::phones::revoke_phone("ph1"));
        assert_eq!(view_status(&url).await, 403);
    });
}

// ---------- the list ----------

#[test]
fn the_list_has_a_progress_summary_and_is_read_again_when_the_file_changes() {
    let tmp = Tmp::new("list");
    let home = tmp.join("home");
    let a = site(&tmp, "a", true);
    let file = pages_file(&home);
    write_list(&file, json!([{ "slug": "a", "name": "Plan A", "path": a, "project": "/p", "by": "claude", "added": 5 }]));
    crate::test_util::block(async {
        hub(&home, clock(1_792_320_000_000));
        let progress = json!({ "done": 1, "total": 3, "now": "Writing the sub page", "updated": "2026-10-19T12:00+00:00" });
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(got, json!({ "pages": [{ "host": "local", "slug": "a", "name": "Plan A", "project": "/p", "by": "claude", "added": 5, "folder": a, "progress": progress }] }));
        let b = site(&tmp, "b", false);
        write_list(&file, json!([record("a", &a), record("b", &b)]));
        set_mtime(&file, std::time::SystemTime::now() + std::time::Duration::from_secs(5));
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        let rows: Vec<(Value, Value)> = got["pages"].as_array().unwrap().iter().map(|p| (p["slug"].clone(), p.get("progress").cloned().unwrap_or(Value::Null))).collect();
        assert_eq!(rows, vec![(json!("a"), progress), (json!("b"), Value::Null)]);
    });
}

#[test]
fn a_page_carries_its_session_and_the_name_the_session_has_now() {
    let tmp = Tmp::new("session");
    let home = tmp.join("home");
    let a = site(&tmp, "a", false);
    let b = site(&tmp, "b", false);
    let c = site(&tmp, "c", false);
    let file = pages_file(&home);
    write_list(
        &file,
        json!([
            { "slug": "a", "name": "a", "path": a, "project": "/p", "by": "claude", "added": 1, "session": "chat-1" },
            { "slug": "b", "name": "b", "path": b, "project": "/p", "by": "claude", "added": 1, "session": "not a session!" },
            record("c", &c),
        ]),
    );
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let sessions =
            |got: Value| got["pages"].as_array().unwrap().iter().map(|p| (p["slug"].clone(), p.get("session").cloned(), p.get("sessionName").cloned())).collect::<Vec<_>>();
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(sessions(got), vec![(json!("a"), Some(json!("chat-1")), None), (json!("b"), None, None), (json!("c"), None, None)]);
        // renamed in Inyon: the list follows at once
        inyon_core::hosts::names::set_session_name(LOCAL_HOST, "chat-1", "Shop checkout", None, None);
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(sessions(got)[0], (json!("a"), Some(json!("chat-1")), Some(json!("Shop checkout"))));
        // a page the hub found under a session's folder carries that session too
        kv::set_value("page-sessions", &json!({ "local/c": "chat-2" })).unwrap();
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(sessions(got)[2], (json!("c"), Some(json!("chat-2")), None));
    });
}

#[test]
fn progress_is_read_at_most_every_thirty_seconds() {
    let tmp = Tmp::new("progress");
    let home = tmp.join("home");
    let a = site(&tmp, "a", true);
    write_list(&pages_file(&home), json!([record("a", &a)]));
    crate::test_util::block(async {
        let now = clock(1_792_320_000_000);
        hub(&home, now.clone());
        let done = || async { call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"][0]["progress"]["done"].clone() };
        assert_eq!(done().await, 1);
        std::fs::write(Path::new(&a).join("progress.json"), r#"{"releases":[{"tasks":[{"status":"done"},{"status":"done"}]}]}"#).unwrap();
        now.set(now.get() + 29_000);
        assert_eq!(done().await, 1);
        now.set(now.get() + 1_000);
        assert_eq!(done().await, 2);
    });
}

#[test]
fn a_fifo_as_progress_json_is_skipped_at_once_not_read_until_something_writes_to_it() {
    let tmp = Tmp::new("fifo-progress");
    let home = tmp.join("home");
    let a = site(&tmp, "a", true);
    let _writer = Fifo::at(&Path::new(&a).join("progress.json"));
    write_list(&pages_file(&home), json!([record("a", &a)]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let at = std::time::Instant::now();
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert!(at.elapsed() < std::time::Duration::from_secs(1), "took {:?}", at.elapsed());
        assert_eq!(got["pages"][0]["slug"], "a");
        assert_eq!(got["pages"][0].get("progress"), None);
    });
}

#[test]
fn a_fifo_as_the_list_file_itself_is_no_list_at_once() {
    let tmp = Tmp::new("fifo-list");
    let home = tmp.join("home");
    let file = pages_file(&home);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let _writer = Fifo::at(&file);
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let at = std::time::Instant::now();
        assert_eq!(call(h_list, "GET", &[], None, &[]).await, Ok(json!({ "pages": [] })));
        assert!(at.elapsed() < std::time::Duration::from_secs(1), "took {:?}", at.elapsed());
    });
}

#[test]
fn a_list_bigger_than_the_cap_is_a_broken_list_at_once_a_sparse_one_claiming_terabytes_too() {
    let tmp = Tmp::new("big-list");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let a = site(&tmp, "a", false);
    let x = site(&tmp, "x", false);
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        for size in [4u64 << 40, PAGES_LIST_MAX + 1] {
            write_list(&file, json!([record("a", &a)]));
            std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_len(size).unwrap();
            let at = std::time::Instant::now();
            assert_eq!(call(h_list, "GET", &[], None, &[]).await, Ok(json!({ "pages": [] })), "{size}");
            assert!(at.elapsed() < std::time::Duration::from_secs(1), "took {:?}", at.elapsed());
            assert!(pages().registry.broken());
            assert_eq!(add_page(json!({ "path": x })).await, Err((500, BROKEN_FILE.into())));
            assert_eq!(call(h_remove, "DELETE", &[("host", "local"), ("slug", "a")], None, &[]).await, Err((500, BROKEN_FILE.into())));
            assert_eq!(std::fs::metadata(&file).unwrap().len(), size);
        }
        // one at the cap is still read
        let text = serde_json::to_string(&json!({ "v": 1, "pages": [record("a", &a)] })).unwrap();
        std::fs::write(&file, format!("{text}{}", " ".repeat(PAGES_LIST_MAX as usize - text.len()))).unwrap();
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"][0]["slug"], "a");
    });
}

#[test]
fn a_symlinked_list_a_dotfile_setup_is_read_and_written_through_the_link_stays_its_file_changes() {
    let tmp = Tmp::new("linked-list");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let a = site(&tmp, "a", false);
    let b = site(&tmp, "b", false);
    let dotfiles = tmp.join("dotfiles");
    write_list(&dotfiles.join("pages.json"), json!([record("a", &a)]));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    // a relative link, as dotfile managers make them
    std::os::unix::fs::symlink("../../dotfiles/pages.json", &file).unwrap();
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"][0]["slug"], "a");
        assert_eq!(add_page(json!({ "path": b })).await.unwrap()["folder"], json!(b));
        assert!(std::fs::symlink_metadata(&file).unwrap().file_type().is_symlink());
        let paths = |v: Value| v["pages"].as_array().unwrap().iter().map(|p| p["path"].clone()).collect::<Vec<_>>();
        assert_eq!(paths(read_list(&dotfiles.join("pages.json"))), vec![json!(a), json!(b)]);
        call(h_remove, "DELETE", &[("host", "local"), ("slug", "a")], None, &[]).await.unwrap();
        assert!(std::fs::symlink_metadata(&file).unwrap().file_type().is_symlink());
        assert_eq!(paths(read_list(&dotfiles.join("pages.json"))), vec![json!(b)]);
        assert_eq!(std::fs::read_dir(&dotfiles).unwrap().count(), 1);
    });
}

#[test]
fn a_named_pipe_put_in_place_of_the_lock_is_not_waited_on_when_it_is_released() {
    let tmp = Tmp::new("fifo-lock");
    let lock = tmp.join("pages.json.lock");
    crate::test_util::block(async {
        let at = std::time::Instant::now();
        let mut writer = None;
        with_lock(&lock, 3000, || {
            writer = Some(Fifo::at(&lock));
            Ok(())
        })
        .await
        .unwrap();
        assert!(at.elapsed() < std::time::Duration::from_secs(1), "took {:?}", at.elapsed());
        // it isn't this writer's lock, so it stays
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(&std::fs::symlink_metadata(&lock).unwrap().file_type()));
        drop(writer);
    });
}

#[test]
fn only_entries_that_may_be_served_come_out() {
    let tmp = Tmp::new("filter");
    let home = tmp.join("home");
    let good = site(&tmp, "good", false);
    std::fs::create_dir_all(home.join(".ssh/site")).unwrap();
    let link = tmp.join("home-link");
    std::os::unix::fs::symlink(&home, &link).unwrap();
    write_list(
        &pages_file(&home),
        json!([
            record("good", &good),
            record("good", &site(&tmp, "second", false)),
            record("Bad", &good),
            record("home", &s(&home)),
            record("ssh", &s(&home.join(".ssh/site"))),
            record("root", "/"),
            record("relative", "some/where"),
            record("via-link", &s(&link)),
            { "slug": "partial", "path": good },
            "junk"
        ]),
    );
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let got = call(h_list, "GET", &[], None, &[]).await.unwrap();
        let rows: Vec<(Value, Value)> = got["pages"].as_array().unwrap().iter().map(|p| (p["slug"].clone(), p["folder"].clone())).collect();
        assert_eq!(rows, vec![(json!("good"), json!(good))]);
        assert_eq!(call(h_open, "POST", &[("host", "local"), ("slug", "home")], None, &[]).await.unwrap_err().0, 404);
    });
}

#[test]
fn a_missing_file_is_an_empty_list_and_one_that_is_not_a_list_is_refused_for_writes_and_left_alone() {
    let tmp = Tmp::new("broken");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let x = site(&tmp, "x", false);
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        assert_eq!(call(h_list, "GET", &[], None, &[]).await, Ok(json!({ "pages": [] })));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"v":1,"pages":["#).unwrap();
        assert_eq!(call(h_list, "GET", &[], None, &[]).await, Ok(json!({ "pages": [] })));
        assert!(pages().registry.broken());
        assert_eq!(add_page(json!({ "path": x })).await, Err((500, BROKEN_FILE.into())));
        assert_eq!(call(h_remove, "DELETE", &[("host", "local"), ("slug", "x")], None, &[]).await, Err((500, BROKEN_FILE.into())));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"v":1,"pages":["#);
    });
}

// ---------- adding and removing ----------

#[test]
fn adding_takes_the_name_and_slug_from_the_marker_the_project_from_git_and_writes_the_file() {
    let tmp = Tmp::new("add");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let folder = site(&tmp, "marked", true);
    let now = 1_792_320_000_000u64;
    crate::test_util::block(async {
        hub(&home, clock(now));
        let entry = add_page(json!({ "path": format!("{folder}/") })).await.unwrap();
        let progress = json!({ "done": 1, "total": 3, "now": "Writing the sub page", "updated": "2026-10-19T12:00+00:00" });
        assert_eq!(
            entry,
            json!({ "host": "local", "slug": "fixture", "name": "Fixture site", "project": folder, "by": "app", "added": now, "folder": folder, "progress": progress })
        );
        assert_eq!(read_list(&file), json!({ "v": 1, "pages": [{ "slug": "fixture", "name": "Fixture site", "path": folder, "project": folder, "by": "app", "added": now }] }));
        // written as JSON.stringify(…, null, 2) writes it, readable only by this user
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("{\n  \"v\": 1,\n  \"pages\": [\n    {\n      \"slug\": \"fixture\""));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!lock_of(&file).exists());

        // the project is the git top level when the folder is in a repository
        let repo = tmp.join("repo");
        std::fs::create_dir_all(repo.join("private/plans")).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&repo).status().unwrap().success());
        copy_dir(&fixtures_dir().join("site"), &repo.join("private/plans/ghostex"));
        let in_repo = add_page(json!({ "path": s(&repo.join("private/plans/ghostex")), "name": "Inyon plan" })).await.unwrap();
        assert_eq!(
            (in_repo["slug"].clone(), in_repo["name"].clone(), in_repo["project"].clone()),
            (json!("fixture-2"), json!("Inyon plan"), json!(s(&std::fs::canonicalize(&repo).unwrap())))
        );
    });
}

#[test]
fn a_folder_in_a_linked_git_worktree_belongs_to_the_main_checkout_as_the_cli_and_the_skill_say() {
    let tmp = Tmp::new("worktree");
    let home = tmp.join("home");
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    let tree = repo.join(".claude/worktrees/pages");
    git(&["worktree", "add", "-q", "--detach", &s(&tree)]);
    copy_dir(&fixtures_dir().join("site"), &tree.join("private/plans/ghostex"));
    crate::test_util::block(async {
        hub(&home, clock(1));
        let entry = add_page(json!({ "path": s(&tree.join("private/plans/ghostex")) })).await.unwrap();
        assert_eq!(entry["project"], json!(s(&std::fs::canonicalize(&repo).unwrap())));
    });
}

#[test]
fn the_project_is_found_the_same_with_any_git_old_ones_that_print_unknown_options_back_too() {
    let tmp = Tmp::new("old-git");
    let repo = tmp.join("repo");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git").arg("-C").arg(&repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    let tree = repo.join(".claude/worktrees/x");
    git(&["worktree", "add", "-q", "--detach", &s(&tree)]);
    std::fs::create_dir_all(tree.join("deep")).unwrap();
    // fixtures/old-git/git is git before 2.31 (prints --path-format=absolute back, exit 0); git-2.4 knows no --git-common-dir
    let old = fixtures_dir().join("../old-git");
    crate::test_util::block(async {
        for g in ["git".to_owned(), s(&old.join("git"))] {
            assert_eq!(project_of(&s(&repo), &g).await, s(&repo), "{g}");
            assert_eq!(project_of(&s(&repo.join("sub")), &g).await, s(&repo), "{g}");
            assert_eq!(project_of(&s(&tree), &g).await, s(&repo), "{g}");
            assert_eq!(project_of(&s(&tree.join("deep")), &g).await, s(&repo), "{g}");
        }
        let g24 = s(&old.join("git-2.4"));
        assert_eq!(project_of(&s(&repo.join("sub")), &g24).await, s(&repo));
        assert_eq!(project_of(&s(&tree.join("deep")), &g24).await, s(&tree));
    });
}

#[test]
fn git_that_never_answers_a_named_pipe_as_git_head_is_stopped_after_three_seconds_and_the_hub_serves_meanwhile() {
    let tmp = Tmp::new("git-stuck");
    let home = tmp.join("home");
    let folder = site(&tmp, "stuck", false);
    std::fs::create_dir_all(Path::new(&folder).join(".git/objects")).unwrap();
    std::fs::create_dir_all(Path::new(&folder).join(".git/refs")).unwrap();
    // a writer late enough that a hub waiting on git fails the test instead of hanging it
    let _writer = Fifo::after(&Path::new(&folder).join(".git/HEAD"), 8);
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let at = std::time::Instant::now();
        let (entry, list_ms) = tokio::join!(async { (add_page(json!({ "path": folder })).await, at.elapsed()) }, async {
            inyon_core::platform::rt::sleep(200).await;
            let t = std::time::Instant::now();
            // another request is answered while git is stuck
            assert_eq!(call(h_list, "GET", &[], None, &[]).await, Ok(json!({ "pages": [] })));
            t.elapsed()
        });
        let (entry, took) = entry;
        assert_eq!(entry.unwrap()["project"], json!(folder));
        assert!(list_ms < std::time::Duration::from_millis(500), "list took {list_ms:?}");
        assert!(took > std::time::Duration::from_millis(GIT_TIMEOUT_MS - 200), "took {took:?}");
        assert!(took < std::time::Duration::from_millis(GIT_TIMEOUT_MS + 1500), "took {took:?}");
    });
}

#[test]
fn a_fifo_as_the_marker_is_skipped_at_once_the_name_and_slug_come_from_the_folder() {
    let tmp = Tmp::new("fifo-marker");
    let home = tmp.join("home");
    let folder = site(&tmp, "piped", false);
    let _writer = Fifo::at(&Path::new(&folder).join(".inyon-page.json"));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let at = std::time::Instant::now();
        let entry = add_page(json!({ "path": folder })).await.unwrap();
        assert!(at.elapsed() < std::time::Duration::from_secs(1), "took {:?}", at.elapsed());
        let base = Path::new(&folder).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!((entry["slug"].clone(), entry["name"].clone()), (json!(base), json!(base)));
    });
}

#[test]
fn without_a_marker_the_slug_comes_from_the_folder_whatever_the_name_and_is_unique() {
    let tmp = Tmp::new("slugs");
    let home = tmp.join("home");
    let twin = |parent: &str| {
        let dir = tmp.join(&format!("twins/{parent}/Handy"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>x</title>").unwrap();
        s(&dir)
    };
    let (h1, h2, named, x) = (twin("a"), twin("b"), site(&tmp, "Folder-Name", false), site(&tmp, "x", false));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let slug = |v: Result<Value, (u16, String)>| v.unwrap()["slug"].as_str().unwrap().to_owned();
        assert_eq!(slug(add_page(json!({ "path": h1, "name": "Handy redesign" })).await), "handy");
        assert_eq!(slug(add_page(json!({ "path": h2, "name": "Handy redesign" })).await), "handy-2");
        assert_eq!(slug(add_page(json!({ "path": named })).await), Path::new(&named).file_name().unwrap().to_string_lossy().to_lowercase());
        assert_eq!(slug(add_page(json!({ "path": x, "slug": "chosen" })).await), "chosen");
    });
}

#[test]
fn the_same_folder_again_is_the_same_page_and_a_new_name_renames_it() {
    let tmp = Tmp::new("again");
    let home = tmp.join("home");
    let folder = site(&tmp, "again", false);
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let first = add_page(json!({ "path": folder, "name": "One" })).await.unwrap();
        assert_eq!(add_page(json!({ "path": folder })).await.unwrap(), first);
        let renamed = add_page(json!({ "path": folder, "name": "Two" })).await.unwrap();
        assert_eq!((renamed["slug"].clone(), renamed["name"].clone(), renamed["added"].clone()), (first["slug"].clone(), json!("Two"), first["added"].clone()));
        assert_eq!(read_list(&pages_file(&home))["pages"].as_array().unwrap().len(), 1);
        assert_eq!(add_page(json!({ "path": folder, "slug": "other" })).await.unwrap_err().0, 409);
    });
}

#[test]
fn adding_refuses_bad_folders_slugs_names_and_other_machines() {
    let tmp = Tmp::new("refuse");
    let home = tmp.join("home");
    std::fs::create_dir_all(home.join(".inyon/site")).unwrap();
    std::fs::write(home.join(".inyon/site/index.html"), "x").unwrap();
    let empty = tmp.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let (s1, s2, s3) = (site(&tmp, "s", false), site(&tmp, "s", false), site(&tmp, "s", false));
    let (taken, t2, t3) = (site(&tmp, "taken", false), site(&tmp, "t2", false), site(&tmp, "t3", false));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let msg = |r: Result<Value, (u16, String)>| r.unwrap_err().1;
        assert_eq!(msg(add_page(json!({ "path": s(&home) })).await), TOO_WIDE);
        assert_eq!(msg(add_page(json!({ "path": "/" })).await), TOO_WIDE);
        assert_eq!(msg(add_page(json!({ "path": s(&home.join(".inyon/site")) })).await), PRIVATE_FOLDER);
        assert_eq!(msg(add_page(json!({ "path": s(&empty) })).await), NO_INDEX);
        assert_eq!(msg(add_page(json!({ "path": "relative" })).await), "The folder must be an absolute path");
        assert_eq!(msg(add_page(json!({})).await), "The folder must be an absolute path");
        assert!(msg(add_page(json!({ "path": s1, "slug": "Not A Slug" })).await).starts_with("A slug is"));
        assert!(msg(add_page(json!({ "path": s1, "slug": null })).await).starts_with("A slug is"));
        assert_eq!(msg(add_page(json!({ "path": s2, "name": 5 })).await), "The name must be text");
        assert_eq!(call(h_register, "POST", &[("host", "laptop")], Some(json!({ "path": s3 })), &[]).await, Err((404, NOT_CONNECTED.into())));
        assert_eq!(call(h_register, "POST", &[("host", "local")], Some(json!([1])), &[]).await, Err((400, "Invalid JSON".into())));
        let to_home = tmp.join("to-home");
        std::os::unix::fs::symlink(&home, &to_home).unwrap();
        std::fs::write(home.join("index.html"), "x").unwrap();
        assert_eq!(msg(add_page(json!({ "path": s(&to_home) })).await), TOO_WIDE);
        add_page(json!({ "path": taken, "slug": "taken" })).await.unwrap();
        assert_eq!(add_page(json!({ "path": t2, "slug": "taken" })).await.unwrap_err().0, 409);
        assert!(add_page(json!({ "path": t3 })).await.is_ok());
    });
}

#[test]
fn adding_keeps_entries_it_cannot_read_and_other_fields_of_the_file_as_they_were() {
    let tmp = Tmp::new("keep");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let unknown = json!({ "slug": "future", "path": "/x", "kind": "remote", "extra": [1] });
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&json!({ "v": 1, "pages": [unknown], "note": "kept" })).unwrap()).unwrap();
    let (k1, k2) = (site(&tmp, "k", false), site(&tmp, "k", false));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        assert_eq!(add_page(json!({ "path": k1, "slug": "future" })).await.unwrap_err().0, 409);
        add_page(json!({ "path": k2, "slug": "k" })).await.unwrap();
        let after = read_list(&file);
        assert_eq!(after["note"], "kept");
        assert_eq!(after["pages"][0], unknown);
        let slugs: Vec<&str> = after["pages"].as_array().unwrap().iter().map(|p| p["slug"].as_str().unwrap()).collect();
        assert_eq!(slugs, ["future", "k"]);
        // and in the order the file had its fields
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("{\n  \"v\": 1,\n  \"pages\": ["));
    });
}

#[test]
fn removing_drops_the_page_from_the_file_and_ends_its_links_even_if_the_slug_comes_back() {
    let tmp = Tmp::new("remove");
    let home = tmp.join("home");
    let folder = site(&tmp, "gone", true);
    write_list(&pages_file(&home), json!([record("gone", &folder), record("kept", &site(&tmp, "kept", false))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let url = open_url("gone").await;
        assert_eq!(call(h_remove, "DELETE", &[("host", "local"), ("slug", "gone")], None, &[]).await, Ok(json!({ "ok": true })));
        let slugs: Vec<Value> = read_list(&pages_file(&home))["pages"].as_array().unwrap().iter().map(|p| p["slug"].clone()).collect();
        assert_eq!(slugs, vec![json!("kept")]);
        // remembered, so `inyon pages find` doesn't add it back by itself
        let real = std::fs::canonicalize(&folder).unwrap().to_string_lossy().into_owned();
        assert_eq!(read_list(&pages_file(&home))["removed"], json!([real]));
        assert_eq!(call(h_remove, "DELETE", &[("host", "local"), ("slug", "gone")], None, &[]).await.unwrap_err().0, 404);
        add_page(json!({ "path": folder, "slug": "gone" })).await.unwrap();
        // added again on purpose: forgotten
        assert_eq!(read_list(&pages_file(&home))["removed"], json!([]));
        assert_eq!(view_status(&url).await, 403);
        assert_eq!(view_status(&open_url("gone").await).await, 200);
    });
}

#[test]
fn taken_off_the_file_by_another_program_ends_its_links_too_other_changes_keep_them() {
    let tmp = Tmp::new("remove-cli");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let folder = site(&tmp, "cli", true);
    write_list(&file, json!([record("cli", &folder)]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let url = open_url("cli").await;
        assert_eq!(view_status(&url).await, 200);
        // another page added, this one renamed: same page, links still open
        let mut renamed = record("cli", &folder);
        renamed["name"] = json!("Renamed");
        write_list(&file, json!([renamed, record("other", &site(&tmp, "other", false))]));
        assert_eq!(view_status(&url).await, 200);
        // removed and added again under the same slug before the hub looked: a new page, old links end
        let mut again = record("cli", &folder);
        again["added"] = json!(2);
        write_list(&file, json!([again]));
        assert_eq!(view_status(&url).await, 403);
        assert_eq!(pages().epochs.of("local", "cli"), 1);
        assert_eq!(view_status(&open_url("cli").await).await, 200);
        // the file deleted: every page it listed is gone
        std::fs::remove_file(&file).unwrap();
        assert!(pages().registry.records().is_empty());
        assert_eq!(pages().epochs.of("local", "cli"), 2);
    });
}

#[test]
fn removed_and_added_again_while_the_hub_was_stopped_its_old_links_still_end() {
    let tmp = Tmp::new("remove-restart");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let folder = site(&tmp, "restart", true);
    write_list(&file, json!([record("plan", &folder)]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let url = open_url("plan").await;
        // the hub stops; the CLI removes the page and adds it again
        let mut again = record("plan", &folder);
        again["added"] = json!(7);
        write_list(&file, json!([again]));
        install(Pages::new(home.clone(), Rc::new(now_ms)));
        assert_eq!(view_status(&url).await, 403);
        // and a restart with nothing changed keeps links working
        let fresh = open_url("plan").await;
        install(Pages::new(home.clone(), Rc::new(now_ms)));
        assert_eq!(view_status(&fresh).await, 200);
    });
}

// ---------- the lock ----------

#[test]
fn the_lock_waits_for_a_live_writer_and_takes_over_one_left_for_more_than_ten_seconds() {
    let tmp = Tmp::new("lock");
    let lock = tmp.join("pages.json.lock");
    std::fs::write(&lock, "1\n").unwrap();
    crate::test_util::block(async {
        assert_eq!(with_lock(&lock, 100, || Ok(1)).await.unwrap_err().message, LOCKED);
        assert!(lock.exists());
        set_mtime(&lock, std::time::SystemTime::now() - std::time::Duration::from_secs(11));
        let inside = with_lock(&lock, 100, || Ok(std::fs::read_to_string(&lock).unwrap())).await.unwrap();
        let (pid, tag) = inside.trim_end().split_once(' ').unwrap();
        assert_eq!((pid, tag.len(), inside.ends_with('\n')), (std::process::id().to_string().as_str(), 16, true));
        assert!(!lock.exists());
        // nothing is left beside it from the takeover
        assert_eq!(std::fs::read_dir(tmp.join("")).unwrap().count(), 0);
        // released when the change fails, too
        assert_eq!(with_lock::<()>(&lock, 100, || Err(http(400, "boom"))).await.unwrap_err().message, "boom");
        assert!(!lock.exists());
    });
}

#[test]
fn a_lock_its_tag_could_not_be_written_to_is_taken_away_again_not_left_for_ten_seconds() {
    let tmp = Tmp::new("lock-full");
    let lock = tmp.join("pages.json.lock");
    let full = |_: &mut std::fs::File, _: &[u8]| Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
    assert_eq!(make_lock(&lock, "1 tag\n", full).unwrap_err().raw_os_error(), Some(libc::ENOSPC));
    assert!(!lock.exists());
    // the next writer gets it at once
    crate::test_util::block(async {
        assert_eq!(with_lock(&lock, 100, || Ok("in")).await.unwrap(), "in");
    });
}

#[test]
fn the_lock_is_removed_only_while_it_is_still_this_writers() {
    let tmp = Tmp::new("lock-owned");
    let lock = tmp.join("pages.json.lock");
    crate::test_util::block(async {
        // another writer took it over (it thought this one had died) and holds it now
        with_lock(&lock, 100, || {
            std::fs::write(&lock, "999 0123456789abcdef\n").unwrap();
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "999 0123456789abcdef\n");
    });
}

#[test]
fn a_stale_lock_judged_by_two_writers_leaves_the_fresh_one_in_its_place() {
    let tmp = Tmp::new("lock-race");
    let lock = tmp.join("pages.json.lock");
    std::fs::write(&lock, "1 stale\n").unwrap();
    set_mtime(&lock, std::time::SystemTime::now() - std::time::Duration::from_secs(11));
    let judged = std::fs::metadata(&lock).unwrap();
    // another writer saw it stale too, took it over and made its own before this one acts
    std::fs::remove_file(&lock).unwrap();
    std::fs::write(&lock, "2 fresh\n").unwrap();
    remove_judged(&lock, &judged).unwrap();
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), "2 fresh\n");
    assert_eq!(std::fs::read_dir(tmp.join("")).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_stale_lock_that_cannot_be_removed_is_an_error_not_a_wait() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = Tmp::new("lock-ro");
    let dir = tmp.join("ro");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("pages.json.lock");
    std::fs::write(&lock, "1\n").unwrap();
    set_mtime(&lock, std::time::SystemTime::now() - std::time::Duration::from_secs(11));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let r = crate::test_util::block(async { with_lock(&lock, 60_000, || Ok(1)).await });
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    // root ignores the folder's mode; then the takeover just works
    if unsafe { libc::geteuid() } != 0 {
        let e = r.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(500), CANT_WRITE_LIST));
    }
}

#[cfg(unix)]
#[test]
fn register_and_delete_answer_the_same_500_as_the_bun_hub_when_a_stale_lock_cannot_be_removed() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return; // root ignores the folder's mode
    }
    let tmp = Tmp::new("lock-ro-routes");
    let home = tmp.join("home");
    let file = pages_file(&home);
    let (listed, fresh) = (site(&tmp, "listed", false), site(&tmp, "new", false));
    write_list(&file, json!([record("listed", &listed)]));
    let lock = file.with_extension("json.lock");
    std::fs::write(&lock, "1\n").unwrap();
    set_mtime(&lock, std::time::SystemTime::now() - std::time::Duration::from_secs(11));
    let inyon = file.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&inyon, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (added, removed) = crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        (add_page(json!({ "path": fresh })).await, call(h_remove, "DELETE", &[("host", "local"), ("slug", "listed")], None, &[]).await)
    });
    std::fs::set_permissions(&inyon, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(added, Err((500, CANT_WRITE_LIST.into())));
    assert_eq!(removed, Err((500, CANT_WRITE_LIST.into())));
}

#[test]
fn two_writers_at_once_both_land() {
    let tmp = Tmp::new("writers");
    let home = tmp.join("home");
    let sites = [site(&tmp, "w1", false), site(&tmp, "w2", false), site(&tmp, "w3", false)];
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let all = futures_util::future::join_all(sites.iter().map(|p| add_page(json!({ "path": p })))).await;
        assert!(all.iter().all(Result::is_ok));
        assert_eq!(read_list(&pages_file(&home))["pages"].as_array().unwrap().len(), 3);
    });
}

// ---------- the hub's checks ----------

#[test]
fn cookie_api_calls_from_a_framed_page_or_another_site_are_refused_reads_too_but_not_a_bearer() {
    let tmp = Tmp::new("framed");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("fixture", &site(&tmp, "f", true))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let cookie = crate::auth::issue_cookie(false).split(';').next().unwrap().to_owned();
        let ask = |method: &'static str, path: &'static str, extra: Vec<(&'static str, String)>| {
            let mut headers: Vec<(&str, String)> = vec![("host", HOST.to_owned())];
            headers.extend(extra);
            async move {
                let refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let res = dispatch(test_req(method, path, &refs, "")).await;
                let status = res.status().as_u16();
                (status, body_json(res).await)
            }
        };
        let refused = json!({ "error": "Cross-site request refused" });
        for (m, p, extra) in [
            ("GET", "/api/pages", vec![("cookie", cookie.clone()), ("origin", "null".into())]),
            ("GET", "/api/pages", vec![("cookie", cookie.clone()), ("sec-fetch-site", "cross-site".into())]),
            ("POST", "/api/pages/local/fixture/open", vec![("cookie", cookie.clone()), ("origin", "null".into())]),
            ("POST", "/api/pages/local/fixture/revoke", vec![("cookie", cookie.clone()), ("origin", "null".into()), ("sec-fetch-site", "cross-site".into())]),
        ] {
            assert_eq!(ask(m, p, extra).await, (403, refused.clone()), "{m} {p}");
        }
        // the app's own calls, and plain tools with no browser headers, still pass
        for extra in [vec![("sec-fetch-site", "same-origin".to_owned())], vec![("sec-fetch-site", "none".into())], vec![]] {
            let mut e = extra;
            e.push(("cookie", cookie.clone()));
            assert_eq!(ask("GET", "/api/pages", e).await.0, 200);
        }
        assert_eq!(ask("GET", "/api/pages", vec![]).await.0, 401);
        // a paired phone sends a bearer from its own origin, which is cross-site
        let bearer = format!("Bearer {PHONE_TOKEN}");
        assert_eq!(ask("GET", "/api/pages", vec![("authorization", bearer.clone()), ("origin", "null".into())]).await.0, 200);
        assert_eq!(ask("GET", "/api/pages", vec![("authorization", bearer), ("origin", "http://localhost".into()), ("sec-fetch-site", "cross-site".into())]).await.0, 200);
    });
}

#[test]
fn the_phone_app_can_read_that_a_name_is_refused_and_nothing_more() {
    let tmp = Tmp::new("wrong-host");
    let home = tmp.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let refused = dispatch(test_req("GET", "/api/pages", &[("host", "evil.example"), ("origin", "http://localhost")], "")).await;
        assert_eq!(refused.status().as_u16(), 421);
        let h = refused.headers();
        assert_eq!(h["access-control-allow-origin"], "http://localhost");
        assert_eq!(h["vary"], "Origin");
        assert!(h.get("access-control-allow-headers").is_none() && h.get("access-control-allow-methods").is_none() && h.get("access-control-allow-credentials").is_none());
        // without an origin there's nothing to allow
        let plain = dispatch(test_req("GET", "/api/pages", &[("host", "evil.example")], "")).await;
        assert_eq!(plain.status().as_u16(), 421);
        assert!(plain.headers().get("access-control-allow-origin").is_none());
        // the right name answers as before: signed out, and not readable from another origin
        let right = dispatch(test_req("GET", "/api/pages", &[("host", HOST), ("origin", "http://localhost")], "")).await;
        assert_eq!(right.status().as_u16(), 401);
        assert!(right.headers().get("access-control-allow-origin").is_none());
    });
}

#[test]
fn view_refuses_a_host_the_hub_does_not_answer_to_even_with_a_signed_in_cookie() {
    let tmp = Tmp::new("host");
    let home = tmp.join("home");
    write_list(&pages_file(&home), json!([record("fixture", &site(&tmp, "f", true))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let url = open_url("fixture").await;
        let cookie = crate::auth::issue_cookie(false).split(';').next().unwrap().to_owned();
        let res = dispatch(test_req("GET", &url, &[("host", "evil.example"), ("cookie", &cookie)], "")).await;
        assert_eq!(res.status().as_u16(), 421);
        assert!(res.headers()["content-security-policy"].to_str().unwrap().starts_with("sandbox allow-scripts allow-forms allow-popups allow-downloads;"));
        assert!(res.headers().get("set-cookie").is_none());
        let b = res.into_body().bytes().await.unwrap();
        assert_eq!(String::from_utf8_lossy(&b), error_page(421));
        // and a forwarded name it doesn't know, behind a local proxy
        let res = dispatch(test_req("GET", &url, &[("host", HOST), ("x-forwarded-host", "evil.example")], "")).await;
        assert_eq!(res.status().as_u16(), 421);
        assert_eq!(view_status(&url).await, 200);
        let _ = Method::GET;
    });
}

// ---------- pages on another machine ----------

/// What a fake machine answers to the arguments of `inyon pages …`.
type Answer = Box<dyn Fn(&str) -> Value>;

/// What a fake machine prints for a streamed `inyon pages …`, through the callback it's given:
/// Some(code) ends the stream there, None keeps it open (in `open`) for more lines later.
type Streams = Box<dyn Fn(&str, &mut dyn FnMut(String)) -> Option<i32>>;

/// A machine whose `inyon pages …` answers come from `answer`, each command noted. Streams stay
/// open and print nothing unless `streams` says otherwise (they're noted as `stream <args>`).
struct Machine {
    calls: RefCell<Vec<String>>,
    answer: RefCell<Answer>,
    streams: RefCell<Option<Streams>>,
    open: RefCell<Vec<(String, inyon_core::hosts::host::OnLine, inyon_core::platform::rt::Deferred<i32>)>>,
}

impl Machine {
    fn new(answer: Answer) -> Machine {
        Machine { calls: RefCell::default(), answer: RefCell::new(answer), streams: RefCell::default(), open: RefCell::default() }
    }

    /// Prints a line on every open stream whose arguments start with `args`.
    fn print(&self, args: &str, line: &str) {
        for (_, on_line, _) in self.open.borrow_mut().iter_mut().filter(|(a, _, _)| a.starts_with(args)) {
            on_line(line.to_owned());
        }
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls.borrow().iter().filter(|c| c.starts_with(prefix)).count()
    }
}

impl inyon_core::hosts::host::HostBackend for Machine {
    fn exec(
        &self,
        _: &inyon_protocol::HostConfig,
        cmd: &str,
        _: inyon_core::hosts::host::ExecOptions,
    ) -> futures_util::future::LocalBoxFuture<'static, HubResult<inyon_core::hosts::host::ExecResult>> {
        // after whatever the connection puts before a command
        let args = cmd.split(inyon_core::features::temp::INYON_PAGES).nth(1).unwrap_or(cmd).trim().to_owned();
        self.calls.borrow_mut().push(args.clone());
        let out = (self.answer.borrow())(&args);
        Box::pin(async move { Ok(inyon_core::hosts::host::ExecResult { code: 0, stdout: format!("{out}\n"), stderr: String::new() }) })
    }
    fn stream(&self, _: &inyon_protocol::HostConfig, cmd: &str, mut on_line: inyon_core::hosts::host::OnLine, _: bool) -> inyon_core::hosts::host::StreamHandle {
        let args = cmd.split(inyon_core::features::temp::INYON_PAGES).nth(1).unwrap_or(cmd).trim().to_owned();
        self.calls.borrow_mut().push(format!("stream {args}"));
        let exited = inyon_core::platform::rt::Deferred::new();
        let ends = self.streams.borrow().as_ref().map(|f| f(&args, &mut on_line));
        match ends {
            Some(Some(code)) => exited.resolve(code),
            Some(None) => self.open.borrow_mut().push((args, on_line, exited.clone())),
            None => {}
        }
        let killed = exited.clone();
        inyon_core::hosts::host::StreamHandle::new(move || killed.resolve(-1), exited)
    }
    fn pty(&self, _: &inyon_protocol::HostConfig, _: &str, _: u16, _: u16, _: inyon_core::hosts::host::OnData) -> HubResult<inyon_core::hosts::host::PtyHandle> {
        Err(HubError::plain("no terminals here"))
    }
}

/// A Windows laptop joined to the hub, connected, its commands answered by `machine`.
fn laptop(machine: Rc<Machine>) {
    let backend: Rc<dyn inyon_core::hosts::host::HostBackend> = machine;
    inyon_core::hosts::host::set_host_backends(move |_| backend.clone());
    let state: inyon_protocol::HostState = serde_json::from_value(json!({
        "config": { "id": "laptop", "name": "laptop", "kind": "node" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 0
    }))
    .unwrap();
    let rt = Rc::new(RefCell::new(inyon_core::hosts::registry::HostRuntime::new(inyon_core::hosts::host::Host::new(state.config.clone()), state.clone())));
    rt.borrow_mut().hold_polls();
    rt.borrow_mut().probe = Some(Rc::new(RefCell::new(inyon_core::tmux::tmux::ProbeResult {
        hostname: "laptop".into(),
        home: "C:/Users/abhash".into(),
        os: "Windows".into(),
        now: 0.0,
        latency_ms: 0.0,
        tmux: inyon_core::tmux::tmux::TmuxProbe::Ok,
        integration: inyon_protocol::IntegrationState::Missing,
        sessions: vec![],
        agent_procs: Default::default(),
        hook_panes: Default::default(),
        status_panes: Default::default(),
        account: None,
        compact_panes: Default::default(),
        installed: vec![],
        signed_in: vec![],
        agy_conversations: Default::default(),
        agy_status: Default::default(),
        agy_titles: Default::default(),
        agy_integration: None,
        hooked: vec![],
        machine_kind: None,
        keys_at: Default::default(),
        unwatched: Default::default(),
        claude_parked: Default::default(),
    })));
    inyon_core::hosts::registry::registry().insert_runtime("laptop", rt);
}

fn robot(listed: bool) -> Value {
    let robot = json!({ "slug": "robot", "name": "Cardboard robot", "path": "D:\\Dev\\robot", "project": "D:\\Dev\\robot", "by": "claude", "added": 5, "progress": { "done": 2, "total": 3, "now": "Wiring" } });
    let gone = json!({ "slug": "gone", "name": "Gone", "path": "D:\\Dev\\gone", "project": "D:\\Dev\\gone", "by": "cli", "added": 6, "problem": "the folder is gone" });
    json!({ "v": 1, "pages": if listed { json!([robot, gone]) } else { json!([gone]) }, "host": "local", "machine": "laptop", "hub": false, "serves": true, "skipped": 0, "message": "" })
}

fn item(real: &str, file: bool, size: u64) -> Value {
    json!({ "real": real, "file": file, "dir": !file, "size": size, "mtime": 1_700_000_000_000.5 })
}

async fn view(url: &str, headers: &[(&str, &str)]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut h = vec![("host", HOST)];
    h.extend_from_slice(headers);
    let r = dispatch(test_req("GET", url, &h, "")).await;
    let status = r.status().as_u16();
    let headers = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned())).collect();
    (status, headers, r.into_body().bytes().await.unwrap().to_vec())
}

// a laptop joined to the hub made pages with Blueprint, and the hub's Pages never showed them
#[test]
fn a_joined_machines_pages_are_listed_and_served_through_its_own_inyon() {
    let tmp = Tmp::new("remote");
    let home = tmp.join("home");
    crate::test_util::block(async {
        let now = clock(now_ms());
        hub(&home, now.clone());
        let machine = Rc::new(Machine::new(Box::new(|_| Value::Null)));
        laptop(machine.clone());
        let answer = |f: Answer| *machine.answer.borrow_mut() = f;
        let listed = Rc::new(Cell::new(true));
        let l = listed.clone();
        let get = Rc::new(RefCell::new(json!({})));
        let g = get.clone();
        answer(Box::new(move |args| if args.starts_with("--json") { robot(l.get()) } else { g.borrow().clone() }));

        // listed with its progress; the one with a problem isn't
        let list = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(
            list["pages"],
            json!([{ "host": "laptop", "slug": "robot", "name": "Cardboard robot", "project": "D:\\Dev\\robot", "by": "claude", "added": 5, "folder": "D:\\Dev\\robot", "progress": { "done": 2, "total": 3, "now": "Wiring" } }])
        );
        // read again only once it's 15 s old (its inyon here has a `pages watch` that never prints)
        call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(machine.count("--json"), 1);

        let url = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        *get.borrow_mut() =
            json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot", false, 0), item("D:/Dev/robot/index.html", true, 5)], "data": "aGVsbG8=" });
        let (status, headers, body) = view(&url, &[]).await;
        assert_eq!((status, body.as_slice()), (200, &b"hello"[..]));
        assert!(headers.contains(&("content-type".into(), "text/html;charset=utf-8".into())), "{headers:?}");
        assert!(headers.contains(&("content-length".into(), "5".into())), "{headers:?}");
        assert_eq!(machine.calls.borrow().last().unwrap(), &format!("get --json --max {} --slash -- 'robot'", pages_remote::REMOTE_FILE_MAX));

        // a file, and the browser's copy still good: the machine is told, and the answer is a 304
        *get.borrow_mut() = json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot/js/app.js", true, 9), Value::Null] });
        let etag = "\"9-18bcfe56800\"";
        assert_eq!(view(&format!("{url}js/app.js"), &[("if-none-match", etag)]).await.0, 304);
        assert_eq!(machine.calls.borrow().last().unwrap(), &format!("get --json --max {} --etag='{etag}' -- 'robot' 'js' 'app.js'", pages_remote::REMOTE_FILE_MAX));
        // a file whose real path leads out of the site is a 404, whatever the machine sent
        *get.borrow_mut() = json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/secret.txt", true, 6), Value::Null], "data": "c2VjcmV0" });
        assert_eq!(view(&format!("{url}leak.txt"), &[]).await.0, 404);
        // too big to fetch
        *get.borrow_mut() = json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot/film.mp4", true, 1 << 30), Value::Null], "big": true });
        assert_eq!(view(&format!("{url}film.mp4"), &[]).await.0, 413);
        // a folder asked for without its slash goes to it with one, and nothing is fetched
        *get.borrow_mut() =
            json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot/mockups", false, 0), item("D:/Dev/robot/mockups/index.html", true, 3)] });
        let (status, headers, _) = view(&format!("{url}mockups"), &[]).await;
        assert_eq!(status, 301);
        assert!(headers.iter().any(|(k, v)| k == "location" && v.ends_with("/mockups/")), "{headers:?}");
        // never a dot name: the hub doesn't even ask
        let asked = machine.calls.borrow().len();
        assert_eq!(view(&format!("{url}.env"), &[]).await.0, 404);
        assert_eq!(machine.calls.borrow().len(), asked);

        // taken off the laptop's list (by its CLI): the old link stops working
        listed.set(false);
        now.set(now.get() + pages_remote::REMOTE_TTL_MS);
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"], json!([]));
        *get.borrow_mut() =
            json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot", false, 0), item("D:/Dev/robot/index.html", true, 5)], "data": "aGVsbG8=" });
        listed.set(true);
        now.set(now.get() + pages_remote::REMOTE_TTL_MS);
        assert_eq!(view(&url, &[]).await.0, 403);
        let again = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        assert_eq!(view(&again, &[]).await.0, 200);

        // removing from the app runs the laptop's own `inyon pages rm`, and ends the links
        let removed = Rc::new(Cell::new(false));
        let (l, r) = (listed.clone(), removed.clone());
        answer(Box::new(move |args| {
            if args.starts_with("rm ") {
                r.set(true);
                json!({ "v": 1, "removed": [], "message": "" })
            } else if r.get() {
                robot(false)
            } else {
                robot(l.get())
            }
        }));
        assert_eq!(call(h_remove, "DELETE", &[("host", "laptop"), ("slug", "robot")], None, &[]).await, Ok(json!({ "ok": true })));
        assert_eq!(machine.calls.borrow().last().unwrap(), "rm --json -- 'robot'");
        assert_eq!(view(&again, &[]).await.0, 403);
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"], json!([]));

        // adding a folder there runs its `inyon pages add`, which checks the folder itself
        answer(Box::new(|args| {
            if args.starts_with("add ") {
                json!({ "v": 1, "page": { "slug": "x", "name": "X", "path": "D:\\Dev\\x", "project": "D:\\Dev\\x", "by": "cli", "added": 9 } })
            } else {
                json!({ "v": 1, "ok": false, "error": "That folder has no index.html" })
            }
        }));
        let added = call(h_register, "POST", &[("host", "laptop")], Some(json!({ "path": "D:\\Dev\\x", "name": "X" })), &[]).await.unwrap();
        assert_eq!((added["host"].as_str(), added["slug"].as_str()), (Some("laptop"), Some("x")));
        assert_eq!(machine.calls.borrow().last().unwrap(), "add --json --name 'X' -- 'D:\\Dev\\x'");
        answer(Box::new(|_| json!({ "v": 1, "ok": false, "error": "That folder has no index.html" })));
        assert_eq!(call(h_register, "POST", &[("host", "laptop")], Some(json!({ "path": "D:\\Dev\\y" })), &[]).await, Err((400, "That folder has no index.html".into())));
        assert_eq!(call(h_register, "POST", &[("host", "nowhere")], Some(json!({ "path": "/x" })), &[]).await.unwrap_err().0, 404);
        // a folder not written in full would land wherever the machine's shell starts, and the
        // quoting keeps `~` from meaning home: neither is sent
        let asked = machine.calls.borrow().len();
        for rel in ["plans/site", "~/site", "./site", "D:site", "\\Dev\\x"] {
            let r = call(h_register, "POST", &[("host", "laptop")], Some(json!({ "path": rel })), &[]).await;
            assert_eq!(r, Err((400, inyon_core::features::pages_core::NOT_ABSOLUTE.into())), "{rel}");
        }
        assert_eq!(machine.calls.borrow().len(), asked);
    });
}

#[test]
fn a_remote_file_at_the_limit_still_fits_one_socket_message() {
    // the machine answers with the file as base64 inside its JSON, all in one message
    let encoded = (pages_remote::REMOTE_FILE_MAX as usize).div_ceil(3) * 4;
    assert!(encoded + 256 * 1024 < crate::ws::MAX_MESSAGE, "{encoded}");
}

#[test]
fn a_machine_whose_inyon_is_too_old_or_that_is_offline_lists_no_pages() {
    let tmp = Tmp::new("remote-old");
    let home = tmp.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        // 0.21's list: no `serves`, and no `pages get` to answer with
        let machine = Rc::new(Machine::new(Box::new(|_| {
            let mut v = robot(true);
            v.as_object_mut().unwrap().remove("serves");
            v
        })));
        laptop(machine.clone());
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"], json!([]));
        assert_eq!(call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap_err().0, 404);
        // a laptop that's away isn't asked at all
        *machine.answer.borrow_mut() = Box::new(|_| robot(true));
        inyon_core::hosts::registry::registry().get("laptop").unwrap().borrow_mut().state.status = inyon_protocol::HostStatus::Error;
        let asked = machine.calls.borrow().len();
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"], json!([]));
        assert_eq!(machine.calls.borrow().len(), asked);
    });
}

/// The robot page's list as `pages watch` prints it, its site stamped `stamp`.
fn watched(stamp: &str) -> String {
    let mut v = robot(true);
    v["pages"][0]["stamp"] = json!(stamp);
    v.as_object_mut().unwrap().remove("message");
    v.to_string()
}

/// What `pages get --lines` prints for a file of the robot page: the answer, then the file as
/// base64 lines of 48 KiB.
fn sent_lines(name: &str, bytes: &[u8]) -> Vec<String> {
    use base64::Engine;
    let answer =
        json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item(&format!("D:/Dev/robot/{name}"), true, bytes.len() as u64), Value::Null], "sends": bytes.len() });
    std::iter::once(answer.to_string()).chain(bytes.chunks(48 * 1024).map(|c| base64::engine::general_purpose::STANDARD.encode(c))).collect()
}

// the hub read every machine's list each 15 s, fetched a file again on every request, and
// refused anything over 8 MB
#[test]
fn a_watched_machine_is_not_asked_again_and_its_big_files_stream_and_are_kept_until_the_site_changes() {
    let tmp = Tmp::new("remote-watch");
    let home = tmp.join("home");
    crate::test_util::block(async {
        let now = clock(now_ms());
        hub(&home, now.clone());
        let machine = Rc::new(Machine::new(Box::new(|_| robot(true))));
        laptop(machine.clone());
        let film: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new((0..20 * 1024 * 1024).map(|i| (i % 251) as u8).collect()));
        assert!(film.borrow().len() as u64 > pages_remote::REMOTE_FILE_MAX);
        let f = film.clone();
        *machine.streams.borrow_mut() = Some(Box::new(move |args, print| {
            if args.starts_with("watch --json") {
                print(watched("one"));
                None
            } else {
                assert_eq!(args, format!("get --json --lines --max {} -- 'robot' 'film.bin'", pages_remote::STREAM_FILE_MAX));
                sent_lines("film.bin", &f.borrow()).into_iter().for_each(&mut *print);
                Some(0)
            }
        }));

        // the list comes from the watch, and stays its list however long it's been
        let list = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(list["pages"][0]["slug"], "robot");
        now.set(now.get() + 3 * pages_remote::REMOTE_TTL_MS);
        call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(machine.count("--json"), 0, "{:?}", machine.calls.borrow());
        assert_eq!(machine.count("stream watch"), 1);

        let url = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        let (status, headers, body) = view(&format!("{url}film.bin"), &[]).await;
        assert_eq!(status, 200);
        assert!(body == *film.borrow(), "{} bytes back", body.len());
        assert!(headers.contains(&("content-length".into(), film.borrow().len().to_string())), "{headers:?}");
        assert_eq!(machine.count("stream get"), 1);

        // asked again: the hub's copy, and a browser that has it gets a 304, the machine not asked
        assert_eq!(view(&format!("{url}film.bin"), &[]).await.2.len(), film.borrow().len());
        let etag = format!("\"{:x}-18bcfe56800\"", film.borrow().len());
        assert_eq!(view(&format!("{url}film.bin"), &[("if-none-match", &etag)]).await.0, 304);
        assert_eq!(machine.count("stream get"), 1);

        // something in the site changed: fetched again
        film.borrow_mut()[0] = 7;
        machine.print("watch", &watched("two"));
        assert_eq!(view(&format!("{url}film.bin"), &[]).await.2, *film.borrow());
        assert_eq!(machine.count("stream get"), 2);
        assert_eq!(machine.count("get --json"), 0, "never the old way");
    });
}

#[test]
fn a_file_that_comes_short_is_not_kept_and_one_over_the_stream_cap_is_too_big() {
    let tmp = Tmp::new("remote-short");
    let home = tmp.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let machine = Rc::new(Machine::new(Box::new(|_| robot(true))));
        laptop(machine.clone());
        *machine.streams.borrow_mut() = Some(Box::new(|args, print| {
            if args.starts_with("watch") {
                print(watched("one"));
                return None;
            }
            if args.ends_with("'huge.bin'") {
                let size = pages_remote::STREAM_FILE_MAX + 1;
                print(json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot/huge.bin", true, size), Value::Null], "big": true }).to_string());
                return Some(0);
            }
            // the file was cut short while it was read: one line of two
            let mut lines = sent_lines("log.txt", &[b'x'; 64 * 1024]);
            lines.pop();
            lines.into_iter().for_each(&mut *print);
            Some(0)
        }));
        call(h_list, "GET", &[], None, &[]).await.unwrap();
        let url = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        assert_eq!(view(&format!("{url}huge.bin"), &[]).await.0, 413);
        let short = view(&format!("{url}log.txt"), &[]).await.0;
        assert_ne!(short, 200);
        view(&format!("{url}log.txt"), &[]).await;
        assert_eq!(machine.count("stream get --json --lines --max 67108864 -- 'robot' 'log.txt'"), 2, "not kept");
    });
}

#[test]
fn a_machine_whose_inyon_has_no_watch_is_asked_the_old_way_and_tried_again_later() {
    let tmp = Tmp::new("remote-nowatch");
    let home = tmp.join("home");
    crate::test_util::block(async {
        let now = clock(now_ms());
        hub(&home, now.clone());
        let machine = Rc::new(Machine::new(Box::new(|args| {
            if args.starts_with("--json") {
                robot(true)
            } else {
                json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot", false, 0), item("D:/Dev/robot/index.html", true, 5)], "data": "aGVsbG8=" })
            }
        })));
        laptop(machine.clone());
        // 0.22.2's inyon: `pages watch` is an unknown command, which prints nothing and fails
        *machine.streams.borrow_mut() = Some(Box::new(|_, _| Some(2)));
        assert_eq!(call(h_list, "GET", &[], None, &[]).await.unwrap()["pages"][0]["slug"], "robot");
        inyon_core::platform::rt::yield_now().await;
        let url = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        assert_eq!(view(&url, &[]).await.2, b"hello");
        assert_eq!(machine.count(&format!("get --json --max {} --slash", pages_remote::REMOTE_FILE_MAX)), 1);
        now.set(now.get() + pages_remote::REMOTE_TTL_MS);
        call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(machine.count("stream watch"), 1, "not tried again at once");
        assert_eq!(machine.count("--json"), 2, "read every 15 s, as before");
        now.set(now.get() + pages_remote::NO_WATCH_MS);
        call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(machine.count("stream watch"), 2, "tried again once its inyon may have been updated");
    });
}

// the plan's research notes were only readable as raw files, or not at all on a phone
#[test]
fn a_sites_research_notes_are_listed_and_open_as_pages_in_the_sandbox() {
    let tmp = Tmp::new("notes");
    let home = tmp.join("home");
    let dir = PathBuf::from(site(&tmp, "notes", false));
    std::fs::create_dir_all(dir.join("research/deeper")).unwrap();
    std::fs::write(dir.join("research/plan.md"), "# The plan\n\n<script>alert(1)</script>\n\nSee [internals](internals.md).\n").unwrap();
    std::fs::write(dir.join("research/Internals.md"), "x").unwrap();
    for skipped in ["research/.draft.md", "research/notes.txt", "research/deeper/more.md"] {
        std::fs::write(dir.join(skipped), "x").unwrap();
    }
    write_list(&pages_file(&home), json!([record("notes", &s(&dir)), record("plain", &site(&tmp, "plain", false))]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let list = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(list["pages"][0]["notes"], json!(["Internals.md", "plan.md"]));
        assert!(list["pages"][1].get("notes").is_none(), "{list}");

        let url = open_url("notes").await;
        let (status, headers, body) = view(&format!("{url}research/plan.md?as=note&theme=dark"), &[]).await;
        let body = String::from_utf8(body).unwrap();
        assert_eq!(status, 200);
        assert!(body.contains("<h1>The plan</h1>") && body.contains("&lt;script&gt;") && body.contains("href=\"internals.md?as=note\""), "{body}");
        let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(header("content-type"), "text/html; charset=utf-8");
        assert!(header("content-security-policy").starts_with("sandbox allow-scripts"), "{headers:?}");
        assert!(header("etag").ends_with("-note\""), "{headers:?}");
        // without ?as=note it's the file as it is, and the link's checks still hold
        let (status, headers, _) = view(&format!("{url}research/plan.md"), &[]).await;
        assert_eq!(status, 200);
        assert!(headers.iter().any(|(k, v)| k == "content-type" && v.starts_with("text/plain")), "{headers:?}");
        assert_eq!(view(&format!("{url}research/.draft.md?as=note"), &[]).await.0, 404);
        assert_eq!(view("/v/not-a-token/research/plan.md?as=note", &[]).await.0, 403);
    });
}

#[test]
fn a_joined_machines_notes_are_listed_as_it_lists_them_and_rendered_by_the_hub() {
    let tmp = Tmp::new("remote-notes");
    let home = tmp.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let machine = Rc::new(Machine::new(Box::new(|_| Value::Null)));
        laptop(machine.clone());
        *machine.answer.borrow_mut() = Box::new(|args| {
            if args.starts_with("--json") {
                let mut l = robot(true);
                l["pages"][0]["notes"] = json!(["plan.md", "../escape.md", ".hidden.md", "plain.txt"]);
                l
            } else {
                json!({ "v": 1, "ok": true, "root": "D:/Dev/robot", "items": [item("D:/Dev/robot/research/plan.md", true, 7), Value::Null], "data": "IyBQbGFuCg==" })
            }
        });
        let list = call(h_list, "GET", &[], None, &[]).await.unwrap();
        assert_eq!(list["pages"][0]["notes"], json!(["plan.md"]));
        let url = call(h_open, "POST", &[("host", "laptop"), ("slug", "robot")], None, &[]).await.unwrap()["url"].as_str().unwrap().to_owned();
        let (status, _, body) = view(&format!("{url}research/plan.md?as=note"), &[]).await;
        assert_eq!(status, 200);
        assert!(String::from_utf8(body).unwrap().contains("<h1>Plan</h1>"));
    });
}

// an agent made a plan on the laptop and never added it: the hub has the laptop's inyon look
#[test]
fn the_hub_has_each_machine_find_the_sites_where_its_agents_work() {
    let tmp = Tmp::new("find");
    let home = tmp.join("home");
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        let machine = Rc::new(Machine::new(Box::new(|_| Value::Null)));
        laptop(machine.clone());
        let rt = inyon_core::hosts::registry::registry().get("laptop").unwrap();
        rt.borrow_mut().state.sessions = serde_json::from_value(json!([{ "id": "$1", "name": "s", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 0, "panes": [
            { "id": "%1", "index": 0, "active": true, "command": "claude", "cwd": "D:\\Dev\\shop", "title": "", "width": 80, "height": 24, "dead": false, "agent": { "kind": "claude", "pid": 1, "state": "idle", "sessionId": "chat-7" } }
        ] }] }]))
        .unwrap();
        let found = Rc::new(Cell::new(true));
        let f = found.clone();
        *machine.answer.borrow_mut() = Box::new(move |args| {
            if args.starts_with("find ") {
                if f.get() {
                    json!({ "v": 1, "found": [{ "slug": "plan", "path": "D:\\Dev\\shop\\private\\plans\\plan" }], "message": "" })
                } else {
                    json!({ "v": 1, "ok": false, "error": "unrecognized subcommand 'find'" })
                }
            } else {
                robot(true)
            }
        });
        let mut not_before = std::collections::HashMap::new();
        // an answer that isn't one (a timeout, a dropped connection) is tried again next time
        let answer = std::mem::replace(&mut *machine.answer.borrow_mut(), Box::new(|_| Value::Null));
        crate::finding::find_all(&mut not_before, 500).await;
        assert!(not_before.is_empty(), "{not_before:?}");
        *machine.answer.borrow_mut() = answer;
        machine.calls.borrow_mut().clear();
        crate::finding::find_all(&mut not_before, 1_000).await;
        assert_eq!(machine.calls.borrow().iter().filter(|c| c.starts_with("find ")).cloned().collect::<Vec<_>>(), ["find --json --by 'claude' -- 'D:\\Dev\\shop'"]);
        // the hub keeps which conversation's folder the page was found under
        assert_eq!(crate::finding::session_of("laptop", "plan").as_deref(), Some("chat-7"));
        // an inyon from before 0.22.4 is left alone for a while
        found.set(false);
        crate::finding::find_all(&mut not_before, 2_000).await;
        assert_eq!(not_before.get("laptop"), Some(&(2_000 + crate::finding::RETRY_MS)));
        let asked = machine.count("find ");
        crate::finding::find_all(&mut not_before, 3_000).await;
        assert_eq!(machine.count("find "), asked);
    });
}

#[test]
fn an_app_following_the_pages_stream_hears_a_page_added_and_removed_through_the_hub() {
    use inyon_protocol::ServerMsg2;
    let tmp = Tmp::new("stream");
    let home = tmp.join("home");
    let a = site(&tmp, "a", false);
    let b = site(&tmp, "b", false);
    write_list(&pages_file(&home), json!([record("a", &a)]));
    crate::test_util::block(async {
        hub(&home, clock(now_ms()));
        crate::pages_stream::start();
        let got = Rc::new(RefCell::new(vec![]));
        let g = got.clone();
        let app = crate::clients::new_client2(move |w| g.borrow_mut().push(w));
        crate::clients::add(&app);
        // small frames, and no compression asked for: all text
        let frames = || -> Vec<ServerMsg2> {
            got.borrow_mut()
                .drain(..)
                .filter_map(|w| match w {
                    inyon_api::streams::Wire::Text(t) => serde_json::from_str(&t).ok(),
                    inyon_api::streams::Wire::Binary(_) => None,
                })
                .collect()
        };
        let settle = || inyon_core::platform::rt::sleep(200);
        let slugs = |rows: &Option<inyon_protocol::Rows>| rows.as_ref().and_then(|r| r.pages.as_ref()).map(|p| p.iter().map(|p| p.slug.clone()).collect::<Vec<_>>());
        inyon_api::streams::client_message2(&app, &json!({ "t": "sub", "stream": "pages" }).to_string());
        settle().await;
        let f = frames();
        assert!(matches!(&f[..], [ServerMsg2::Snapshot(s)] if slugs(&Some(s.rows.clone())) == Some(vec!["a".into()])), "the list as GET /api/pages has it: {f:?}");

        add_page(json!({ "path": b })).await.unwrap();
        settle().await;
        let f = frames();
        // a folder added from the app is named after it ("b-…")
        assert!(matches!(&f[..], [ServerMsg2::Delta(d)] if slugs(&d.append).is_some_and(|s| s.len() == 1 && s[0].starts_with("b-"))), "{f:?}");

        call(h_remove, "DELETE", &[("host", "local"), ("slug", "a")], None, &[]).await.unwrap();
        settle().await;
        let f = frames();
        assert!(matches!(&f[..], [ServerMsg2::Delta(d)] if d.remove.as_ref().and_then(|k| k.pages.clone()) == Some(vec!["local/a".into()])), "{f:?}");
        crate::clients::close_client(&app);
    });
}
