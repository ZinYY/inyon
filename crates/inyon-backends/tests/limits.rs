//! The open-file limit the hub raises for itself, and puts back for what it starts. A test binary
//! of its own: it changes this process's limit.

#![cfg(unix)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use inyon_backends::limits::{for_children, raise_open_files};
use inyon_backends::process::ProcessBackend;
use inyon_backends::runtime::run_core;
use inyon_core::hosts::host::{ExecOptions, HostBackend};
use inyon_protocol::HostConfig;
use serde_json::json;

fn local() -> HostConfig {
    serde_json::from_value(json!({ "id": "local", "name": "local", "kind": "local" })).unwrap()
}

#[test]
fn commands_and_terminals_get_the_limit_the_hub_had_before_it_raised_its_own() {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
    // start low, as a hub started by systemd does (1024), whatever the shell running this gave
    let low: libc::rlim_t = 1000;
    assert!(lim.rlim_max > low, "the hard limit is too low to tell a raise apart");
    lim.rlim_cur = low;
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) }, 0);

    let now = raise_open_files().unwrap();
    assert!(now > 1000, "raised to {now}");
    assert_eq!(for_children(), Some(1000));

    run_core(|| async {
        let b = ProcessBackend::new("/tmp", vec![]);
        let r = b.exec(&local(), "ulimit -n", ExecOptions::default()).await.unwrap();
        assert_eq!(r.stdout.trim(), "1000", "a command");

        let out = Rc::new(RefCell::new(String::new()));
        let o = out.clone();
        let t = b.pty(&local(), "echo \"limit $(ulimit -n)\"", 80, 24, Box::new(move |d| o.borrow_mut().push_str(&String::from_utf8_lossy(&d)))).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), t.exited.wait()).await;
        for _ in 0..50 {
            if out.borrow().contains("limit ") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(out.borrow().contains("limit 1000"), "a terminal: {}", out.borrow());
    });
}
