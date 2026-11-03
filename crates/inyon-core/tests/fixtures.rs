//! Every shared capture through the Rust readers, compared with fixtures/expected: what the TS
//! readers made of them while the Bun hub existed (until 0.23), and what INYON_BLESS=1 writes for
//! a capture added since. Also the chat.screen wire samples, rebuilt from the captures they came from.

use std::path::{Path, PathBuf};

use inyon_core::claude::screen::{codex_browsing, parse_screen, prompt_summary};
use inyon_protocol::AgentKind;
use serde_json::{json, Value};

/// Date.now() while the readers run (EXPECTED_NOW in the Bun hub's expected.ts)
const EXPECTED_NOW: u64 = 1_792_320_000_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn agent(name: &str) -> AgentKind {
    serde_json::from_value(Value::String(name.to_owned())).unwrap_or_else(|_| panic!("{name} isn't an agent"))
}

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

/// The same object `screenExpectation` builds in TS.
fn expectation(agent: AgentKind, lines: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    for (key, width) in [("auto", None), ("80", Some(80)), ("120", Some(120))] {
        let screen = parse_screen(lines, width, agent);
        let mut run = json!({ "screen": screen });
        if let Some(p) = &screen.prompt {
            run["summary"] = Value::String(prompt_summary(p));
        }
        out.insert(key.into(), run);
    }
    if agent == AgentKind::Codex {
        out.insert("browsing".into(), serde_json::to_value(codex_browsing(lines)).unwrap());
    }
    Value::Object(out)
}

/// Where two values first differ, as a path ("auto.screen.prompt.options[1].label").
fn first_difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            // a field one side leaves out and the other has, even as null, is a difference: on the
            // wire null and missing mean different things (ScreenState.background, .remote)
            keys.into_iter().find_map(|k| match (x.get(k), y.get(k)) {
                (Some(a), Some(b)) => first_difference(&format!("{path}.{k}"), a, b),
                (Some(a), None) => Some(format!("{path}.{k}: rust {a} · ts missing")),
                (None, Some(b)) => Some(format!("{path}.{k}: rust missing · ts {b}")),
                (None, None) => None,
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_difference(&format!("{path}[{i}]"), p, q)),
        _ if a == b => None,
        _ => Some(format!("{path}: rust {a} · ts {b}")),
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).unwrap().split('\n').map(str::to_owned).collect()
}

#[test]
fn every_capture_reads_as_the_ts_readers_read_it() {
    inyon_core::clock::set_now(Some(EXPECTED_NOW));
    let mut checked = 0;
    let mut failures = vec![];
    for dir in sorted(&fixtures().join("screens")) {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let kind = agent(&name);
        for file in sorted(&dir) {
            let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
            let expected_path = fixtures().join(format!("expected/screens/{name}/{stem}.json"));
            let got = expectation(kind, &read_lines(&file));
            let Ok(text) = std::fs::read_to_string(&expected_path) else {
                // a new capture: INYON_BLESS=1 writes what the readers make of it, to look over and commit
                if std::env::var_os("INYON_BLESS").is_some() {
                    std::fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
                    std::fs::write(&expected_path, format!("{}\n", serde_json::to_string_pretty(&got).unwrap())).unwrap();
                    checked += 1;
                } else {
                    failures.push(format!("{name}/{stem}: no fixtures/expected file (INYON_BLESS=1 cargo test -p inyon-core --test fixtures writes it)"));
                }
                continue;
            };
            let want: Value = serde_json::from_str(&text).unwrap();
            checked += 1;
            if let Some(d) = first_difference("", &got, &want) {
                failures.push(format!("{name}/{stem}{d}"));
            }
        }
    }
    assert!(checked > 70, "only {checked} captures found");
    assert!(failures.is_empty(), "{} of {checked} captures differ:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn every_expected_file_has_its_capture() {
    for dir in sorted(&fixtures().join("expected/screens")) {
        for file in sorted(&dir) {
            let capture = fixtures().join("screens").join(dir.file_name().unwrap()).join(file.file_stem().unwrap()).with_extension("txt");
            assert!(capture.exists(), "{} has no capture", file.display());
        }
    }
}

fn without_since(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("since");
    }
    v
}

#[test]
fn chat_screen_samples_come_out_of_their_captures() {
    inyon_core::clock::set_now(Some(EXPECTED_NOW));
    let sources: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(fixtures().join("expected/chat-screen.json")).unwrap()).unwrap();
    assert!(!sources.is_empty());
    let mut failures = vec![];
    for s in &sources {
        let sample = s["sample"].as_str().unwrap();
        let capture = s["capture"].as_str().unwrap_or_else(|| panic!("{sample} has no capture"));
        let (agent_name, _) = capture.split_once('/').unwrap();
        let mut text = std::fs::read_to_string(fixtures().join(format!("screens/{capture}.txt"))).unwrap();
        if let Some(r) = s["replace"].as_array() {
            text = text.replacen(r[0].as_str().unwrap(), r[1].as_str().unwrap(), 1);
        }
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let width = s["width"].as_u64().map(|w| w as u32);
        let msg: Value = serde_json::from_str(&std::fs::read_to_string(fixtures().join(format!("protocol/ServerMsg/{sample}"))).unwrap()).unwrap();
        let got = serde_json::to_value(parse_screen(&lines, width, agent(agent_name))).unwrap();
        if let Some(d) = first_difference("", &without_since(&got), &without_since(&msg["screen"])) {
            failures.push(format!("{sample}{d}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} samples differ:\n{}", failures.len(), sources.len(), failures.join("\n"));
}

#[test]
fn a_null_on_one_side_and_nothing_on_the_other_is_a_difference() {
    let a = serde_json::json!({ "screen": { "background": null, "remote": false } });
    let b = serde_json::json!({ "screen": { "remote": false } });
    assert_eq!(first_difference("", &a, &b).as_deref(), Some(".screen.background: rust null · ts missing"));
    assert_eq!(first_difference("", &b, &a).as_deref(), Some(".screen.background: rust missing · ts null"));
    assert_eq!(first_difference("", &a, &a), None);
}
