//! Loads every file under the repository's fixtures/ folder, so a file in the wrong place or one
//! that doesn't parse fails here as well as in the Bun tests that read the same files.

mod common;

use common::{entries, fixtures, round_trip_as};
use inyon_protocol::AGENT_KINDS;

fn agent_folders(dir: &str) -> Vec<(String, Vec<std::path::PathBuf>)> {
    let path = fixtures().join(dir);
    if !path.exists() {
        return vec![];
    }
    entries(&path)
        .into_iter()
        .map(|d| {
            let name = d.file_name().unwrap().to_string_lossy().into_owned();
            assert!(d.is_dir() && AGENT_KINDS.iter().any(|k| k.as_str() == name), "fixtures/{dir}/{name} should be a folder named after an agent");
            (name, entries(&d))
        })
        .collect()
}

#[test]
fn only_known_folders() {
    for e in entries(&fixtures()) {
        let name = e.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            ["README.md", "screens", "transcripts", "protocol", "link", "expected", "pages", "old-git", "upgrade"].contains(&name.as_str()),
            "fixtures/{name} isn't part of the layout in fixtures/README.md"
        );
    }
}

#[test]
fn screens_are_text() {
    let mut count = 0;
    for (agent, files) in agent_folders("screens") {
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.ends_with(".txt"), "fixtures/screens/{agent}/{name} should be a .txt capture");
            let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("fixtures/screens/{agent}/{name}: {e}"));
            assert!(!text.trim().is_empty(), "fixtures/screens/{agent}/{name} is empty");
            count += 1;
        }
    }
    assert!(count > 0, "no screen captures found");
}

#[test]
fn transcripts_are_json_lines() {
    for (agent, files) in agent_folders("transcripts") {
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.ends_with(".jsonl"), "fixtures/transcripts/{agent}/{name} should be .jsonl");
            for (i, line) in std::fs::read_to_string(&f).unwrap().lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| panic!("fixtures/transcripts/{agent}/{name}:{}: {e}", i + 1));
            }
        }
    }
}

/// Each sample, read as the type its folder names and written back, is the JSON it started as.
/// A file may hold one message or an array of them.
#[test]
fn protocol_samples_round_trip() {
    let mut failures = vec![];
    let mut count = 0;
    for dir in entries(&fixtures().join("protocol")) {
        let ty = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(dir.is_dir(), "fixtures/protocol/{ty} should be a folder named after a type");
        for f in entries(&dir) {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap_or_else(|e| panic!("fixtures/protocol/{ty}/{name}: {e}"));
            let samples = match value {
                serde_json::Value::Array(all) => all,
                one => vec![one],
            };
            for (i, s) in samples.iter().enumerate() {
                count += 1;
                if let Err(e) = round_trip_as(&ty, s) {
                    failures.push(format!("fixtures/protocol/{ty}/{name} #{i}: {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(count >= 40, "only {count} protocol samples");
}
