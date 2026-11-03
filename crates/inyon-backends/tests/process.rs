//! The process backend against real processes: output, input, exit codes, timeouts that take the
//! whole pipeline, streams, terminals, and the ssh command line.
//! These run /bin/sh, so they only run where there is one.

#![cfg(unix)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inyon_backends::process::ProcessBackend;
use inyon_backends::runtime::run_core;
use inyon_core::hosts::host::{ExecOptions, HostBackend};
use inyon_protocol::HostConfig;
use serde_json::json;

fn local() -> HostConfig {
    serde_json::from_value(json!({ "id": "local", "name": "local", "kind": "local" })).unwrap()
}

#[test]
fn a_command_gives_its_output_its_errors_and_its_exit_code() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let r = b.exec(&local(), "echo out; echo err >&2; exit 3", ExecOptions::default()).await.unwrap();
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (3, "out\n", "err\n"));
        let r = b.exec(&local(), "tr a-z A-Z", ExecOptions::input("shout")).await.unwrap();
        assert_eq!(r.stdout, "SHOUT");
        // nothing on stdin unless given: a command reading it ends at once
        let r = b.exec(&local(), "cat; echo done", ExecOptions::default()).await.unwrap();
        assert_eq!(r.stdout, "done\n");
    });
}

#[test]
fn a_timeout_ends_the_whole_pipeline_and_says_so() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let t0 = Instant::now();
        // the sleep holds the pipe open: killing only the shell would leave the call waiting on it
        let r = b.exec(&local(), "echo started; sleep 30 | cat", ExecOptions::timeout(300)).await.unwrap();
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert_eq!(r.code, 124);
        assert_eq!(r.stdout, "started\n");
        assert!(r.stderr.ends_with("timed out"), "{}", r.stderr);
    });
}

#[test]
fn a_timeout_also_covers_output_held_open_by_something_left_in_the_background() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let started = Instant::now();
        // the shell exits at once; the sleep it leaves keeps stdout open
        let r = b.exec(&local(), "sleep 10 & echo started", ExecOptions::timeout(500)).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (124, "started\n", "timed out"));
    });
}

#[test]
fn old_tether_variables_reach_commands_under_their_inyon_names() {
    let home = std::env::temp_dir().join(format!("inyon-tether-env-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let out = run_core(|| async {
        // the hub as a machine set up under the old name starts it: only TETHER_* is set
        let env = inyon_backends::env::HubEnv::from_vars([("HOME".to_owned(), home.display().to_string()), ("TETHER_PORT_TEST".to_owned(), "4999".to_owned())]);
        inyon_backends::env::install_hub(&env, "box").unwrap();
        // a command on this machine, through whatever backend the hub plugged in
        let host = inyon_core::hosts::host::Host::new(local());
        host.run("echo \"$INYON_PORT_TEST\"").await.unwrap().stdout
    });
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(out, "4999\n");
}

#[test]
fn a_stream_hands_on_each_line_skips_empty_ones_and_ends_when_killed() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let lines = Rc::new(RefCell::new(Vec::<String>::new()));
        let l = lines.clone();
        let h = b.stream(&local(), "printf 'one\\n\\ntwo\\nthr'; sleep 0.2; printf 'ee\\n'; sleep 30", Box::new(move |s| l.borrow_mut().push(s)), false);
        for _ in 0..100 {
            if lines.borrow().len() == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(*lines.borrow(), ["one", "two", "three"]);
        assert!(!h.exited.is_settled());
        h.kill();
        let code = tokio::time::timeout(Duration::from_secs(5), h.exited.wait()).await.expect("the stream ended");
        assert_eq!(code, 128 + 15);
    });
}

#[test]
fn a_stream_that_keeps_stdin_open_stays_up_until_killed() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let h = b.stream(&local(), "cat >/dev/null; echo eof", Box::new(|_| {}), true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!h.exited.is_settled());
        h.kill();
        tokio::time::timeout(Duration::from_secs(5), h.exited.wait()).await.expect("the stream ended");
        // and without it, cat sees the end of its input at once
        let h = b.stream(&local(), "cat >/dev/null", Box::new(|_| {}), false);
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), h.exited.wait()).await.unwrap(), 0);
    });
}

#[test]
fn a_terminal_runs_its_command_takes_typing_and_reports_its_exit() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let out = Rc::new(RefCell::new(String::new()));
        let o = out.clone();
        let t = b.pty(&local(), "printf 'ready '; read x; echo \"got $x [$TERM]\"", 80, 24, Box::new(move |d| o.borrow_mut().push_str(&String::from_utf8_lossy(&d)))).unwrap();
        for _ in 0..100 {
            if out.borrow().contains("ready") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        t.write("yo\r");
        let code = tokio::time::timeout(Duration::from_secs(5), t.exited.wait()).await.expect("the terminal's command ended");
        assert_eq!(code, 0);
        for _ in 0..50 {
            if out.borrow().contains("got yo") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(out.borrow().contains("got yo [xterm-256color]"), "{}", out.borrow());
    });
}

#[test]
fn closing_a_terminal_types_nothing_into_it() {
    // a program that outlives the kill for a moment, like a tmux client on its way out: what
    // reaches it then reaches the pane it shows, and a newline there is Enter
    let dir = std::env::temp_dir().join(format!("inyon-pty-close-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (got, pid) = (dir.join("got"), dir.join("pid"));
    let typed = run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let cmd = format!("trap '' TERM HUP; stty raw -echo; echo $$ > '{}'; exec cat > '{}'", pid.display(), got.display());
        let t = b.pty(&local(), &cmd, 80, 24, Box::new(|_| {})).unwrap();
        let started = Instant::now();
        while !std::fs::read_to_string(&pid).is_ok_and(|s| s.ends_with('\n')) {
            assert!(started.elapsed() < Duration::from_secs(5), "the terminal never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        t.kill();
        drop(t);
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::fs::read(&got).unwrap_or_default()
    });
    let cat: i32 = std::fs::read_to_string(&pid).unwrap().trim().parse().unwrap();
    // SAFETY: the cat this test started, which ignores the polite signals
    unsafe { libc::kill(cat, libc::SIGKILL) };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(typed.is_empty(), "closing the terminal typed {typed:?}");
}

#[test]
fn a_paste_into_a_terminal_that_is_not_reading_holds_nothing_up() {
    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        // raw like a full-screen agent: the terminal keeps every key, so once its buffer is full a
        // write waits for a read that never comes
        let t = b.pty(&local(), "stty raw -echo; sleep 8", 80, 24, Box::new(|_| {})).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let started = Instant::now();
        t.write(&"x".repeat(4 << 20));
        t.kill();
        assert!(started.elapsed() < Duration::from_secs(2), "the paste held up the core's thread for {:?}", started.elapsed());
        assert!(tokio::time::timeout(Duration::from_secs(5), t.exited.wait()).await.is_ok(), "the terminal never ended");
    });
}

#[test]
fn a_remote_machine_goes_through_ssh_with_one_shared_connection() {
    let b = ProcessBackend::new("/data/cm", vec![]);
    let c: HostConfig = serde_json::from_value(json!({ "id": "s", "name": "s", "kind": "ssh", "target": "me@box", "port": 2222, "identityFile": "/k/id" })).unwrap();
    assert_eq!(
        b.argv(&c, "echo 'hi'", false),
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=/data/cm/%C",
            "-o",
            "ControlPersist=600",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-p",
            "2222",
            "-i",
            "/k/id",
            "-T",
            "me@box",
            r"sh -c 'echo '\''hi'\'''",
        ]
    );
    assert_eq!(b.argv(&c, "tmux attach", true)[21], "-tt");
    assert_eq!(b.argv(&local(), "true", false), ["/bin/sh", "-c", "true"]);
}

#[test]
fn stopping_does_not_wait_for_a_blocking_call_still_in_flight() {
    let started = std::time::Instant::now();
    run_core(|| async {
        // an outgoing request that would take a long time to answer
        drop(tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_secs(20))));
        tokio::task::yield_now().await;
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
}
