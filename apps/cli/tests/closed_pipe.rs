//! A reader that stops early (`inyon status --plain | grep -q hub`) closes the pipe while inyon is
//! still printing: inyon ends quietly, never with a panic (exit 101).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn a_reader_that_stops_after_one_line_ends_inyon_quietly_not_with_a_panic() {
    let home = std::env::temp_dir().join(format!("inyon-closed-pipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let node = home.join(".inyon").join("node");
    std::fs::create_dir_all(&node).unwrap();
    // far more than a pipe holds, so inyon is still writing when the reader goes
    let log: String = (0..60_000).map(|i| format!("2026-10-23T10:00:00Z line {i} of a long link log\n")).collect();
    std::fs::write(node.join("link.log"), log).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_inyon"))
        .args(["logs", "--role", "link", "-n", "60000"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    assert!(first.contains("line"), "{first}");
    drop(out);
    let status = child.wait().unwrap();
    let mut said = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut said).unwrap();
    let _ = std::fs::remove_dir_all(&home);
    assert!(!said.contains("panicked"), "{said}");
    assert_ne!(status.code(), Some(101), "{said}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc_sigpipe()), "ended by SIGPIPE, as `ls` would be: {status:?}");
    }
}

#[cfg(unix)]
fn libc_sigpipe() -> i32 {
    13
}
