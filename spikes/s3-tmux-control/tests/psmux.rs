//! psmux (tmux for Windows) over the same client. Runs only when S3_PSMUX names the psmux binary
//! (the Windows CI job sets it). It records which control-mode features answer and prints a
//! table; only the basics Inyon already relies on (attach, list-panes, %output, send-keys) fail
//! the test.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use s3_tmux_control::tmux::{self, Tmux};
use s3_tmux_control::{Control, Notification};

const SOCKET: &str = "inyon-s3";
const SESSION: &str = "s3";

fn output_until(rx: &Receiver<Notification>, needle: &str, timeout: Duration) -> Option<Vec<u8>> {
    let end = Instant::now() + timeout;
    let mut buf = Vec::new();
    while Instant::now() < end {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Notification::Output { data, .. } | Notification::ExtendedOutput { data, .. }) => {
                buf.extend(data);
                if buf.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    return Some(buf);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    None
}

#[test]
fn psmux_control_mode() {
    let Some(bin) = std::env::var_os("S3_PSMUX").map(PathBuf::from) else {
        println!("S3_PSMUX not set, skipped");
        return;
    };
    let mut t = Tmux::new(bin);
    t.socket = Some(SOCKET.into());
    t.utf8 = false;
    println!("version: {:?}", t.version());
    let _ = t.run(["kill-server"]);
    let out = t.run(["new-session", "-d", "-s", SESSION, "-x", "120", "-y", "30"]).unwrap();
    assert!(out.status.success(), "new-session: {}", String::from_utf8_lossy(&out.stderr));
    std::thread::sleep(Duration::from_secs(2));

    // the form Inyon's hub uses (live.ts): -C attach-session -t <session>; psmux also reads
    // PSMUX_SESSION_NAME, set to the same session in case it ignores -t
    let mut cmd = t.command();
    cmd.env("PSMUX_SESSION_NAME", format!("{SOCKET}__{SESSION}"));
    cmd.args(["-C", "attach-session", "-t", SESSION]);
    let (c, rx) = Control::spawn(cmd).unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let mut table: Vec<(&str, String)> = Vec::new();
    let mut note = |name: &'static str, r: String| {
        println!("{name}: {r}");
        table.push((name, r));
    };

    // tmux answers the attach-session it was started with in a block with flags 0
    note("attach answered (flags 0 block)", format!("{}", c.wait_ready(Duration::from_secs(3))));
    // replies line up only if psmux sends no block of its own on attach
    let sync = c.cmd("display-message -p S3-SYNC");
    note("reply order", format!("{:?}", sync.as_ref().map(|r| r.text())));
    if sync.as_ref().map(|r| r.text().trim() != "S3-SYNC").unwrap_or(true) {
        // one more try: an attach block sent with flags 1 would shift every reply by one
        note("reply order (retry)", format!("{:?}", c.cmd("display-message -p S3-SYNC2").map(|r| r.text())));
    }
    let panes = tmux::list_panes(&c);
    note(
        "list-panes -a -F (hub format)",
        format!("{:?}", panes.as_ref().map(|p| p.iter().map(|p| format!("{} {} {}x{}", p.pane_id, p.command, p.width, p.height)).collect::<Vec<_>>())),
    );
    let pane = panes.as_ref().ok().and_then(|p| p.first()).map(|p| p.pane_id.clone()).unwrap_or_else(|| "%1".into());
    note("list-sessions", format!("{:?}", c.cmd("list-sessions -F '#{session_id} #{session_name}'").map(|r| r.text())));
    note("#{version}", format!("{:?}", c.cmd("display-message -p '#{version}'").map(|r| r.text())));

    let typed = tmux::type_text(&c, &pane, "echo ('S3-' + 6*7)").and_then(|_| tmux::send_keys(&c, &pane, &["Enter"]));
    note("send-keys -l + Enter", format!("{typed:?}"));
    let out = output_until(&rx, "S3-42", Duration::from_secs(15));
    note("%output after typing", out.as_ref().map(|b| format!("seen ({} bytes)", b.len())).unwrap_or_else(|| "not seen".into()));

    let cap = tmux::capture(&c, &pane);
    note(
        "capture-pane -p -e",
        format!("{:?}", cap.as_ref().map(|rows| (rows.len(), rows.iter().any(|r| String::from_utf8_lossy(r).contains("S3-42")), rows.iter().any(|r| r.contains(&0x1b))))),
    );
    note("paste (set-buffer + paste-buffer -p)", format!("{:?}", tmux::paste(&c, &pane, "# pasted\nline two")));
    // the hub's way on Windows: load-buffer from a file, then paste-buffer -p
    let file = std::env::temp_dir().join("inyon-s3-paste.txt");
    std::fs::write(&file, "# loaded\nline two").unwrap();
    let loaded = c.cmd(format!("load-buffer -b s3 {}", s3_tmux_control::escape::quote(&file.to_string_lossy())));
    note("load-buffer <file>", format!("{:?}", loaded.as_ref().map(|r| r.text())));
    note("paste-buffer -p", format!("{:?}", c.cmd(format!("paste-buffer -d -p -b s3 -t {pane}")).map(|r| r.text())));
    // a paste could also go in as raw bytes with the brackets written by hand
    note("send-keys -H", format!("{:?}", c.cmd(format!("send-keys -t {pane} -H 23 20 68 65 78")).map(|r| r.text())));
    note("resize-window", format!("{:?}", tmux::resize_window(&c, &pane, 100, 30)));
    note("resize-pane", format!("{:?}", tmux::resize_pane(&c, &pane, 100, 25)));
    note("refresh-client -C 120,30", format!("{:?}", c.cmd("refresh-client -C 120,30").map(|r| r.text())));
    note("refresh-client -f pause-after=2", format!("{:?}", c.cmd("refresh-client -f pause-after=2").map(|r| r.text())));
    note("refresh-client -B subscription", format!("{:?}", c.cmd("refresh-client -B 'cmd:%*:#{pane_current_command}'").map(|r| r.text())));
    note(
        "list-commands",
        format!(
            "{:?}",
            c.cmd("list-commands").map(|r| r.lines.iter().map(|l| String::from_utf8_lossy(l).split(' ').next().unwrap_or("").to_string()).collect::<Vec<_>>().join(" "))
        ),
    );
    note("stray lines (echo)", c.stats.stray.load(std::sync::atomic::Ordering::Relaxed).to_string());

    let killed = t.run(["kill-session", "-t", SESSION]);
    let mut exit = None;
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Notification::Exit { reason }) => exit = Some(format!("%exit {reason:?}")),
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                exit.get_or_insert_with(|| "EOF without %exit".into());
                break;
            }
            Err(_) => break,
        }
    }
    note("kill-session", format!("{:?} -> {}", killed.map(|o| o.status.success()), exit.unwrap_or_else(|| "client still open".into())));
    let _ = t.run(["kill-server"]);

    println!("\n| feature | psmux answer |\n|---|---|");
    for (k, v) in &table {
        println!("| {k} | {} |", v.replace('|', "/"));
    }
    assert!(panes.is_ok(), "list-panes over control mode failed");
    assert!(typed.is_ok(), "send-keys over control mode failed");
    assert!(out.is_some(), "no %output for typed text");
}
