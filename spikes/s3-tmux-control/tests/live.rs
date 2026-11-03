//! Against a real tmux. Each test owns a private server (see common/mod.rs).
#![cfg(unix)]

mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{collect, contains, wait_file, wait_for, Server, Stall};
use s3_tmux_control::tmux::{self, attach_args, Version};
use s3_tmux_control::{Control, Error, Notification};

const T: Duration = Duration::from_secs(10);

#[test]
fn lists_sessions_windows_and_panes() {
    let s = Server::start("list");
    let (c, _rx) = s.attach(false);
    for args in [
        &["new-session", "-d", "-s", "rød økt", "-n", "vindu ø", "-x", "90", "-y", "20"][..],
        &["new-window", "-d", "-t", "main:", "-n", "build"],
        &["split-window", "-d", "-t", "main:build"],
    ] {
        assert!(s.tmux.run(args).unwrap().status.success());
    }
    s.fit("main:build");

    let panes = tmux::list_panes(&c).unwrap();
    let names: Vec<_> = panes.iter().map(|p| (p.session_name.as_str(), p.window_name.as_str())).collect();
    assert!(names.contains(&("rød økt", "vindu ø")), "{names:?}");
    let build: Vec<_> = panes.iter().filter(|p| p.window_name == "build").collect();
    assert_eq!(build.len(), 2, "{panes:#?}");
    assert!(build.iter().all(|p| p.pane_id.starts_with('%') && p.window_id.starts_with('@') && p.session_id.starts_with('$')));
    // pane_current_command can still read "tmux" for a pane forked a moment ago, so not checked
    assert!(build.iter().all(|p| p.width == 120 && p.pid > 0 && !p.dead), "{build:#?}");
    let other = panes.iter().find(|p| p.session_name == "rød økt").unwrap();
    // 3.2 sizes a detached session from default-size, which the test server sets to 120x40
    if s.version() != (Version { major: 3, minor: 2 }) {
        assert_eq!((other.width, other.height), (90, 20));
    }

    // the plain listings work too
    let sessions = c.cmd("list-sessions -F '#{session_id} #{session_name}'").unwrap();
    assert_eq!(sessions.lines.len(), 2);
    let windows = c.cmd("list-windows -a -F '#{window_id} #{window_name}'").unwrap();
    assert_eq!(windows.lines.len(), 3);
    println!("tmux {:?}: {} panes listed over control mode", s.version(), panes.len());
}

#[test]
fn captures_the_screen_with_styles() {
    let s = Server::start("capture");
    let (c, rx) = s.attach(false);
    let pane = s.window("cap", "printf '\\033[31mred\\033[0m plain rød\\n'; exec sleep 60\n");
    collect(&rx, &pane, T, |b| contains(b, b"plain"));
    let rows = tmux::capture(&c, &pane).unwrap();
    let first = &rows[0];
    assert!(contains(first, b"\x1b[31mred"), "{:?}", String::from_utf8_lossy(first));
    assert!(contains(first, "plain rød".as_bytes()));
    assert_eq!(rows.len(), 40, "one entry per visible row");

    // several panes in one write
    let all = tmux::capture_many(&c, &[&pane, &pane, "%9999"]);
    assert!(all[0].is_ok() && all[1].is_ok());
    assert!(matches!(&all[2], Err(Error::Command(m)) if m.contains("can't find pane")), "{:?}", all[2]);
}

#[test]
fn streams_output_and_decodes_escapes() {
    let s = Server::start("output");
    let (c, rx) = s.attach(true);
    let pane = s.window("out", "printf 'back\\\\slash\\ttab\\001ctl rød\\033[1mbold\\n'; exec sleep 60\n");
    let out = collect(&rx, &pane, T, |b| contains(b, b"bold"));
    assert!(contains(&out, "back\\slash\ttab\x01ctl rød\x1b[1mbold".as_bytes()), "{:?}", String::from_utf8_lossy(&out));
    drop(c);
}

#[test]
fn types_keys_and_literal_text() {
    let s = Server::start("keys");
    let (c, rx) = s.attach(false);
    let file = s.dir.join("typed");
    let pane = s.window("keys", &format!("stty raw -echo; printf ready; exec cat > {}\n", file.display()));
    collect(&rx, &pane, T, |b| contains(b, b"ready"));

    let text = "a \"b\" $HOME ; \\ ~ #{pane_id} 'q' -- rød\nnext";
    tmux::type_text(&c, &pane, text).unwrap();
    tmux::send_keys(&c, &pane, &["Enter"]).unwrap();
    let got = wait_file(&file, T, |b| b.ends_with(b"next\r"));
    // newlines go in as C-j (\n), Enter as \r; nothing is expanded or eaten by tmux's parser
    assert_eq!(String::from_utf8_lossy(&got), "a \"b\" $HOME ; \\ ~ #{pane_id} 'q' -- rød\nnext\r");
    assert!(matches!(tmux::send_keys(&c, &pane, &["Enter; kill-server"]), Err(Error::Command(_))));
}

#[test]
fn pastes_as_one_bracketed_paste() {
    let s = Server::start("paste");
    let (c, rx) = s.attach(false);
    let file = s.dir.join("pasted");
    let pane = s.window("paste", &format!("stty raw -echo; printf '\\033[?2004h'; exec cat > {}\n", file.display()));
    // paste-buffer -p only wraps when the program has asked for it, so wait until it has
    collect(&rx, &pane, T, |b| contains(b, b"\x1b[?2004h"));

    let text = "line1\nline2 \"q\" $X\ttab ; \\ end";
    tmux::paste(&c, &pane, text).unwrap();
    let got = wait_file(&file, T, |b| b.ends_with(b"\x1b[201~"));
    assert_eq!(String::from_utf8_lossy(&got), "\x1b[200~line1\rline2 \"q\" $X\ttab ; \\ end\x1b[201~");

    // a long paste goes over in many set-buffer chunks and still arrives as one paste
    let big: String = (0..4000).map(|i| format!("{i:05} the quick brown fox ø\n")).collect();
    let file = s.dir.join("pasted-big");
    let pane = s.window("big", &format!("stty raw -echo; printf '\\033[?2004h'; exec cat > {}\n", file.display()));
    collect(&rx, &pane, T, |b| contains(b, b"\x1b[?2004h"));
    let t0 = Instant::now();
    tmux::paste(&c, &pane, &big).unwrap();
    let got = wait_file(&file, Duration::from_secs(30), |b| b.ends_with(b"\x1b[201~"));
    let want = format!("\x1b[200~{}\x1b[201~", big.replace('\n', "\r"));
    assert_eq!(got.len(), want.len());
    assert!(got == want.as_bytes());
    println!("pasted {} KB in {:?}", big.len() / 1024, t0.elapsed());
    // the buffer is gone afterwards (paste-buffer -d)
    let bufs = c.cmd("list-buffers -F '#{buffer_name}'").unwrap();
    assert!(!bufs.text().contains("inyon-"), "{}", bufs.text());
}

#[test]
fn pastes_without_brackets_when_not_asked() {
    let s = Server::start("plainpaste");
    let (c, rx) = s.attach(false);
    let file = s.dir.join("pasted");
    let pane = s.window("paste", &format!("stty raw -echo; printf ready; exec cat > {}\n", file.display()));
    collect(&rx, &pane, T, |b| contains(b, b"ready"));
    tmux::paste(&c, &pane, "one\ntwo").unwrap();
    let got = wait_file(&file, T, |b| b.ends_with(b"two"));
    assert_eq!(got, b"one\rtwo");
}

/// Expected pane bytes for `seq 1 n` after the terminal's \n → \r\n.
fn seq_lines(n: u32) -> Vec<u8> {
    (1..=n).flat_map(|i| format!("{i}\n").into_bytes()).collect()
}

fn strip_cr(b: &[u8]) -> Vec<u8> {
    b.iter().copied().filter(|&c| c != b'\r').collect()
}

#[test]
fn keeps_every_byte_under_load() {
    let s = Server::start("load");
    let (c, rx) = s.attach(true);
    let n = 300_000;
    let t0 = Instant::now();
    let pane = s.window("load", &format!("sleep 0.3; seq 1 {n}; echo LOAD-DONE; exec sleep 60\n"));
    let out = collect(&rx, &pane, Duration::from_secs(60), |b| contains(&b[b.len().saturating_sub(64)..], b"LOAD-DONE"));
    let el = t0.elapsed();
    let got = strip_cr(&out);
    let want = seq_lines(n);
    assert!(got.starts_with(&want), "lost or reordered output: got {} bytes, want {}+", got.len(), want.len());
    let wire = c.stats.bytes.load(std::sync::atomic::Ordering::Relaxed);
    println!("load: {} KB of pane output in {el:?} ({:.1} MB/s), {} KB on the wire", want.len() / 1024, want.len() as f64 / el.as_secs_f64() / 1e6, wire / 1024);
}

#[test]
fn a_client_that_stops_reading_loses_nothing() {
    let s = Server::start("stall");
    // our own spawn, so the reader can be made to fall behind for 3 s
    let mut cmd = s.tmux.command();
    cmd.args(attach_args(s.version(), "main", true));
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let stall = Stall { inner: child.stdout.take().unwrap(), after: 4096, stall: Duration::from_secs(3), seen: 0, done: false };
    let (c, rx) = Control::over(stall, child.stdin.take().unwrap());
    assert!(c.wait_ready(T));
    let n = 200_000;
    let pane = s.window("stall", &format!("sleep 0.3; seq 1 {n}; echo STALL-DONE; exec sleep 60\n"));
    let out = collect(&rx, &pane, Duration::from_secs(60), |b| contains(&b[b.len().saturating_sub(64)..], b"STALL-DONE"));
    assert!(strip_cr(&out).starts_with(&seq_lines(n)), "lost output after a 3 s stall");
    assert!(!c.closed());
    drop(c);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn pause_after_pauses_instead_of_buffering() {
    let s = Server::start("pause");
    if !s.version().has_client_flags() {
        println!("tmux {:?}: no pause-after before 3.2, skipped", s.version());
        return;
    }
    let mut cmd = s.tmux.command();
    cmd.args(attach_args(s.version(), "main", false));
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // a 5 s stall against pause-after=1; 3 s was not always enough on a CI runner
    let stall = Stall { inner: child.stdout.take().unwrap(), after: 256 << 10, stall: Duration::from_secs(5), seen: 0, done: false };
    let (c, rx) = Control::over(stall, child.stdin.take().unwrap());
    assert!(c.wait_ready(T));
    c.cmd("refresh-client -f pause-after=1").unwrap();
    let pane = s.window("pause", "sleep 0.3; while :; do seq 1 20000; sleep 0.05; done\n");
    // tmux decides to pause when new output finds old output still queued; on CI runners that
    // did not always happen within the stall, so a run without %pause is noted, not failed
    let end = Instant::now() + Duration::from_secs(30);
    let mut paused = None;
    while paused.is_none() && Instant::now() < end {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Notification::Pause { pane: p }) => paused = Some(p),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    println!("tmux {:?}: %pause after a 5 s stall: {paused:?}", s.version());
    let Some(paused) = paused else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    assert_eq!(paused, pane);
    // output arrives as %extended-output with its age once pause-after is on
    c.cmd(format!("refresh-client -A '{pane}:continue'")).unwrap();
    wait_for(&rx, T, |n| matches!(n, Notification::Continue { pane: p } if *p == pane).then_some(()));
    wait_for(&rx, T, |n| matches!(n, Notification::ExtendedOutput { pane: p, .. } if *p == pane).then_some(()));
    drop(c);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn resizes_windows_and_panes() {
    let s = Server::start("resize");
    let (c, rx) = s.attach(false);
    let window = tmux::list_panes(&c).unwrap()[0].window_id.clone();
    tmux::resize_window(&c, &window, 100, 30).unwrap();
    let p = &tmux::list_panes(&c).unwrap()[0];
    assert_eq!((p.width, p.height), (100, 30));
    // 3.3a did not always send %layout-change for resize-window in CI, so only note it
    let seen = rx.recv_timeout(Duration::from_secs(2)).map(|n| format!("{n:?}"));
    println!("tmux {:?}: after resize-window: {seen:?}", s.version());

    c.cmd(format!("split-window -h -d -t {window}")).unwrap();
    let first = tmux::list_panes(&c).unwrap()[0].pane_id.clone();
    tmux::resize_pane(&c, &first, 40, 30).unwrap();
    let panes = tmux::list_panes(&c).unwrap();
    assert_eq!(panes[0].width, 40);
    assert_eq!(panes[1].width, 100 - 40 - 1, "the other pane takes the rest, less the border");
}

#[test]
fn a_control_client_does_not_resize_windows() {
    let s = Server::start("size");
    let (c, _rx) = s.attach(true);
    std::thread::sleep(Duration::from_millis(300));
    let p = &tmux::list_panes(&c).unwrap()[0];
    println!("tmux {:?}: window is {}x{} with a control client attached", s.version(), p.width, p.height);
    assert_eq!((p.width, p.height), (120, 40), "the control client changed the window size");
}

/// A window made by a command from the control client. tmux 3.2's default_window_size still
/// counts the creating client, control or not (3.3 added the exception), so this records what
/// size such a window gets.
#[test]
fn windows_made_over_control_mode() {
    let s = Server::start("ctlwindow");
    let (c, _rx) = s.attach(false);
    let id = c.cmd("new-window -d -t main: -P -F '#{pane_id}'").unwrap().text().trim().to_string();
    let p = tmux::list_panes(&c).unwrap().into_iter().find(|p| p.pane_id == id).unwrap();
    println!("tmux {:?}: a window made over control mode is {}x{}", s.version(), p.width, p.height);
    if s.version() != (Version { major: 3, minor: 2 }) {
        assert_eq!((p.width, p.height), (120, 40));
    }
}

/// The hub's attach as it is today (live.ts: `-C attach-session -t <s> -f read-only,ignore-size`)
/// on a session nobody has a terminal on.
#[test]
fn the_hubs_attach_today() {
    // five fresh servers, to catch a shrink that only happens sometimes
    let mut sizes = Vec::new();
    let mut before = Vec::new();
    let mut v = None;
    for round in 0..5 {
        let s = Server::start(&format!("hubattach{round}"));
        v = Some(s.version());
        before.push(s.tmux.window_size("main"));
        let mut cmd = s.tmux.command();
        cmd.args(attach_args(s.version(), "main", true));
        let (c, _rx) = Control::spawn(cmd).unwrap();
        assert!(c.wait_ready(T));
        std::thread::sleep(Duration::from_millis(300));
        let p = &tmux::list_panes(&c).unwrap()[0];
        sizes.push((p.width, p.height));
    }
    let v = v.unwrap();
    println!("tmux {v:?}: before the hub's attach the window was {before:?}, after {sizes:?}");
    assert!(sizes.iter().all(|&s| s == (120, 40)));
}

/// The hub's arguments word for word, whatever the version: tmux before 3.2 has no `-f` on
/// attach-session, so there the hub's live client never attaches.
#[test]
fn the_hubs_exact_arguments() {
    let s = Server::start("hubargs");
    let mut cmd = s.tmux.command();
    cmd.args(["-C", "attach-session", "-t", "main", "-f", "read-only,ignore-size"]);
    let (c, _rx) = Control::spawn(cmd).unwrap();
    let attached = c.wait_ready(T);
    println!("tmux {:?}: `-C attach-session -t main -f read-only,ignore-size` attached: {attached}", s.version());
    assert_eq!(attached, s.version().has_client_flags());
}

#[test]
fn reports_structure_changes() {
    let s = Server::start("structure");
    let (c, rx) = s.attach(true);
    let (w, _) = s.attach(false);
    let r = w.cmd("new-window -d -t main: -n one -P -F '#{window_id}'").unwrap();
    let id = r.text().trim().to_string();
    wait_for(&rx, T, |n| matches!(n, Notification::WindowAdd { window } if *window == id).then_some(()));
    w.cmd(format!("rename-window -t {id} two")).unwrap();
    wait_for(&rx, T, |n| matches!(n, Notification::WindowRenamed { name, .. } if name == "two").then_some(()));
    w.cmd(format!("split-window -d -t {id}")).unwrap();
    wait_for(&rx, T, |n| matches!(n, Notification::LayoutChange { window, .. } if *window == id).then_some(()));
    w.cmd(format!("kill-window -t {id}")).unwrap();
    // a killed window has left the session by the time it is reported
    wait_for(&rx, T, |n| matches!(n, Notification::WindowClose { window } | Notification::UnlinkedWindowClose { window } if *window == id).then_some(()));
    w.cmd("new-session -d -s other").unwrap();
    wait_for(&rx, T, |n| matches!(n, Notification::SessionsChanged).then_some(()));
    drop(c);
}

/// read-only stops a client's own keyboard, not commands it sends: over control mode a read-only
/// client can still send-keys. So read-only is no safety boundary for Inyon, only a size and
/// input courtesy. The test records what each version does.
#[test]
fn a_read_only_client_can_still_send_keys() {
    let s = Server::start("readonly");
    let (c, rx) = s.attach(true);
    let pane = tmux::list_panes(&c).unwrap()[0].pane_id.clone();
    let r = tmux::type_text(&c, &pane, "echo RO-$((6*7))").and_then(|_| tmux::send_keys(&c, &pane, &["Enter"]));
    println!("tmux {:?}: send-keys from a read-only client -> {r:?}", s.version());
    if r.is_ok() {
        collect(&rx, &pane, T, |b| contains(b, b"RO-42"));
    }
}

#[test]
fn exits_when_its_session_is_killed() {
    let s = Server::start("exit");
    s.tmux.run(["new-session", "-d", "-s", "gone"]).unwrap();
    let (c, rx) = s.tmux.attach("gone", true).unwrap();
    assert!(tmux::list_panes(&c).is_ok());
    s.tmux.run(["kill-session", "-t", "gone"]).unwrap();
    // 3.2 and later say %exit first; 3.0 just closes the stream
    let mut exit = None;
    let end = Instant::now() + T;
    loop {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Notification::Exit { reason }) => exit = Some(reason),
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(e) => panic!("still attached: {e:?}"),
        }
    }
    println!("tmux {:?}: session killed, %exit: {exit:?}", s.version());
    assert!(c.closed());
    assert!(matches!(c.cmd("list-sessions"), Err(Error::Closed(_))));
}

#[test]
fn exits_when_the_server_dies() {
    let s = Server::start("serverexit");
    let (c, rx) = s.attach(true);
    s.tmux.run(["kill-server"]).unwrap();
    let end = Instant::now() + T;
    let mut saw_exit = false;
    loop {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Notification::Exit { .. }) => saw_exit = true,
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    println!("tmux {:?}: server killed, %exit seen: {saw_exit}", s.version());
    assert!(c.closed());
}

#[test]
fn detaches_when_stdin_closes() {
    let s = Server::start("detach");
    let (c, rx) = s.attach(true);
    c.detach();
    let end = Instant::now() + T;
    while rx.recv_timeout(end.saturating_duration_since(Instant::now())).is_ok() {}
    assert!(c.closed());
    // and the server is still there
    assert!(s.tmux.run(["has-session", "-t", "main"]).unwrap().status.success());
}

#[test]
fn pipelines_commands_in_order() {
    let s = Server::start("pipeline");
    let (c, _rx) = s.attach(false);
    let n = 2000;
    let cmds: Vec<String> = (0..n).map(|i| format!("display-message -p 'n={i} #{{session_name}}'")).collect();
    let t0 = Instant::now();
    let replies = c.cmds(&cmds);
    let el = t0.elapsed();
    for (i, r) in replies.iter().enumerate() {
        assert_eq!(r.as_ref().unwrap().text(), format!("n={i} main"));
    }
    // the same as one process per command, which is what a shell round trip costs at best
    let t1 = Instant::now();
    for i in 0..50 {
        let out = s.tmux.run(["display-message", "-p", &format!("n={i}")]).unwrap();
        assert!(out.status.success());
    }
    let per_proc = t1.elapsed() / 50;
    println!("pipelined: {n} commands in {el:?} ({:?} each); one tmux process per command: {per_proc:?} each", el / n);
    let v = c.cmd("display-message -p '#{version}'").unwrap();
    println!("#{{version}} over control mode: {:?}", v.text());
}

#[test]
fn subscriptions_report_format_changes() {
    let s = Server::start("subscribe");
    if !s.version().has_client_flags() {
        println!("tmux {:?}: no subscriptions before 3.2, skipped", s.version());
        return;
    }
    let (c, rx) = s.attach(false);
    c.cmd("refresh-client -B 'cmd:%*:#{pane_current_command}'").unwrap();
    let pane = tmux::list_panes(&c).unwrap()[0].pane_id.clone();
    // tmux re-checks subscriptions at most once a second, and in our runs only after the pane
    // wrote something: a script that exec'd sleep without output was never reported
    tmux::type_text(&c, &pane, "exec sleep 60").unwrap();
    tmux::send_keys(&c, &pane, &["Enter"]).unwrap();
    let v = wait_for(&rx, T, |n| match n {
        Notification::SubscriptionChanged { name, pane: p, value } if name == "cmd" && *p == pane && value == "sleep" => Some(value.clone()),
        _ => None,
    });
    assert_eq!(v, "sleep");
}
