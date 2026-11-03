//! The core's actions and terminal mirror against a real tmux server: claude/launch-ready.test.ts,
//! claude/end-session.test.ts, the restart test from claude/integration.test.ts, the end-to-end
//! half of terminal/mirror.test.ts and tmux/locale.test.ts. Each test has a tmux server of its
//! own, in its own folder (TMUX_TMPDIR, with $TMUX dropped), refuses to run unless tmux really
//! uses it, and stops it by its socket path: the user's server is never touched.

#![cfg(unix)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::future::LocalBoxFuture;
use indexmap::IndexMap;
use inyon_backends::process::ProcessBackend;
use inyon_backends::runtime::run_core;
use inyon_core::claude::actions::{argv, end_session, resume_command_argv, shell_ready, Closes, EndSession};
use inyon_core::hosts::host::{set_host_backends, ExecOptions, ExecResult, Host, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use inyon_core::hosts::http_error::HubResult;
use inyon_core::hosts::registry::{registry, HostRuntime};
use inyon_core::terminal::mirror::open_mirror;
use inyon_core::tmux::grid::rest_command;
use inyon_core::tmux::tmux::{capture_panes, type_into, AgentProc, ProbeResult, TmuxProbe};
use inyon_protocol::{AgentKind, HostConfig, IntegrationState};
use serde_json::json;

/// A folder for one test's tmux server, removed with the server.
struct Own {
    dir: PathBuf,
}

impl Own {
    fn new(name: &str) -> Own {
        let dir = PathBuf::from(format!("/tmp/inyon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Own { dir }
    }

    fn prefix(&self) -> String {
        format!("unset TMUX TMUX_PANE; export TMUX_TMPDIR='{}'; ", self.dir.display())
    }

    fn socket(&self) -> String {
        format!("{}/tmux-{}/default", self.dir.display(), unsafe { libc::getuid() })
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        // by socket path, so it can only ever stop this test's server
        let _ =
            std::process::Command::new("sh").arg("-c").arg(format!("[ -S '{s}' ] && tmux -S '{s}' kill-server 2>/dev/null; true", s = self.socket())).env_remove("TMUX").status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Every machine's commands run here, against the test's own tmux server.
struct OwnTmux {
    prefix: String,
    real: Rc<ProcessBackend>,
}

impl HostBackend for OwnTmux {
    fn exec(&self, config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        self.real.exec(config, &format!("{}{cmd}", self.prefix), opts)
    }
    fn stream(&self, config: &HostConfig, cmd: &str, on_line: OnLine, keep_stdin: bool) -> StreamHandle {
        self.real.stream(config, &format!("{}{cmd}", self.prefix), on_line, keep_stdin)
    }
    fn pty(&self, config: &HostConfig, cmd: &str, cols: u16, rows: u16, on_data: OnData) -> HubResult<PtyHandle> {
        self.real.pty(config, &format!("{}{cmd}", self.prefix), cols, rows, on_data)
    }
}

fn local_config(id: &str) -> HostConfig {
    serde_json::from_value(json!({ "id": id, "name": id, "kind": "local" })).unwrap()
}

/// Plugs in the test's server and checks tmux really uses it.
async fn own_tmux(own: &Own, id: &str) -> Host {
    own_tmux_with(own, id, "").await
}

/// The same, with `more` run before every command (after the server's own setup).
async fn own_tmux_with(own: &Own, id: &str, more: &str) -> Host {
    let b = Rc::new(OwnTmux { prefix: format!("{}{more}", own.prefix()), real: ProcessBackend::new(own.dir.join("cm"), vec![]) });
    set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
    let host = Host::new(local_config(id));
    host.run("tmux -f /dev/null new-session -d -s guard 'sleep 600'").await.unwrap();
    let socket = host.run("tmux display -p -t guard '#{socket_path}'").await.unwrap().stdout;
    assert!(socket.trim().starts_with(&own.dir.display().to_string()), "tmux used {socket:?}, not the test's own");
    host
}

async fn tmux(host: &Host, args: &str) -> String {
    host.run(&format!("tmux -f /dev/null {args}")).await.unwrap().stdout.trim().to_owned()
}

async fn until(what: impl Fn() -> LocalBoxFuture<'static, bool>, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if what().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out");
}

// ---------- launch-ready.test.ts ----------

async fn script_pane(own: &Own, host: &Host, script: &str) -> String {
    let f = own.dir.join(format!("s{}.sh", rand_id()));
    std::fs::write(&f, script).unwrap();
    tmux(host, &format!("new-session -d -x 100 -y 30 -P -F '#{{pane_id}}' sh {}", f.display())).await
}

fn rand_id() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

async fn screen(host: &Host, p: &str) -> String {
    capture_panes(host, &[p.to_owned()]).await.unwrap().get(p).cloned().unwrap_or_default().join("\n")
}

// test-isolation.test.ts: with $TMUX set, tmux ignores TMUX_TMPDIR, and a test once stopped the
// user's real server
#[test]
fn commands_run_by_tests_cant_reach_the_tmux_server_the_tests_were_started_from() {
    run_core(|| async {
        // a plain backend, as if cargo test ran inside the user's tmux
        let started_from = vec![("TMUX".to_owned(), "/tmp/tmux-1000/default,1,0".to_owned()), ("TMUX_PANE".to_owned(), "%0".to_owned())];
        let b = ProcessBackend::new("/tmp", started_from);
        set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
        let host = Host::new(local_config("iso"));
        let out = host.run(r#"echo "[$TMUX][$TMUX_PANE][$TMUX_TMPDIR]""#).await.unwrap().stdout;
        let out = out.trim();
        assert!(out.starts_with("[][][") && out.contains("inyon-test-tmux-"), "{out}");
    });
}

#[test]
fn waits_for_a_slow_shell_so_the_command_reaches_its_prompt() {
    let own = Own::new("ready-slow");
    run_core(|| async {
        let host = own_tmux(&own, "ready").await;
        // a shell whose startup takes a second and throws away anything typed meanwhile
        let p = script_pane(&own, &host, "sleep 1; stty -echo; while read -t 0.01 x; do :; done; stty echo; printf 'me@box ~ $ '; read cmd; echo \"ran: $cmd\"; sleep 30").await;
        shell_ready(&host, &p, 8000).await.unwrap();
        type_into(&host, &p, "claude", true).await.unwrap();
        let (h, pc) = (host.clone(), p.clone());
        until(
            move || {
                let (h, p) = (h.clone(), pc.clone());
                Box::pin(async move { screen(&h, &p).await.contains("ran:") })
            },
            2000,
        )
        .await;
        assert!(screen(&host, &p).await.contains("ran: claude"));
    });
}

#[test]
fn steps_past_zshs_first_run_questions_without_answering_them_for_good() {
    let own = Own::new("ready-zsh");
    run_core(|| async {
        let host = own_tmux(&own, "ready").await;
        // like zsh-newuser-install: one key, no Enter, and the question stays on screen afterwards
        let p = script_pane(
            &own,
            &host,
            r#"echo 'This is the Z Shell configuration function for new users,'; echo 'zsh-newuser-install.'; printf -- '--- Type one of the keys in parentheses --- '; stty -icanon min 1; k=$(dd bs=1 count=1 2>/dev/null); stty icanon; printf '\nme@box %% '; read cmd; echo "ran: $cmd ($k)"; sleep 30"#,
        )
        .await;
        shell_ready(&host, &p, 8000).await.unwrap();
        type_into(&host, &p, "claude", true).await.unwrap();
        let (h, pc) = (host.clone(), p.clone());
        until(
            move || {
                let (h, p) = (h.clone(), pc.clone());
                Box::pin(async move { screen(&h, &p).await.contains("ran:") })
            },
            2000,
        )
        .await;
        assert!(screen(&host, &p).await.contains("ran: claude (q)"));
    });
}

// ---------- end-session.test.ts ----------

/// What tmux has now: session:window → pane ids (the guard's session left out).
async fn layout(host: &Host) -> IndexMap<String, Vec<String>> {
    let out = tmux(host, "list-panes -a -F '#{session_name}:#{window_index} #{pane_id}' 2>/dev/null").await;
    let mut m: IndexMap<String, Vec<String>> = IndexMap::new();
    for l in out.lines().filter(|l| !l.is_empty() && !l.starts_with("guard:")) {
        let (w, p) = l.split_once(' ').unwrap();
        m.entry(w.to_owned()).or_default().push(p.to_owned());
    }
    m
}

/// The registry's view of the machine, built from tmux the way a probe would, with `agent` in
/// `pane`, and (`seen`) the agent's process found there.
async fn put_in_registry(host: &Host, pane: &str, agent: Option<&str>, seen: bool) {
    let out = tmux(host, "list-panes -a -F '#{session_id} #{session_name} #{window_id} #{window_index} #{pane_id} #{pane_pid}'").await;
    let rows: Vec<Vec<String>> = out.lines().map(|l| l.split(' ').map(str::to_owned).collect()).collect();
    let me = rows.iter().find(|r| r[4] == pane).unwrap().clone();
    let mut sessions: IndexMap<String, (String, IndexMap<String, Vec<serde_json::Value>>)> = IndexMap::new();
    for r in &rows {
        let s = sessions.entry(r[0].clone()).or_insert_with(|| (r[1].clone(), IndexMap::new()));
        let mut p = json!({ "id": r[4], "index": 0, "active": true, "command": "sh", "cwd": "/", "title": "", "width": 80, "height": 20, "dead": false });
        if r[4] == pane {
            if let Some(kind) = agent {
                p["agent"] = json!({ "kind": kind, "pid": 1, "state": "idle" });
            }
        }
        s.1.entry(r[2].clone()).or_default().push(p);
    }
    let sessions: Vec<serde_json::Value> = sessions
        .into_iter()
        .map(|(id, (name, windows))| {
            let windows: Vec<serde_json::Value> =
                windows.into_iter().map(|(wid, panes)| json!({ "id": wid, "index": 0, "name": "w", "active": true, "activity": 0, "panes": panes })).collect();
            json!({ "id": id, "name": name, "attached": 0, "windows": windows })
        })
        .collect();
    let state = serde_json::from_value(
        json!({ "config": local_config("end"), "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "updatedAt": 0, "sessions": sessions }),
    )
    .unwrap();
    let mut rt = HostRuntime::new(host.clone(), state);
    rt.hold_polls();
    if seen && agent.is_some() {
        let pid: f64 = me[5].parse().unwrap();
        let mut procs = IndexMap::new();
        procs.insert(pane.to_owned(), AgentProc { kind: AgentKind::Claude, pid, started_at: 0.0, cwd: None, resume: None, args: String::new(), pane_pid: pid, custom: None });
        rt.probe = Some(Rc::new(RefCell::new(probe(procs))));
    }
    registry().insert_runtime("end", Rc::new(RefCell::new(rt)));
}

fn probe(agent_procs: IndexMap<String, AgentProc>) -> ProbeResult {
    ProbeResult {
        hostname: "end".into(),
        home: "/tmp".into(),
        os: "Linux".into(),
        now: 0.0,
        latency_ms: 0.0,
        tmux: TmuxProbe::Ok,
        integration: IntegrationState::Missing,
        sessions: vec![],
        agent_procs,
        hook_panes: IndexMap::new(),
        status_panes: IndexMap::new(),
        account: None,
        compact_panes: IndexMap::new(),
        installed: vec![],
        signed_in: vec![],
        agy_conversations: IndexMap::new(),
        agy_status: IndexMap::new(),
        agy_titles: IndexMap::new(),
        agy_integration: None,
        hooked: vec![],
        machine_kind: None,
        keys_at: Default::default(),
        unwatched: Default::default(),
        claude_parked: Default::default(),
    }
}

async fn start(host: &Host, target: &str, cmd: &str) -> String {
    tmux(host, &format!("{target} -P -F '#{{pane_id}}' {}", inyon_core::hosts::host::shq(cmd))).await
}

fn closed(c: Closes) -> EndSession {
    EndSession { closed: c }
}

#[test]
fn a_split_window_keeps_its_other_pane_and_other_windows_and_sessions_are_left_alone() {
    let own = Own::new("end-split");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        let a = start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        let b = start(&host, "split-window -t work:0", "sleep 600").await;
        start(&host, "new-window -t work", "sleep 600").await;
        start(&host, "new-session -d -s other", "sleep 600").await;
        put_in_registry(&host, &b, None, false).await;
        assert_eq!(end_session("end", &b).await.unwrap(), closed(Closes::Pane));
        let l = layout(&host).await;
        let mut keys: Vec<&str> = l.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["other:0", "work:0", "work:1"]);
        assert_eq!(l["work:0"], [a]);
        assert_eq!((l["work:1"].len(), l["other:0"].len()), (1, 1));
    });
}

#[test]
fn a_windows_last_pane_takes_the_window_not_the_session() {
    let own = Own::new("end-window");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        let b = start(&host, "new-window -t work", "sleep 600").await;
        put_in_registry(&host, &b, None, false).await;
        assert_eq!(end_session("end", &b).await.unwrap(), closed(Closes::Window));
        assert_eq!(layout(&host).await.keys().cloned().collect::<Vec<_>>(), ["work:0"]);
    });
}

#[test]
fn a_sessions_last_window_takes_the_session_and_only_that_one() {
    let own = Own::new("end-session");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        let a = start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        start(&host, "new-session -d -s other", "sleep 600").await;
        put_in_registry(&host, &a, None, false).await;
        assert_eq!(end_session("end", &a).await.unwrap(), closed(Closes::Session));
        assert_eq!(layout(&host).await.keys().cloned().collect::<Vec<_>>(), ["other:0"]);
    });
}

/// a stand-in agent: reads lines, notes /exit and leaves when asked
fn obedient(log: &std::path::Path) -> String {
    format!("sh -c 'while read l; do [ \"$l\" = /exit ] && echo exited > {} && exit 0; done'", log.display())
}

#[test]
fn an_agent_is_asked_to_exit_first_then_its_pane_closes() {
    let own = Own::new("end-agent");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        let log = own.dir.join("agent.log");
        let a = start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        let b = start(&host, "split-window -t work:0", &obedient(&log)).await;
        put_in_registry(&host, &b, Some("claude"), true).await;
        assert_eq!(end_session("end", &b).await.unwrap(), closed(Closes::Pane));
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "exited");
        assert_eq!(layout(&host).await, IndexMap::from([("work:0".to_owned(), vec![a])]));
    });
}

#[test]
fn an_agent_whose_process_the_hub_never_saw_is_gone_as_soon_as_its_pane_is() {
    let own = Own::new("end-unseen");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        let log = own.dir.join("agent.log");
        let a = start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        let b = start(&host, "split-window -t work:0", &obedient(&log)).await;
        // no process to ask kill -0 about: only tmux can say the pane is gone, and tmux display answers
        // with nothing, and success, for a pane that no longer exists
        put_in_registry(&host, &b, Some("claude"), false).await;
        let t0 = Instant::now();
        assert_eq!(end_session("end", &b).await.unwrap(), closed(Closes::Pane));
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "exited");
        assert_eq!(layout(&host).await, IndexMap::from([("work:0".to_owned(), vec![a])]));
    });
}

#[test]
fn an_agent_that_wont_exit_goes_with_its_pane() {
    let own = Own::new("end-stubborn");
    run_core(|| async {
        let host = own_tmux(&own, "end").await;
        let a = start(&host, "new-session -d -s work -x 80 -y 20", "sleep 600").await;
        // ignores /exit and Ctrl+C
        let b = start(&host, "split-window -t work:0", r#"sh -c 'trap "" INT; while :; do read l; done'"#).await;
        put_in_registry(&host, &b, Some("claude"), true).await;
        assert_eq!(end_session("end", &b).await.unwrap(), closed(Closes::Pane));
        assert_eq!(layout(&host).await, IndexMap::from([("work:0".to_owned(), vec![a])]));
    });
}

// ---------- integration.test.ts: a restart reads the real argument list ----------

#[test]
fn a_restart_keeps_a_flag_value_with_spaces_in_it_whole_read_from_the_real_argument_list() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
        let host = Host::new(local_config("local"));
        let id = "7409eef9-d507-4808-a592-0bfabe896e3a";
        // a stand-in agent started with a settings file in a folder with a space
        let mut proc = std::process::Command::new("sh").args(["-c", "sleep 30", "claude", "--settings", "/work/my project/settings.json", "--model", "sonnet"]).spawn().unwrap();
        let pid = proc.id() as f64;
        // just after the spawn the process can still show the parent's arguments
        let mut args = argv(&host, pid).await;
        for _ in 0..20 {
            if args.as_ref().is_some_and(|a| a.iter().any(|x| x == "--settings")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            args = argv(&host, pid).await;
        }
        let _ = proc.kill();
        let _ = proc.wait();
        if cfg!(target_os = "linux") {
            assert_eq!(
                resume_command_argv(AgentKind::Claude, Some(id), &args.unwrap()),
                format!("claude --resume {id} '--settings' '/work/my project/settings.json' '--model' 'sonnet'")
            );
        } else {
            assert_eq!(args, None);
        }
    });
}

// ---------- mirror.test.ts, end to end ----------

#[test]
fn a_terminal_on_a_pane_shows_its_screen_types_into_it_and_gives_the_window_back_when_closed() {
    let own = Own::new("mirror");
    run_core(|| async {
        let host = own_tmux(&own, "mirror").await;
        let pane = tmux(&host, r#"new-session -d -s mirror -x 80 -y 20 -P -F '#{pane_id}' "printf 'ready\\n'; exec cat -v""#).await;
        let shown = Rc::new(RefCell::new(String::new()));
        let s = shown.clone();
        let term = open_mirror(host.clone(), &pane, 60, 15, move |d| s.borrow_mut().push_str(&String::from_utf8_lossy(&d)), None);
        let size = |h: Host, p: String| async move { h.run(&format!("tmux display -p -t {p} '#{{window_width}}x#{{window_height}}'")).await.unwrap().stdout.trim().to_owned() };
        let on_screen = |h: Host, p: String| async move { h.run(&format!("tmux capture-pane -p -t {p}")).await.unwrap().stdout };
        let sh = shown.clone();
        until(
            move || {
                let ok = sh.borrow().contains("ready");
                Box::pin(async move { ok })
            },
            5000,
        )
        .await;
        // the window takes the phone's size while the terminal is open
        assert_eq!(size(host.clone(), pane.clone()).await, "60x15");

        term.write("hello world\r");
        let (h, p) = (host.clone(), pane.clone());
        // the echo, and cat's copy
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { on_screen(h, p).await.split("hello world").count() == 3 })
            },
            5000,
        )
        .await;
        let sh = shown.clone();
        until(
            move || {
                let ok = sh.borrow().split("hello world").count() >= 3;
                Box::pin(async move { ok })
            },
            5000,
        )
        .await;

        term.write("\x1b[A");
        let (h, p) = (host.clone(), pane.clone());
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { on_screen(h, p).await.contains("^[[A") })
            },
            5000,
        )
        .await;
        term.write("\x1b[200~pasted text\x1b[201~\r");
        let (h, p) = (host.clone(), pane.clone());
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { on_screen(h, p).await.split("pasted text").count() == 3 })
            },
            5000,
        )
        .await;

        term.resize(40, 10);
        let (h, p) = (host.clone(), pane.clone());
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { size(h, p).await == "40x10" })
            },
            5000,
        )
        .await;

        term.kill();
        assert_eq!(term.exited.wait().await, 0);
        let (h, p) = (host.clone(), pane.clone());
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { h.run(&format!("tmux show -wv -t {p} window-size")).await.unwrap().stdout.trim() != "manual" })
            },
            5000,
        )
        .await;
    });
}

#[test]
fn the_terminal_ends_when_its_pane_goes_away() {
    let own = Own::new("mirror-gone");
    run_core(|| async {
        let host = own_tmux(&own, "mirror").await;
        let p = tmux(&host, "new-session -d -s gone -P -F '#{pane_id}' 'sleep 600'").await;
        let term = open_mirror(host.clone(), &p, 40, 10, |_| {}, None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        tmux(&host, "kill-session -t gone").await;
        assert!(tokio::time::timeout(Duration::from_secs(5), term.exited.wait()).await.is_ok(), "still open");
    });
}

// ---------- locale.test.ts ----------

/// A machine reached over SSH often has no locale at all. tmux then wrote its field separator as
/// "_" and the machine listed no sessions, though two were running. The locale is taken away as
/// SSH would, and HOME moved so the probe writes nothing in the real one.
async fn no_locale(own: &Own) -> Host {
    let host = own_tmux_with(own, "locale", &format!("unset LANG LC_CTYPE LC_MESSAGES; export LC_ALL=C HOME='{}'; ", own.dir.display())).await;
    tmux(&host, r#"new-session -d -s 'tëst' -x 80 -y 20 "printf 'räksmörgås ⏺\n'; sleep 600""#).await;
    tmux(&host, "new-session -d -s second 'sleep 600'").await;
    host
}

#[test]
fn sessions_are_listed_with_their_names_intact_when_the_machine_has_no_locale() {
    let own = Own::new("locale-names");
    run_core(|| async {
        let host = no_locale(&own).await;
        let r = inyon_core::tmux::tmux::probe(&host).await.unwrap();
        assert_eq!(r.tmux, TmuxProbe::Ok);
        let mut names: Vec<String> = r.sessions.iter().map(|s| s.name.clone()).filter(|n| n != "guard").collect();
        names.sort();
        assert_eq!(names, ["second", "tëst"]);
    });
}

#[test]
fn a_panes_screen_keeps_its_non_ascii_text() {
    let own = Own::new("locale-screen");
    run_core(|| async {
        let host = no_locale(&own).await;
        let r = inyon_core::tmux::tmux::probe(&host).await.unwrap();
        let pane = r.sessions.iter().find(|s| s.name == "tëst").unwrap().windows[0].panes[0].id.clone();
        let (h, p) = (host.clone(), pane.clone());
        until(
            move || {
                let (h, p) = (h.clone(), p.clone());
                Box::pin(async move { screen(&h, &p).await.contains("räksmörgås ⏺") })
            },
            3000,
        )
        .await;
    });
}

// ---------- the resting grid ----------

async fn window_size(host: &Host, w: &str) -> String {
    tmux(host, &format!("display -p -t {w} '#{{window_width}}x#{{window_height}}'")).await
}

#[test]
fn a_window_nobody_shows_rests_wide_and_the_next_client_still_sizes_it() {
    let own = Own::new("grid");
    run_core(|| async {
        let host = own_tmux(&own, "grid").await;
        let w = tmux(&host, "new-session -d -s g -x 80 -y 24 -P -F '#{window_id}' 'sleep 600'").await;
        host.run(&rest_command(std::slice::from_ref(&w))).await.unwrap();
        assert_eq!(window_size(&host, &w).await, "200x50");
        assert_eq!(tmux(&host, &format!("show -wv -t {w} window-size")).await, "", "the option is back to the server's");

        // someone attaches at 120×30: the window takes their size, as it would without Inyon
        let client = host.pty("tmux attach -t g", 120, 30, |_| {}).unwrap();
        let (h, w2) = (host.clone(), w.clone());
        until(
            move || {
                let (h, w2) = (h.clone(), w2.clone());
                Box::pin(async move { window_size(&h, &w2).await.starts_with("120x") })
            },
            5000,
        )
        .await;
        client.kill();
        // wait for the client to be gone: one still leaving on a busy machine can resize the next
        // windows (seen with tmux 3.4 in CI)
        let h = host.clone();
        until(
            move || {
                let h = h.clone();
                Box::pin(async move { tmux(&h, "list-clients -F '#{client_name}'").await.is_empty() })
            },
            5000,
        )
        .await;

        // a window someone gave a window-size of its own is left as it is
        let m = tmux(&host, "new-session -d -s m -x 80 -y 24 -P -F '#{window_id}' 'sleep 600'").await;
        tmux(&host, &format!("set-option -w -t {m} window-size smallest")).await;
        host.run(&rest_command(std::slice::from_ref(&m))).await.unwrap();
        assert_eq!(window_size(&host, &m).await, "80x24");
        assert_eq!(tmux(&host, &format!("show -wv -t {m} window-size")).await, "smallest");

        // and none rest when every window is sized by hand
        let n = tmux(&host, "new-session -d -s n -x 80 -y 24 -P -F '#{window_id}' 'sleep 600'").await;
        tmux(&host, "set-option -g window-size manual").await;
        host.run(&rest_command(std::slice::from_ref(&n))).await.unwrap();
        assert_eq!(window_size(&host, &n).await, "80x24");
    });
}
