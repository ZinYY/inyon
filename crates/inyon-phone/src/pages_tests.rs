use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use futures_util::future::LocalBoxFuture;
use inyon_api::http::test_req;
use inyon_core::features::temp::INYON_PAGES;
use inyon_core::hosts::host::{set_host_backends, ExecOptions, ExecResult, Host, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use inyon_core::hosts::http_error::HubError;
use inyon_core::hosts::registry::{registry, HostRuntime};
use inyon_core::platform::store::MemoryStore;
use inyon_core::tmux::tmux::{ProbeResult, TmuxProbe};
use inyon_protocol::{HostConfig, HostState, IntegrationState};

use super::*;
use crate::phone::tests::{body, on_phone};
use crate::phone::Phone;

/// A computer whose `inyon pages …` answers come from `answer` (by its arguments), each noted. A
/// stream prints what `lines` gives for it and ends; with no `lines` it ends at once, printing
/// nothing, as an inyon without `pages watch` does.
type Answer = Box<dyn Fn(&str) -> ExecResult>;
type Lines = Box<dyn Fn(&str) -> Option<Vec<String>>>;

struct Computer {
    calls: RefCell<Vec<String>>,
    answer: RefCell<Answer>,
    lines: RefCell<Option<Lines>>,
    /// while set, `pages get` waits for it: the computer still reading the file
    held: RefCell<Option<Deferred<()>>>,
}

fn said(v: Value) -> ExecResult {
    ExecResult { code: 0, stdout: format!("{v}\n"), stderr: String::new() }
}

impl Computer {
    fn new(answer: impl Fn(&str) -> ExecResult + 'static) -> Rc<Computer> {
        Rc::new(Computer { calls: RefCell::default(), answer: RefCell::new(Box::new(answer)), lines: RefCell::default(), held: RefCell::default() })
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls.borrow().iter().filter(|c| c.starts_with(prefix)).count()
    }

    fn last(&self) -> String {
        self.calls.borrow().last().cloned().unwrap_or_default()
    }
}

/// The arguments after `inyon pages`, whatever the connection put before them.
fn args_of(cmd: &str) -> String {
    cmd.split(INYON_PAGES).nth(1).unwrap_or(cmd).trim().to_owned()
}

impl HostBackend for Computer {
    fn exec(&self, _: &HostConfig, cmd: &str, _: ExecOptions) -> LocalBoxFuture<'static, inyon_core::hosts::http_error::HubResult<ExecResult>> {
        let args = args_of(cmd);
        self.calls.borrow_mut().push(args.clone());
        let out = (self.answer.borrow())(&args);
        let held = self.held.borrow().clone().filter(|_| args.starts_with("get "));
        Box::pin(async move {
            if let Some(d) = held {
                d.wait().await;
            }
            Ok(out)
        })
    }
    fn stream(&self, _: &HostConfig, cmd: &str, mut on_line: OnLine, _: bool) -> StreamHandle {
        let args = args_of(cmd);
        self.calls.borrow_mut().push(format!("stream {args}"));
        let exited = Deferred::new();
        match self.lines.borrow().as_ref().and_then(|f| f(&args)) {
            // a watch keeps running after its first list
            Some(lines) if args.starts_with("watch") => lines.into_iter().for_each(&mut on_line),
            Some(lines) => {
                lines.into_iter().for_each(&mut on_line);
                exited.resolve(0);
            }
            None => exited.resolve(2),
        }
        let killed = exited.clone();
        StreamHandle::new(move || killed.resolve(-1), exited)
    }
    fn pty(&self, _: &HostConfig, _: &str, _: u16, _: u16, _: OnData) -> inyon_core::hosts::http_error::HubResult<PtyHandle> {
        Err(HubError::plain("no terminals here"))
    }
}

/// A laptop paired with the phone, connected, its commands answered by `computer`.
fn laptop(computer: Rc<Computer>) {
    let backend: Rc<dyn HostBackend> = computer;
    set_host_backends(move |_| backend.clone());
    let state: HostState = serde_json::from_value(json!({
        "config": { "id": "laptop", "name": "studio", "kind": "link" }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 0
    }))
    .unwrap();
    let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state)));
    rt.borrow_mut().hold_polls();
    rt.borrow_mut().probe = Some(Rc::new(RefCell::new(ProbeResult {
        hostname: "studio".into(),
        home: "/home/sam".into(),
        os: "Linux".into(),
        now: 0.0,
        latency_ms: 0.0,
        tmux: TmuxProbe::Ok,
        integration: IntegrationState::Missing,
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
    registry().insert_runtime("laptop", rt);
}

/// `inyon pages --json` on the laptop: one Blueprint site, and one whose folder is gone.
fn listed() -> Value {
    json!({ "v": 1, "pages": [
        { "slug": "plan", "name": "Launch plan", "path": "/home/sam/app/private/plans/launch", "project": "/home/sam/app", "by": "claude", "added": 5, "session": "c-1", "progress": { "done": 2, "total": 3, "now": "Wiring" }, "notes": ["plan.md"] },
        { "slug": "gone", "name": "Gone", "path": "/home/sam/gone", "project": "/home/sam/gone", "by": "cli", "added": 6, "problem": "the folder is gone" },
    ], "host": "local", "machine": "studio", "hub": false, "serves": true, "skipped": 0, "message": "" })
}

const ROOT: &str = "/home/sam/app/private/plans/launch";

fn item(real: &str, file: bool, size: u64) -> Value {
    json!({ "real": real, "file": file, "dir": !file, "size": size, "mtime": 1_700_000_000_000.5 })
}

/// What `pages get --json` answers for a file of the site, with its bytes.
fn file(rel: &str, bytes: &[u8]) -> Value {
    use base64::Engine;
    json!({ "v": 1, "ok": true, "root": ROOT, "items": [item(&format!("{ROOT}/{rel}"), true, bytes.len() as u64), Value::Null], "data": base64::engine::general_purpose::STANDARD.encode(bytes) })
}

/// The laptop answering its list, and `pages get` with whatever `get` holds now.
fn answering(get: Rc<RefCell<Value>>) -> Rc<Computer> {
    Computer::new(move |args| if args.starts_with("--json") { said(listed()) } else { said(get.borrow().clone()) })
}

async fn get(p: &Phone, url: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let r = p.answer(test_req("GET", url, &[], "")).await;
    let status = r.status().as_u16();
    let headers = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned())).collect();
    (status, headers, r.into_body().bytes().await.unwrap().to_vec())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

async fn open(p: &Phone, slug: &str) -> String {
    let (status, v) = body(p.answer(test_req("POST", &format!("/api/pages/laptop/{slug}/open"), &[], "")).await).await;
    assert_eq!(status, 200, "{v}");
    v["url"].as_str().unwrap().to_owned()
}

// the phone app showed "Pages open through a hub" and nothing else when it had no hub
#[test]
fn a_paired_computers_pages_are_listed_and_open_without_a_hub() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let get_answer = Rc::new(RefCell::new(json!({})));
        let computer = answering(get_answer.clone());
        laptop(computer.clone());

        let (status, list) = body(p.answer(test_req("GET", "/api/pages", &[], "")).await).await;
        assert_eq!(status, 200);
        assert_eq!(
            list,
            json!({ "pages": [{ "host": "laptop", "slug": "plan", "name": "Launch plan", "project": "/home/sam/app", "by": "claude", "added": 5, "folder": ROOT,
                "progress": { "done": 2, "total": 3, "now": "Wiring" }, "notes": ["plan.md"], "session": "c-1" }], "outdated": [] })
        );

        let url = open(&p, "plan").await;
        assert!(url.starts_with("/v/v1.") && url.ends_with('/'), "{url}");
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": ROOT, "items": [item(ROOT, false, 0), item(&format!("{ROOT}/index.html"), true, 5)], "data": "aGVsbG8=" });
        let (status, headers, bytes) = get(&p, &url).await;
        assert_eq!((status, bytes.as_slice()), (200, &b"hello"[..]));
        assert_eq!(header(&headers, "content-type"), Some("text/html;charset=utf-8"));
        assert_eq!(computer.last(), format!("get --json --max {PHONE_FILE_MAX} --slash -- 'plan'"));

        // each file by its type, as the hub serves it
        for (name, ty) in [
            ("kit/app.js", "text/javascript;charset=utf-8"),
            ("kit/site.css", "text/css;charset=utf-8"),
            ("shots/home-d.webp", "image/webp"),
            ("data.bin", "application/octet-stream"),
        ] {
            *get_answer.borrow_mut() = file(name, b"x");
            let (status, headers, _) = get(&p, &format!("{url}{name}")).await;
            assert_eq!((status, header(&headers, "content-type")), (200, Some(ty)), "{name}");
        }
        // progress.json is never kept
        *get_answer.borrow_mut() = file("progress.json", b"{}");
        let (_, headers, _) = get(&p, &format!("{url}progress.json")).await;
        assert_eq!(header(&headers, "cache-control"), Some("no-store"));

        // a folder asked for without its slash goes to it with one
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": ROOT, "items": [item(&format!("{ROOT}/mockups"), false, 0), item(&format!("{ROOT}/mockups/index.html"), true, 3)], "data": "aGk=" });
        let (status, headers, _) = get(&p, &format!("{url}mockups")).await;
        assert_eq!(status, 301);
        assert_eq!(header(&headers, "location"), Some(format!("{url}mockups/").as_str()));
        // a HEAD (the viewer looking for a site's parts) carries no body
        let r = p.answer(test_req("HEAD", &format!("{url}mockups/"), &[], "")).await;
        assert_eq!(r.status().as_u16(), 200);
        assert!(r.into_body().bytes().await.unwrap().is_empty());
    });
}

#[test]
fn nothing_outside_the_pages_folder_is_ever_served() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let get_answer = Rc::new(RefCell::new(json!({})));
        let computer = answering(get_answer.clone());
        laptop(computer.clone());
        let url = open(&p, "plan").await;
        let token = url.trim_start_matches("/v/").trim_end_matches('/').to_owned();

        // dot names and climbing out: refused before the computer is asked
        let asked = computer.calls.borrow().len();
        for path in [".git/config", ".env", "kit/.secret", "a//b", "%2e%2e/%2e%2e/.ssh/id_ed25519", "x%2fy", "bad%zz"] {
            assert_eq!(get(&p, &format!("{url}{path}")).await.0, 404, "{path}");
        }
        assert_eq!(computer.calls.borrow().len(), asked);
        // `..` leaves the token behind, so what's left isn't this page's link at all, and a slash
        // written as %2f is no slash
        for (path, status) in [("../../../etc/passwd", 404), ("%2e%2e/x", 403), ("..%2f..%2fetc/passwd", 404)] {
            assert_eq!(get(&p, &format!("/v/{token}/{path}")).await.0, status, "{path}");
        }
        assert_eq!(computer.calls.borrow().len(), asked);

        // a file whose real path leads out of the site (a symlink), whatever the computer sent
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": ROOT, "items": [item("/home/sam/.ssh/id_ed25519", true, 6), Value::Null], "data": "c2VjcmV0" });
        assert_eq!(get(&p, &format!("{url}key")).await.0, 404);
        // missing
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": ROOT, "items": [Value::Null, Value::Null] });
        let (status, headers, _) = get(&p, &format!("{url}nope.html")).await;
        assert_eq!((status, header(&headers, "content-type")), (404, Some("text/html;charset=utf-8")));
        // a page the computer no longer lists
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": Value::Null, "items": [] });
        assert_eq!(get(&p, &format!("{url}index.html")).await.0, 404);
        // too big for the link: the computer sends nothing and says so
        *get_answer.borrow_mut() = json!({ "v": 1, "ok": true, "root": ROOT, "items": [item(&format!("{ROOT}/film.mp4"), true, 1 << 30), Value::Null], "big": true });
        assert_eq!(get(&p, &format!("{url}film.mp4")).await.0, 413);
        // only reading
        assert_eq!(p.answer(test_req("POST", &url, &[], "")).await.status().as_u16(), 405);

        // a made-up token, one for another page, and one after Revoke
        assert_eq!(get(&p, &format!("/v/v1.e30.{}/", "A".repeat(43))).await.0, 403);
        assert_eq!(body(p.answer(test_req("POST", "/api/pages/laptop/gone/open", &[], "")).await).await.0, 404);
        assert_eq!(body(p.answer(test_req("POST", "/api/pages/laptop/plan/revoke", &[], "")).await).await.0, 200);
        *get_answer.borrow_mut() = file("index.html", b"hi");
        assert_eq!(get(&p, &format!("{url}index.html")).await.0, 403);
        let again = open(&p, "plan").await;
        assert_eq!(get(&p, &format!("{again}index.html")).await.0, 200);
    });
}

// a page asks for its files all at once: two big ones sent together could close the computer's
// link to the phone
#[test]
fn a_computer_sends_one_file_at_a_time() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let get_answer = Rc::new(RefCell::new(file("a.js", b"a")));
        let computer = answering(get_answer);
        laptop(computer.clone());
        let p = Rc::new(p);
        let url = open(&p, "plan").await;
        let held = Deferred::new();
        *computer.held.borrow_mut() = Some(held.clone());
        let done = Rc::new(Cell::new(0));
        for name in ["a.js", "b.js", "c.js"] {
            let (p, url, done) = (p.clone(), format!("{url}{name}"), done.clone());
            tokio::task::spawn_local(async move {
                assert_eq!(get(&p, &url).await.0, 200);
                done.set(done.get() + 1);
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(computer.count("get "), 1, "{:?}", computer.calls.borrow());
        *computer.held.borrow_mut() = None;
        held.resolve(());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!((computer.count("get "), done.get()), (3, 3));
    });
}

#[test]
fn a_computer_whose_inyon_is_too_old_is_named_so_the_app_can_say_to_update_it() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        // from before `inyon pages`: clap's own words
        let computer = Computer::new(|_| ExecResult { code: 2, stdout: String::new(), stderr: "error: unrecognized subcommand 'pages'\n".into() });
        laptop(computer.clone());
        let (_, list) = body(p.answer(test_req("GET", "/api/pages", &[], "")).await).await;
        assert_eq!(list, json!({ "pages": [], "outdated": ["laptop"] }));
        assert_eq!(body(p.answer(test_req("POST", "/api/pages/laptop/plan/open", &[], "")).await).await.0, 404);
        // 0.21's list: it has pages but no `serves`, so no `pages get` either
        *computer.answer.borrow_mut() = Box::new(|_| {
            let mut v = listed();
            v.as_object_mut().unwrap().remove("serves");
            said(v)
        });
        p.pages_forget("laptop");
        let (_, list) = body(p.answer(test_req("GET", "/api/pages", &[], "")).await).await;
        assert_eq!(list, json!({ "pages": [], "outdated": ["laptop"] }));
        // updated
        *computer.answer.borrow_mut() = Box::new(|_| said(listed()));
        p.pages_forget("laptop");
        let (_, list) = body(p.answer(test_req("GET", "/api/pages", &[], "")).await).await;
        assert_eq!((list["pages"].as_array().map(Vec::len), &list["outdated"]), (Some(1), &json!([])));
    });
}

#[test]
fn a_watched_computers_files_stream_under_the_phones_limit_and_are_kept_until_the_site_changes() {
    use base64::Engine;
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let computer = Computer::new(|_| said(listed()));
        let stamp = Rc::new(RefCell::new("one".to_owned()));
        let st = stamp.clone();
        *computer.lines.borrow_mut() = Some(Box::new(move |args| {
            if args.starts_with("watch --json") {
                let mut v = listed();
                v["pages"][0]["stamp"] = json!(*st.borrow());
                return Some(vec![v.to_string()]);
            }
            assert_eq!(args, format!("get --json --lines --max {PHONE_FILE_MAX} -- 'plan' 'shots' 'home.webp'"));
            let bytes = vec![7u8; 100 * 1024];
            let answer =
                json!({ "v": 1, "ok": true, "root": ROOT, "items": [item(&format!("{ROOT}/shots/home.webp"), true, bytes.len() as u64), Value::Null], "sends": bytes.len() });
            Some(std::iter::once(answer.to_string()).chain(bytes.chunks(48 * 1024).map(|c| base64::engine::general_purpose::STANDARD.encode(c))).collect())
        }));
        laptop(computer.clone());
        let (_, list) = body(p.answer(test_req("GET", "/api/pages", &[], "")).await).await;
        assert_eq!(list["pages"][0]["slug"], "plan");
        let url = open(&p, "plan").await;
        let (status, _, bytes) = get(&p, &format!("{url}shots/home.webp")).await;
        assert_eq!((status, bytes.len()), (200, 100 * 1024));
        // asked again: the phone's copy, while the site's stamp stays
        assert_eq!(get(&p, &format!("{url}shots/home.webp")).await.0, 200);
        assert_eq!(computer.count("stream get"), 1, "{:?}", computer.calls.borrow());
    });
}

#[test]
fn a_note_opens_as_a_page_to_read() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let get_answer = Rc::new(RefCell::new(file("research/plan.md", b"# Plan\n\n<script>x</script>")));
        laptop(answering(get_answer));
        let url = open(&p, "plan").await;
        let (status, headers, bytes) = get(&p, &format!("{url}research/plan.md?as=note&theme=dark")).await;
        let html = String::from_utf8(bytes).unwrap();
        assert_eq!((status, header(&headers, "content-type")), (200, Some("text/html; charset=utf-8")));
        assert!(html.contains("<h1>Plan</h1>") && html.contains("&lt;script&gt;"), "{html}");
        // without the query it's the Markdown itself
        let (_, headers, _) = get(&p, &format!("{url}research/plan.md")).await;
        assert_eq!(header(&headers, "content-type"), Some("text/plain;charset=utf-8"));
    });
}

#[test]
fn folders_are_added_and_removed_through_the_computers_own_inyon() {
    on_phone(Rc::new(MemoryStore::new()), |p| async move {
        let computer = Computer::new(|args| {
            if args.starts_with("add ") {
                said(json!({ "v": 1, "page": { "slug": "x", "name": "X", "path": "/home/sam/x", "project": "/home/sam/x", "by": "cli", "added": 9 } }))
            } else if args.starts_with("rm ") {
                said(json!({ "v": 1, "removed": [], "message": "" }))
            } else {
                said(listed())
            }
        });
        laptop(computer.clone());
        let post = |path: &str, b: Value| test_req("POST", path, &[("content-type", "application/json")], &b.to_string());
        let (status, added) = body(p.answer(post("/api/pages/laptop", json!({ "path": "/home/sam/x", "name": "X" }))).await).await;
        assert_eq!((status, added["host"].as_str(), added["slug"].as_str()), (200, Some("laptop"), Some("x")));
        assert_eq!(computer.last(), "add --json --name 'X' -- '/home/sam/x'");
        // never a folder that isn't written in full, and never on a machine the phone doesn't have
        assert_eq!(body(p.answer(post("/api/pages/laptop", json!({ "path": "~/x" }))).await).await, (400, json!({ "error": NOT_ABSOLUTE })));
        assert_eq!(body(p.answer(post("/api/pages/nowhere", json!({ "path": "/x" }))).await).await.0, 404);

        let url = open(&p, "plan").await;
        let (status, _) = body(p.answer(test_req("DELETE", "/api/pages/laptop/plan", &[], "")).await).await;
        assert_eq!(status, 200);
        assert_eq!(computer.last(), "rm --json -- 'plan'");
        // its links end with it
        assert_eq!(get(&p, &format!("{url}index.html")).await.0, 403);
    });
}
