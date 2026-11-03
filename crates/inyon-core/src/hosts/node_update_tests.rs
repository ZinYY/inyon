//! hosts/node-update.test.ts. "the API refuses machines it cannot update" goes through the hub's
//! route, so it is in inyon-hub's core_routes.rs; the overview test is in registry_tests.rs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use inyon_protocol::{newer_release, HostState, VERSION};
use serde_json::{json, Value};

use super::*;
use crate::hosts::host::ExecResult;
use crate::hosts::node_link::{self, NodeSocket};
use crate::test_support::fake::{ok, Fake};
use crate::test_support::rt::{run, wait};

fn fast() -> Options {
    Options { poll_ms: 5, timeout_ms: 2000, keep_ms: 50, failed_ms: 1000 }
}

struct Sock;
impl NodeSocket for Sock {
    fn send(&self, _: &str) {}
    fn close(&self, _: u16, _: &str) {}
}

fn state(id: &str, kind: &str, more: Value) -> HostState {
    let mut s =
        json!({ "config": { "id": id, "name": id, "kind": kind }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 0 });
    for (k, v) in more.as_object().unwrap() {
        s[k] = v.clone();
    }
    serde_json::from_value(s).unwrap()
}

/// A machine connected over the node link, saying hello with this version.
fn connect(id: &str, version: &str, os: &str) {
    node_link::attach(id, Rc::new(Sock));
    node_link::receive(id, &json!({ "type": "hello", "version": version, "os": os }));
}

/// A fake machine: answers the launcher (with `launch`), then the status file with `status`.
fn fake(launch: ExecResult) -> (Rc<Fake>, Rc<RefCell<String>>) {
    let status = Rc::new(RefCell::new("running".to_owned()));
    let st = status.clone();
    let f = Fake::install(move |_, cmd, _| if cmd.contains("update-status\" 2>/dev/null") { ok(&format!("{}\n", st.borrow())) } else { launch.clone() });
    (f, status)
}

fn host(id: &str) -> Host {
    Host::new(serde_json::from_value(json!({ "id": id, "name": id, "kind": "node" })).unwrap())
}

async fn until(check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            break;
        }
        wait(5).await;
    }
    assert!(check());
}

fn scripts(u: &NodeUpdates, text: &'static str) {
    u.use_scripts(move |_, _| Box::pin(async move { Ok(text.to_owned()) }));
}

#[test]
fn versions_compare_by_number() {
    assert!(newer_release("0.6.3", "0.3.0"));
    assert!(newer_release("0.10.0", "0.9.12"));
    assert!(!newer_release("0.6.3", "0.6.3"));
    assert!(!newer_release("garbage", "0.1.0"));
}

#[test]
fn a_machine_paired_with_the_hub_that_runs_an_older_inyon_shows_an_update() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        let view = |s: HostState| serde_json::to_value(u.view(&s)).unwrap();
        assert_eq!(view(state("old", "node", json!({ "nodeVersion": "0.3.0" }))), json!({ "latest": VERSION, "supported": true }));
        assert_eq!(view(state("same", "node", json!({ "nodeVersion": VERSION }))), Value::Null);
        assert_eq!(view(state("ssh", "ssh", json!({ "nodeVersion": "0.1.0" }))), Value::Null);
        // phone-paired laptops compare with the newest release, once it's known
        assert_eq!(view(state("laptop", "link", json!({ "nodeVersion": "0.6.3" }))), Value::Null);
        u.set_latest_release(Some("0.7.0".into()));
        // too old to have `tether-node update`: the app says to run setup again instead
        assert_eq!(view(state("laptop", "link", json!({ "nodeVersion": "0.6.3" }))), json!({ "latest": "0.7.0", "supported": false }));
        assert_eq!(view(state("laptop", "link", json!({ "nodeVersion": "0.6.4" }))), json!({ "latest": "0.7.0", "supported": true }));
    });
}

#[test]
fn refusals_say_why() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        scripts(&u, "script");
        let _f = fake(ok(""));
        let h = host("r");
        let start = |s: HostState| u.start(&h, &s);
        assert_eq!(start(state("r", "ssh", json!({}))).as_deref(), Some("Only machines running inyon can be updated from here."));
        assert_eq!(start(state("r", "node", json!({ "status": "connecting", "nodeVersion": "0.3.0" }))).as_deref(), Some("r isn't connected. Update it once it's back online."));
        // the registry still thinks it's online, but the link just dropped
        assert_eq!(start(state("r", "node", json!({ "nodeVersion": "0.3.0" }))).as_deref(), Some("r isn't connected. Update it once it's back online."));
        connect("r", VERSION, "linux x64");
        assert_eq!(start(state("r", "node", json!({ "nodeVersion": VERSION }))).as_deref(), Some("inyon on r is already up to date."));
        // tether-node as a Bun script, from before the rename: set up again rather than updated
        connect("r", "0.3.0", "linux x64");
        assert_eq!(
            start(state("r", "node", json!({ "nodeVersion": "0.3.0" }))).as_deref(),
            Some("r runs tether-node from before the rename to Inyon. Copy the \"Add a machine\" command again and run it there.")
        );
        assert_eq!(serde_json::to_value(u.view(&state("r", "node", json!({ "nodeVersion": "0.3.0" })))).unwrap(), json!({ "latest": VERSION, "supported": false }));
        connect("r", "0.3.0", "win32 x64");
        assert_eq!(NodeUpdates::new(fast()).start(&h, &state("r", "node", json!({ "nodeVersion": "0.3.0" }))).as_deref(), Some("This hub can't send updates to tether-node."));
        assert_eq!(start(state("r", "node", json!({ "nodeVersion": "0.3.0" }))), None);
        assert_eq!(start(state("r", "node", json!({ "nodeVersion": "0.3.0" }))).as_deref(), Some("inyon on r is already updating."));
        connect("l", "0.6.3", "linux x64");
        u.set_latest_release(Some("0.7.0".into()));
        assert_eq!(start(state("l", "link", json!({ "nodeVersion": "0.6.3" }))).as_deref(), Some("tether-node 0.6.3 can't update itself. Run its setup command on l again."));
    });
}

#[test]
fn an_old_windows_machine_gets_the_script_through_exec_and_the_update_ends_when_it_is_back_on_the_new_version() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        let asked: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
        let a = asked.clone();
        u.use_scripts(move |platform, arch| {
            a.borrow_mut().push((platform.into(), arch.into()));
            Box::pin(async { Ok("the ps1 with its checksum".to_owned()) })
        });
        let changes = Rc::new(Cell::new(0));
        let c = changes.clone();
        let _off = u.on_change(move || c.set(c.get() + 1));
        connect("win", "0.3.0", "win32 x64");
        let (m, _) = fake(ok(""));
        assert_eq!(u.start(&host("win"), &state("win", "node", json!({ "nodeVersion": "0.3.0" }))), None);
        assert_eq!(
            serde_json::to_value(u.view(&state("win", "node", json!({ "nodeVersion": "0.3.0" })))).unwrap(),
            json!({ "latest": VERSION, "supported": true, "state": "updating" })
        );
        until(|| m.calls.borrow().len() >= 2).await;
        assert_eq!(*asked.borrow(), vec![("win32".to_owned(), "x64".to_owned())]);
        let first = m.calls.borrow()[0].clone();
        assert_eq!((first.cmd.as_str(), first.input.as_deref()), (WINDOWS_LAUNCHER, Some("the ps1 with its checksum")));
        assert!(first.cmd.contains("-File \"$d/update.ps1\" -Launch"));

        // the node restarts and comes back with the hub's version
        connect("win", VERSION, "win32 x64");
        until(|| u.view(&state("win", "node", json!({ "nodeVersion": VERSION }))).and_then(|v| v.state) == Some(NodeUpdateState::Updated)).await;
        assert!(changes.get() >= 2);
        // a minute later (here 50 ms) it's just current again
        until(|| u.view(&state("win", "node", json!({ "nodeVersion": VERSION }))).is_none()).await;
    });
}

#[test]
fn a_failure_the_script_reports_is_shown_and_the_next_try_starts_over() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        scripts(&u, "script");
        connect("bad", "0.3.0", "win32 x64");
        let (m, status) = fake(ok(""));
        let s = || state("bad", "node", json!({ "nodeVersion": "0.3.0" }));
        u.start(&host("bad"), &s());
        until(|| m.calls.borrow().len() >= 2).await;
        *status.borrow_mut() = "failed: the download didn't match the hub's checksum, so nothing was changed".into();
        until(|| u.view(&s()).and_then(|v| v.state) == Some(NodeUpdateState::Failed)).await;
        assert_eq!(u.view(&s()).unwrap().error.as_deref(), Some("The download didn't match the hub's checksum, so nothing was changed"));
        *status.borrow_mut() = "running".into();
        assert_eq!(u.start(&host("bad"), &s()), None);
        assert_eq!(u.view(&s()).unwrap().state, Some(NodeUpdateState::Updating));
    });
}

#[test]
fn a_launcher_that_cant_start_fails_at_once_with_its_last_line_of_output() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        scripts(&u, "script");
        connect("nolaunch", "0.3.0", "win32 x64");
        let _m = fake(ExecResult { code: 1, stdout: String::new(), stderr: "powershell.exe: not found\n".into() });
        let s = || state("nolaunch", "node", json!({ "nodeVersion": "0.3.0" }));
        u.start(&host("nolaunch"), &s());
        until(|| u.view(&s()).and_then(|v| v.state) == Some(NodeUpdateState::Failed)).await;
        assert_eq!(u.view(&s()).unwrap().error.as_deref(), Some("powershell.exe: not found"));
    });
}

#[test]
fn an_update_that_never_finishes_times_out() {
    run(|| async {
        let u = NodeUpdates::new(Options { timeout_ms: 30, ..fast() });
        scripts(&u, "script");
        connect("slow", "0.3.0", "win32 x64");
        let _m = fake(ok(""));
        let s = || state("slow", "node", json!({ "nodeVersion": "0.3.0" }));
        u.start(&host("slow"), &s());
        until(|| u.view(&s()).and_then(|v| v.state) == Some(NodeUpdateState::Failed)).await;
        assert!(u.view(&s()).unwrap().error.unwrap().contains("taking too long"));
    });
}

#[test]
fn a_laptop_paired_with_the_phone_that_still_runs_tether_node_runs_its_own_update_detached() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        u.set_latest_release(Some("0.7.0".into()));
        connect("lap", "0.6.4", "darwin arm64");
        let (m, _) = fake(ok(""));
        assert_eq!(u.start(&host("lap"), &state("lap", "link", json!({ "nodeVersion": "0.6.4" }))), None);
        until(|| !m.calls.borrow().is_empty()).await;
        let first = m.calls.borrow()[0].clone();
        assert_eq!((first.cmd.as_str(), first.input), (LINK_UPDATE, None));
        assert_eq!(LINK_UPDATE, r#""$TETHER_NODE_EXE" ${TETHER_NODE_SCRIPT:+"$TETHER_NODE_SCRIPT"} update --background"#);
    });
}

#[test]
fn from_0_7_0_a_machine_runs_its_own_inyon_update_detached_and_its_status_is_read_where_inyon_keeps_it() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        u.set_latest_release(Some("0.7.1".into()));
        connect("new", "0.7.0", "linux arm64");
        let (m, _) = fake(ok(""));
        assert_eq!(u.start(&host("new"), &state("new", "link", json!({ "nodeVersion": "0.7.0" }))), None);
        until(|| m.calls.borrow().len() >= 2).await;
        let calls = m.calls.borrow().clone();
        assert_eq!((calls[0].cmd.as_str(), calls[0].input.clone()), (SELF_UPDATE, None));
        assert_eq!(SELF_UPDATE, r#""$INYON_EXE" ${INYON_SCRIPT:+"$INYON_SCRIPT"} update --background"#);
        assert_eq!(calls[1].cmd, r#"cat "$HOME/.inyon/node/update-status" 2>/dev/null || cat "$HOME/.tether-node/update-status" 2>/dev/null; true"#);
    });
}

#[test]
fn the_phone_learns_the_newest_release_from_latest_json_on_the_website() {
    run(|| async {
        let u = NodeUpdates::new(fast());
        let asked = Rc::new(Cell::new(0));
        let a = asked.clone();
        let timer = u.watch_releases(
            move || {
                a.set(a.get() + 1);
                Box::pin(async { latest_release_from(200, r#"{"version":"0.7.2","tag":"v0.7.2","files":[]}"#) })
            },
            6 * 3_600_000,
        );
        crate::platform::rt::clear(&mut timer.borrow_mut());
        until(|| u.latest_release().as_deref() == Some("0.7.2")).await;
        assert_eq!(asked.get(), 1);
    });
}

#[test]
fn a_latest_json_that_fails_or_names_no_release_leaves_the_last_known_version_alone() {
    run(|| async {
        for (status, body) in [(404, "gone"), (200, r#"{"version":"0.8.0-rc.1"}"#), (200, r#"{"tag":"v0.8.0"}"#), (200, "<html>")] {
            let u = NodeUpdates::new(fast());
            u.set_latest_release(Some("0.7.2".into()));
            let asked = Rc::new(Cell::new(0));
            let a = asked.clone();
            let timer = u.watch_releases(
                move || {
                    a.set(a.get() + 1);
                    Box::pin(async move { latest_release_from(status, body) })
                },
                6 * 3_600_000,
            );
            crate::platform::rt::clear(&mut timer.borrow_mut());
            until(|| asked.get() == 1).await;
            wait(10).await;
            assert_eq!(u.latest_release().as_deref(), Some("0.7.2"));
        }
    });
}

#[test]
fn fetch_latest_release_reads_a_plain_release_number_and_nothing_else() {
    assert_eq!(latest_release_from(200, r#"{"version":" v0.14.1 "}"#).as_deref(), Ok("0.14.1"));
    for v in ["\"0.15.0-rc.1\"", "\"0.15\"", "\"latest\"", "\"\"", "15", "null"] {
        assert_eq!(latest_release_from(200, &format!("{{\"version\":{v},\"files\":[]}}")), Err("inyon.tech didn't name a release".into()));
    }
    assert_eq!(latest_release_from(404, "not found"), Err("inyon.tech answered 404".into()));
}

#[test]
fn a_hub_on_a_test_build_offers_it_to_machines_on_the_release_before_and_only_to_them() {
    assert!(newer_target("0.19.0-rc.1", "0.18.0"));
    assert!(newer_target("0.19.0-rc.1", "0.18.0-build.7"));
    assert!(newer_target("0.19.0-rc.2", "0.19.0-rc.1"));
    assert!(!newer_target("0.19.0-rc.1", "0.19.0-rc.1"));
    assert!(!newer_target("0.19.0-rc.1", "0.19.0-rc.2"));
    assert!(!newer_target("0.19.0-rc.1", "0.19.0"));
    assert!(!newer_target("0.19.0-rc.1", "0.20.0"));
    // a release target is compared as before
    assert!(newer_target("0.19.0", "0.19.0-rc.1"));
    assert!(!newer_target("0.18.0", "0.18.0"));
}

/// From node-update-scripts.test.ts: the parts that don't need PowerShell.
#[test]
fn only_a_plain_checksum_version_and_architecture_go_into_the_script() {
    let ps1 = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../inyon-hub/assets/hub-update.ps1")).unwrap();
    let sha = "a".repeat(64);
    assert!(fill_update_script(&ps1, &sha, "0.6.3", "x64").unwrap().contains(&format!("$Want = '{sha}'")));
    assert!(fill_update_script(&ps1, &sha, "0.6.3", "arm64").unwrap().contains("$Arch = 'arm64'"));
    assert!(fill_update_script(&ps1, &sha, "0.6.3'; rm -rf ~; '", "x64").unwrap_err().contains("Bad version"));
    assert!(fill_update_script(&ps1, &"A".repeat(64), "0.6.3", "x64").unwrap_err().contains("Bad checksum"));
    assert!(fill_update_script(&ps1, &sha, "0.6.3", "x86").unwrap_err().contains("Bad architecture"));
    assert!(!fill_update_script(&ps1, &sha, "0.6.3", "x64").unwrap().contains("@@"));
}

#[test]
fn the_windows_launcher_writes_the_script_and_hands_it_to_powershell_to_start_as_a_task() {
    let l = WINDOWS_LAUNCHER;
    assert!(l.contains("cat > \"$d/update.ps1\""));
    assert!(l.contains(r#"printf 'running\n' > "$d/update-status""#));
    assert!(l.contains("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"$d/update.ps1\" -Launch"));
}
