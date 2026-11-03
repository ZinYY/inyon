//! No console window ever opens on Windows: every program inyon starts there is started hidden
//! (CREATE_NO_WINDOW through `sys::quiet`, `sys::console_child`, `ops::hide_window` and the like,
//! or `creation_flags` itself). A console program started from the Inyon app, which has no console,
//! would otherwise get a window of its own.
//!
//! Windows can't be asked after the fact whether a window flashed, so this reads the source: each
//! `Command::new(` outside tests has to be hidden before it's started, or say on the line above
//! that it's a builder `// hidden by the caller`, or with `// shows-window:` why it may show.

use std::fs;
use std::path::{Path, PathBuf};

/// What hides a child, in the code that runs on Windows.
const HIDES: [&str; 6] = ["quiet(", "hide_window(", "hide_window_std(", "console_child(", "console_child_std(", "creation_flags("];
/// What starts it.
const STARTS: [&str; 5] = [".spawn()", ".status()", ".output()", "wait_with_output()", "spawn_detached("];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            // test files and the test helpers only run under cargo test
            if !name.ends_with("_tests.rs") && name != "tests.rs" && name != "test_support.rs" {
                out.push(p);
            }
        }
    }
}

/// The lines before the file's `#[cfg(test)] mod …`, where its tests begin.
fn code_lines(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let test_attr = |l: &str| l.trim() == "#[cfg(test)]" || l.trim().starts_with("#[cfg(all(test,");
    let end = lines.windows(2).position(|w| test_attr(w[0]) && (w[1].trim_start().starts_with("mod ") || w[1].trim_start().starts_with("pub(crate) mod "))).unwrap_or(lines.len());
    lines[..end].to_vec()
}

/// Every `Command::new(` in `text` that's started without being hidden first, as line numbers.
fn unhidden(text: &str) -> Vec<usize> {
    let lines = code_lines(text);
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("Command::new(") || line.trim_start().starts_with("//") {
            continue;
        }
        if (i.saturating_sub(3)..i).any(|j| lines[j].contains("// shows-window:") || lines[j].contains("// hidden by the caller")) {
            continue;
        }
        // the builder, up to where it's started (or a few lines on, for one that's returned)
        let mut hidden = false;
        for l in lines.iter().skip(i).take(30) {
            if HIDES.iter().any(|h| l.contains(h)) {
                hidden = true;
                break;
            }
            if STARTS.iter().any(|s| l.contains(s)) {
                break;
            }
        }
        if !hidden {
            found.push(i + 1);
        }
    }
    found
}

#[test]
fn every_program_started_on_windows_is_started_hidden() {
    let mut files = Vec::new();
    // the hub runs on Linux and macOS only
    for dir in ["apps/cli/src", "crates/inyon-engine/src", "crates/inyon-core/src", "crates/inyon-backends/src"] {
        rust_files(&root().join(dir), &mut files);
    }
    assert!(files.len() > 20, "found the sources: {files:?}");
    let mut bad = Vec::new();
    for f in &files {
        let text = fs::read_to_string(f).unwrap();
        for n in unhidden(&text) {
            bad.push(format!("{}:{n}", f.strip_prefix(root()).unwrap_or(f).display()));
        }
    }
    assert!(bad.is_empty(), "started without CREATE_NO_WINDOW (hide it, or say why with // shows-window:):\n{}", bad.join("\n"));
}

#[test]
fn the_check_catches_a_child_started_in_plain_view() {
    let plain = "fn a() {\n    let _ = std::process::Command::new(\"cmd.exe\").arg(\"/k\").spawn();\n}\n";
    assert_eq!(unhidden(plain), vec![2]);
    let hidden = "fn a() {\n    let mut c = Command::new(\"tar\");\n    c.arg(\"x\");\n    ops::hide_window_std(&mut c);\n    c.status();\n}\n";
    assert!(unhidden(hidden).is_empty());
    // hidden after it was started doesn't count
    let late = "fn a() {\n    let mut c = Command::new(\"tar\");\n    c.status();\n    quiet(&mut c);\n}\n";
    assert_eq!(unhidden(late), vec![2]);
    let said = "fn a() {\n    // shows-window: Linux only\n    Command::new(\"systemctl\").status();\n}\n";
    assert!(unhidden(said).is_empty());
    let in_tests = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { Command::new(\"x\").spawn(); }\n}\n";
    assert!(unhidden(in_tests).is_empty());
}
