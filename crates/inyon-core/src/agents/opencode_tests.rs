//! agents/opencode.test.ts, test for test. Each test's name in the TS file is its doc comment. The
//! client's tests send real requests with curl on a sandboxed machine to a stand-in server on
//! loopback, as the TS sends them to Bun.serve.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::agents::opencode_api::{open_code_agents_from, open_code_catalog_from_providers, open_code_model, opencode_request, OpenCodeClient, OpenCodeModel, OpenCodeSelection};
use crate::agents::opencode_controls::{live_children, live_value, selection_from_footer, selection_from_messages, stored_selection, stored_selection_of};
use crate::claude::transcript::new_meta;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{assert_match, host, rt, TempDir};
use indexmap::IndexSet;
use inyon_protocol::{AgentModel, ModeOption};
use serde_json::json;

/// One request the stand-in server was sent.
#[derive(Debug, Clone)]
struct Asked {
    method: String,
    /// with its query
    path: String,
    body: Option<Value>,
}

type Respond = dyn Fn(&str, &str) -> (u16, String) + Send + Sync;

/// A stand-in OpenCode server on loopback that records what it was asked and answers through
/// `respond(method, pathname)`.
struct FakeServer {
    port: u16,
    calls: Arc<Mutex<Vec<Asked>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeServer {
    fn start(respond: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (calls, stop) = (Arc::new(Mutex::new(vec![])), Arc::new(AtomicBool::new(false)));
        let (c, st, respond): (_, _, Box<Respond>) = (calls.clone(), stop.clone(), Box::new(respond));
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(conn) = conn {
                    Self::serve(conn, &c, &*respond);
                }
            }
        });
        FakeServer { port, calls, stop, thread: Some(thread) }
    }

    /// answers `routes` ("METHOD /path", else "/path") as JSON, anything else with a 404
    fn routes(routes: Vec<(&str, Value)>) -> FakeServer {
        let routes: Vec<(String, Value)> = routes.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        Self::start(move |method, path| {
            let key = format!("{method} {path}");
            match routes.iter().find(|(k, _)| *k == key).or_else(|| routes.iter().find(|(k, _)| k == path)) {
                Some((_, v)) => (200, v.to_string()),
                None => (404, String::new()),
            }
        })
    }

    fn serve(conn: TcpStream, calls: &Mutex<Vec<Asked>>, respond: &Respond) {
        let mut r = BufReader::new(conn);
        let mut line = String::new();
        if r.read_line(&mut line).is_err() {
            return;
        }
        let mut words = line.split_whitespace();
        let (method, path) = (words.next().unwrap_or("").to_owned(), words.next().unwrap_or("").to_owned());
        let mut length = 0;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0; length];
        let _ = r.read_exact(&mut body);
        let text = String::from_utf8_lossy(&body);
        let body = if method == "GET" || text.is_empty() { None } else { serde_json::from_str(&text).ok() };
        let pathname = path.split('?').next().unwrap_or("").to_owned();
        calls.lock().unwrap().push(Asked { method: method.clone(), path, body });
        let (status, answer) = respond(&method, &pathname);
        let mut conn = r.into_inner();
        let _ = write!(conn, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}", answer.len());
    }

    fn uri(&self) -> String {
        format!("opencode://127.0.0.1:{}/ses_one", self.port)
    }

    fn calls(&self) -> Vec<Asked> {
        self.calls.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Runs `test` with every machine a shell in a temporary home (the TS uses its own machine).
fn on_machine<F: std::future::Future<Output = ()>>(test: impl FnOnce(crate::hosts::host::Host) -> F) {
    let home = TempDir::new("inyon-opencode-");
    rt::run(|| async {
        Sandbox::install(&home.s());
        test(host("test-opencode", "local")).await;
    });
}

fn set(ids: &[&str]) -> IndexSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn list(v: Value) -> Vec<Value> {
    v.as_array().unwrap().clone()
}

/// an OpenCode session's subagents count as background work, and their questions as waiting
#[test]
fn an_opencode_sessions_subagents_count_as_background_work_and_their_questions_as_waiting() {
    let states = json!({ "ses_root": { "type": "idle" }, "ses_a": { "type": "busy" }, "ses_b": { "type": "retry" }, "ses_other": { "type": "busy" } });
    let children = set(&["ses_a", "ses_b"]);
    let v = live_value(&states, &json!([]), "ses_root", &children);
    assert_match(&v, json!({ "working": false, "background": "2 agents" }));
    assert_eq!(v.permission, None);
    let mut one = states.clone();
    one["ses_b"] = json!({ "type": "idle" });
    assert_eq!(live_value(&one, &json!([]), "ses_root", &children).background.as_deref(), Some("1 agent"));
    assert_eq!(live_value(&json!({ "ses_root": { "type": "busy" } }), &json!([]), "ses_root", &set(&[])).background, None);
    // a question from a subagent is this session's to answer; another session's is not
    assert_eq!(live_value(&states, &json!([{ "id": "p1", "sessionID": "ses_a" }]), "ses_root", &children).permission.unwrap()["id"], "p1");
    assert_eq!(live_value(&states, &json!([{ "id": "p2", "sessionID": "ses_other" }]), "ses_root", &children).permission, None);
}

/// OpenCode HTTP failures are never accepted as a delivered message
#[test]
fn opencode_http_failures_are_never_accepted_as_a_delivered_message() {
    on_machine(|h| async move {
        let server = FakeServer::start(|_, _| (400, json!({ "error": "secret-provider-key" }).to_string()));
        assert!(!opencode_post(&h, &format!("http://127.0.0.1:{}/session/bad/prompt_async", server.port), &json!({})).await);
        let err = opencode_request(&h, &format!("http://127.0.0.1:{}/session", server.port), "GET", None, 12).await.unwrap_err().message;
        assert!(err.contains("HTTP 400") && !err.contains("secret-provider-key"), "{err}");
        let err = opencode_request(&h, "http://example.com:80/session", "GET", None, 12).await.unwrap_err().message;
        assert!(err.contains("local loopback"), "{err}");
    });
    // refused before anything runs on the machine
    rt::run(|| async {
        let fake = crate::test_support::fake::Fake::install(|_, _, _| crate::test_support::fake::ok(""));
        assert!(opencode_request(&host("test-opencode", "local"), "http://example.com:80/session", "GET", None, 12).await.is_err());
        assert!(fake.cmds().is_empty());
    });
}

/// OpenCode async prompts preserve provider, model slash, reasoning variant and directory
#[test]
fn opencode_async_prompts_preserve_provider_model_slash_reasoning_variant_and_directory() {
    on_machine(|h| async move {
        let s = FakeServer::routes(vec![("POST /session/ses_one/prompt_async", json!({}))]);
        let client = OpenCodeClient::new(&h, &s.uri(), Some("/tmp/a b")).unwrap();
        let selection = OpenCodeSelection { model: Some("openrouter/vendor/model".into()), variant: Some("high".into()), agent: Some("plan".into()) };
        client.prompt("hello", &selection, &[]).await.unwrap();
        let calls = s.calls();
        assert_eq!(calls[0].path, "/session/ses_one/prompt_async?directory=%2Ftmp%2Fa%20b");
        assert_eq!(
            calls[0].body,
            Some(json!({ "model": { "providerID": "openrouter", "modelID": "vendor/model" }, "variant": "high", "agent": "plan", "parts": [{ "type": "text", "text": "hello" }] }))
        );
    });
    assert!(open_code_model("ambiguous").is_err());
}

/// undo, redo, share, compact and questions go to the routes OpenCode 1.19 has for them
#[test]
fn undo_redo_share_compact_and_questions_go_to_the_routes_opencode_1_19_has_for_them() {
    on_machine(|h| async move {
        let s = FakeServer::routes(vec![
            ("POST /session/ses_one/revert", json!({ "id": "ses_one", "revert": { "messageID": "msg_2" } })),
            ("POST /session/ses_one/unrevert", json!({ "id": "ses_one" })),
            ("POST /session/ses_one/share", json!({ "id": "ses_one", "share": { "url": "https://opencode.ai/s/abc" } })),
            ("DELETE /session/ses_one/share", json!({ "id": "ses_one" })),
            ("POST /session/ses_one/summarize", json!(true)),
            (
                "GET /question",
                json!([{ "id": "que_1", "sessionID": "ses_one", "questions": [{ "question": "Tabs or spaces?", "header": "Indentation", "options": [{ "label": "Tabs", "description": "" }] }] }]),
            ),
            ("POST /question/que_1/reply", json!(true)),
            ("POST /tui/select-session", json!(true)),
            ("POST /instance/dispose", json!(true)),
        ]);
        let client = OpenCodeClient::new(&h, &s.uri(), Some("/w")).unwrap();
        assert_eq!(client.revert("msg_2").await.unwrap().unwrap()["revert"]["messageID"], "msg_2");
        client.unrevert().await.unwrap();
        assert_eq!(client.share().await.unwrap().unwrap()["share"]["url"], "https://opencode.ai/s/abc");
        client.unshare().await.unwrap();
        client.summarize("opencode/big-pickle").await.unwrap();
        assert_eq!(client.questions().await.unwrap().unwrap()[0]["id"], "que_1");
        client.reply_question("que_1", &[vec!["Tabs".into()]]).await.unwrap();
        client.select_session("ses_two").await.unwrap();
        client.dispose().await.unwrap();
        let calls = s.calls();
        assert_eq!(
            calls.iter().map(|c| format!("{} {}", c.method, c.path.split('?').next().unwrap())).collect::<Vec<_>>(),
            [
                "POST /session/ses_one/revert",
                "POST /session/ses_one/unrevert",
                "POST /session/ses_one/share",
                "DELETE /session/ses_one/share",
                "POST /session/ses_one/summarize",
                "GET /question",
                "POST /question/que_1/reply",
                "POST /tui/select-session",
                "POST /instance/dispose",
            ]
        );
        assert_eq!(calls[0].body, Some(json!({ "messageID": "msg_2" })));
        assert_eq!(calls[4].body, Some(json!({ "providerID": "opencode", "modelID": "big-pickle" })));
        assert_eq!(calls[6].body, Some(json!({ "answers": [["Tabs"]] })));
        assert_eq!(calls[7].body, Some(json!({ "sessionID": "ses_two" })));
        // every call carries the session's folder, which the server keys its state by
        assert!(calls.iter().all(|c| c.path.ends_with("?directory=%2Fw")));
    });
}

#[test]
fn a_model_id_splits_at_its_first_slash_into_provider_and_model() {
    assert_eq!(open_code_model("openrouter/vendor/model"), Ok(OpenCodeModel { provider_id: "openrouter".into(), model_id: "vendor/model".into() }));
    assert_eq!(serde_json::to_value(open_code_model("opencode/big-pickle").unwrap()).unwrap(), json!({ "providerID": "opencode", "modelID": "big-pickle" }));
    assert!(open_code_model("ambiguous").is_err());
    assert!(open_code_model("/x").is_err());
    assert!(open_code_model("x/").is_err());
}

/// catalog exposes only connected provider models and does not leak provider credentials
#[test]
fn catalog_exposes_only_connected_provider_models_and_does_not_leak_provider_credentials() {
    let catalog = open_code_catalog_from_providers(
        &json!({ "connected": ["one"], "all": [
            { "id": "one", "name": "First", "key": "SECRET", "options": { "apiKey": "SECRET" }, "models": { "m": { "id": "m", "name": "Model", "variants": { "low": {}, "high": {}, "disabled": { "disabled": true } }, "limit": { "context": 123 } } } },
            { "id": "two", "models": { "m": { "id": "m" } } },
        ] }),
        0,
    );
    assert_eq!(
        catalog.models,
        vec![AgentModel {
            id: "one/m".into(),
            label: "First · Model".into(),
            note: None,
            context: Some(Num(123.0)),
            efforts: Some(vec!["low".into(), "high".into()]),
            default_effort: None,
            variants: None
        }]
    );
    assert!(!serde_json::to_string(&catalog).unwrap().contains("SECRET"));
}

/// the agents a session can switch to are the primary ones, custom included, never OpenCode's hidden helpers
#[test]
fn the_agents_a_session_can_switch_to_are_the_primary_ones_custom_included_never_opencodes_hidden_helpers() {
    let agents = open_code_agents_from(&json!([
        { "name": "build", "mode": "primary" }, { "name": "plan", "mode": "primary" }, { "name": "reviewer", "mode": "all" },
        { "name": "explore", "mode": "subagent" }, { "name": "title", "mode": "primary", "hidden": true },
    ]));
    let mode = |id: &str, label: &str| ModeOption { id: id.into(), label: label.into() };
    assert_eq!(agents, vec![mode("build", "Build"), mode("plan", "Plan"), mode("reviewer", "Reviewer")]);
}

/// session discovery rejects ambiguous or mismatched cwd conversations, unless the terminal title names one
#[test]
fn session_discovery_rejects_ambiguous_or_mismatched_cwd_conversations_unless_the_terminal_title_names_one() {
    let sessions = list(json!([{ "id": "a", "directory": "/a", "title": "first" }, { "id": "b", "directory": "/b", "title": "other" }]));
    assert_eq!(select_open_code_session(&sessions, Some("/a"), None, None).as_deref(), Some("a"));
    assert_eq!(select_open_code_session(&sessions, Some("/missing"), None, None), None);
    let mut two = sessions.clone();
    two.extend(list(
        json!([{ "id": "c", "directory": "/a", "title": "second", "time": { "updated": 5 } }, { "id": "d", "directory": "/a", "title": "second", "time": { "updated": 9 } }]),
    ));
    assert_eq!(select_open_code_session(&two, Some("/a"), None, None), None);
    assert_eq!(select_open_code_session(&two, Some("/a"), None, Some("first")).as_deref(), Some("a"));
    // two with the same title: the one that moved last
    assert_eq!(select_open_code_session(&two, Some("/a"), None, Some("second")).as_deref(), Some("d"));
    assert_eq!(select_open_code_session(&two, Some("/a"), None, Some("nope")), None);
    assert_eq!(select_open_code_session(&sessions, Some("/a"), Some("b"), None), None);
    assert_eq!(select_open_code_session(&sessions, Some("/a"), Some("a"), None).as_deref(), Some("a"));
    assert_eq!(titled_session(Some("OC | pong")).as_deref(), Some("pong"));
    assert_eq!(titled_session(Some("OpenCode")), None);
}

/// session metrics count user turns rather than every intermediate tool response and preserve zero cost
#[test]
fn session_metrics_count_user_turns_rather_than_every_intermediate_tool_response_and_preserve_zero_cost() {
    let mut meta = new_meta();
    let mut messages = list(
        json!([{ "info": { "id": "u", "role": "user", "model": { "providerID": "one", "modelID": "m" }, "agent": "plan", "variant": "high", "time": { "created": 100 } }, "parts": [{ "type": "text", "text": "hello" }] }]),
    );
    for id in [1, 2] {
        messages
            .push(json!({ "info": { "id": format!("a{id}"), "role": "assistant", "providerID": "one", "modelID": "m", "cost": 0, "time": { "completed": 110 } }, "parts": [] }));
    }
    opencode_items(&messages, &mut meta);
    assert_eq!(meta.stats.turns, 1);
    assert_eq!(meta.stats.cost_usd, Some(Num(0.0)));
    assert_match(&meta.status, json!({ "model": "one/m", "effort": "high" }));
}

/// the context is the last reply's whole usage, reasoning and cache included, as the TUI's sidebar counts it
#[test]
fn the_context_is_the_last_replys_whole_usage_reasoning_and_cache_included_as_the_tuis_sidebar_counts_it() {
    let mut meta = new_meta();
    opencode_items(
        &list(json!([
            { "info": { "id": "u", "role": "user", "time": { "created": 1 } }, "parts": [{ "type": "text", "text": "hi" }] },
            { "info": { "id": "a1", "role": "assistant", "time": { "created": 2, "completed": 3 }, "tokens": { "input": 14049, "output": 3, "reasoning": 0, "cache": { "read": 1792, "write": 0 } } }, "parts": [] },
            // an aborted reply has no usage worth counting
            { "info": { "id": "a2", "role": "assistant", "time": { "created": 4 }, "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }, "parts": [] },
        ])),
        &mut meta,
    );
    assert_eq!(meta.stats.context_tokens, Some(Num(15844.0)));
    assert_eq!(meta.stats.context_at, Some(3));
    let mut total = new_meta();
    opencode_items(
        &list(
            json!([{ "info": { "id": "a", "role": "assistant", "time": { "created": 2 }, "tokens": { "total": 36533, "input": 14666, "output": 74, "reasoning": 1441, "cache": { "read": 20352, "write": 0 } } }, "parts": [] }]),
        ),
        &mut total,
    );
    assert_eq!(total.stats.context_tokens, Some(Num(36533.0)));
}

/// direct OpenCode shell produces a delivery receipt and provider errors are visible
#[test]
fn direct_opencode_shell_produces_a_delivery_receipt_and_provider_errors_are_visible() {
    let rows = opencode_items(
        &list(json!([
            { "info": { "id": "u", "role": "user", "time": { "created": 1000 } }, "parts": [{ "type": "text", "synthetic": true, "text": "The following tool was executed by the user" }] },
            { "info": { "id": "a", "parentID": "u", "role": "assistant", "time": { "created": 1100 }, "error": { "data": { "message": "Provider is unavailable" } } }, "parts": [{ "id": "p", "type": "tool", "tool": "bash", "state": { "input": { "command": "printf hello" }, "status": "completed", "output": "hello" } }] },
        ])),
        &mut new_meta(),
    );
    assert_match(&rows[0], json!({ "kind": "user", "shell": true, "text": "printf hello" }));
    let rows = serde_json::to_value(&rows).unwrap();
    assert!(rows.as_array().unwrap().iter().any(|r| r["kind"] == "notice" && r["text"] == "Provider is unavailable"));
}

/// a shell receipt with no id is never paired with a reply that has no parent
#[test]
fn a_shell_receipt_with_no_id_is_never_paired_with_a_reply_that_has_no_parent() {
    let ran = json!({ "type": "text", "synthetic": true, "text": "The following tool was executed by the user" });
    let reply = |parent: Option<&str>| {
        let mut info = json!({ "id": format!("msg_r{}", parent.unwrap_or("")), "role": "assistant" });
        if let Some(p) = parent {
            info["parentID"] = json!(p);
        }
        json!({ "info": info, "parts": [{ "type": "tool", "tool": "bash", "callID": "c1", "state": { "status": "completed", "input": { "command": "ls" }, "output": "a.ts" } }] })
    };
    let users = |rows: Vec<ChatItem>| serde_json::to_value(rows).unwrap().as_array().unwrap().iter().filter(|r| r["kind"] == "user").cloned().collect::<Vec<_>>();
    let mut meta = new_meta();
    assert_eq!(users(opencode_items(&[json!({ "info": { "role": "user" }, "parts": [ran.clone()] }), reply(None)], &mut meta)), Vec::<Value>::new());
    // with ids, the receipt still finds its reply
    let rows = users(opencode_items(&[json!({ "info": { "id": "msg_u", "role": "user" }, "parts": [ran] }), reply(None), reply(Some("msg_u"))], &mut meta));
    assert_match(&rows, json!([{ "text": "ls", "shell": true }]));
}

/// what a message runs with comes from the footer first, then the session's record, then the last message
#[test]
fn what_a_message_runs_with_comes_from_the_footer_first_then_the_sessions_record_then_the_last_message() {
    let model = |id: &str, label: &str, efforts: &[&str]| AgentModel {
        id: id.into(),
        label: label.into(),
        note: None,
        context: None,
        efforts: Some(efforts.iter().map(|s| s.to_string()).collect()),
        default_effort: None,
        variants: None,
    };
    let models =
        [model("opencode/deepseek-v4.1-flash-free", "OpenCode · DeepSeek V4.1 Flash Free", &["low", "high", "max"]), model("opencode/big-pickle", "OpenCode · Big Pickle", &[])];
    let sel = |m: &str, v: Option<&str>, a: Option<&str>| OpenCodeSelection { model: Some(m.into()), variant: v.map(str::to_owned), agent: a.map(str::to_owned) };
    assert_eq!(
        selection_from_footer(Some("DeepSeek V4.1 Flash Free OpenCode"), Some("max"), Some("plan"), &models),
        Some(sel("opencode/deepseek-v4.1-flash-free", Some("max"), Some("plan")))
    );
    assert_eq!(selection_from_footer(Some("Big Pickle OpenCode"), None, Some("build"), &models), Some(sel("opencode/big-pickle", None, Some("build"))));
    // a footer naming a model the catalogue doesn't know (or no footer at all) decides nothing
    assert_eq!(selection_from_footer(Some("Mystery Model Acme"), None, Some("build"), &models), None);
    assert_eq!(selection_from_footer(None, None, Some("build"), &models), None);
    let info = json!({ "id": "ses_one", "agent": "plan", "model": { "id": "deepseek-v4.1-flash-free", "providerID": "opencode", "variant": "max" } });
    assert_eq!(stored_selection(Some(&info)), sel("opencode/deepseek-v4.1-flash-free", Some("max"), Some("plan")));
    on_machine(|h| async move {
        let s = FakeServer::routes(vec![("GET /session/ses_one", info)]);
        assert_eq!(stored_selection_of(&OpenCodeClient::new(&h, &s.uri(), Some("/w")).unwrap()).await, sel("opencode/deepseek-v4.1-flash-free", Some("max"), Some("plan")));
        assert_eq!(s.calls()[0].path, "/session/ses_one?directory=%2Fw");
    });
    assert_eq!(stored_selection(None), OpenCodeSelection::default());
    assert_eq!(
        selection_from_messages(&list(json!([
            { "info": { "id": "one", "role": "user", "model": { "providerID": "p", "modelID": "old", "variant": "high" }, "agent": "build" } },
            { "info": { "id": "two", "role": "user", "model": { "providerID": "p", "modelID": "native" } } },
        ]))),
        sel("p/native", None, Some("build"))
    );
}

#[test]
fn the_endpoint_is_found_from_the_sockets_the_process_and_its_children_hold() {
    let ss = "LISTEN 0 512 127.0.0.1:4096 0.0.0.0:* users:((\"opencode\",pid=42,fd=20))\nLISTEN 0 512 127.0.0.1:5000 0.0.0.0:* users:((\"node\",pid=99,fd=3))\nLISTEN 0 512 [::1]:4097 [::]:* users:((\"bun\",pid=43,fd=9))\nLISTEN 0 512 127.0.0.1:4096 0.0.0.0:* users:((\"opencode\",pid=42,fd=21))";
    let probe = opencode_endpoint_probe(42, ss, "opencode\0-s\0ses_abc\0", "43\n");
    assert_eq!(probe, EndpointProbe { ports: vec!["4096".into(), "4097".into()], explicit: Some("ses_abc".into()) });
    assert_eq!(opencode_endpoint_probe(42, "", "opencode\0--session=ses_x\0", "").explicit.as_deref(), Some("ses_x"));
    assert_eq!(opencode_endpoint_commands(0), None);
    let sessions = json!([{ "id": "ses_abc", "directory": "/w" }, { "id": "ses_b", "directory": "/w" }]);
    assert_eq!(opencode_port_endpoint("4096", &sessions, Some("/w"), Some("ses_abc"), None).as_deref(), Some("opencode://127.0.0.1:4096/ses_abc"));
    assert_eq!(opencode_port_endpoint("4096", &sessions, Some("/w"), None, Some(" OpenCode ")).as_deref(), Some("opencode://127.0.0.1:4096/"));
    assert_eq!(opencode_port_endpoint("4096", &sessions, Some("/w"), None, None), None);
    assert_eq!(opencode_port_endpoint("4096", &json!({}), Some("/w"), None, None), None);
    assert_eq!(parse_endpoint("opencode://127.0.0.1:4096/ses_abc"), Some(Endpoint { base: "http://127.0.0.1:4096".into(), session: "ses_abc".into() }));
    assert_eq!(parse_endpoint("opencode://10.0.0.1:4096/x"), None);
    assert!(via_open_code_api(Some("opencode://127.0.0.1:1/")));
    assert!(!via_open_code_api(Some("opencode-db:ses_a")));
    assert_eq!(live_children(&json!([{ "id": "c1", "parentID": "ses_abc" }, { "id": "c2", "parentID": "x" }, { "parentID": "ses_abc" }]), "ses_abc"), set(&["c1"]));
}

/// review: a null at the end of a `??` chain falls through to the default, as in the TS
#[test]
fn a_null_error_or_output_reads_as_the_default_not_as_null() {
    let rows = opencode_items(
        &list(json!([
            { "info": { "id": "a", "role": "assistant", "time": { "created": 1100 }, "error": { "name": null } }, "parts": [{ "id": "p", "type": "tool", "tool": "bash", "state": { "input": { "command": "false" }, "status": "error", "error": null } }] },
        ])),
        &mut new_meta(),
    );
    let rows = serde_json::to_value(&rows).unwrap();
    assert_match(&rows, json!([{ "kind": "notice", "text": "OpenCode request failed" }, { "kind": "assistant" }, { "kind": "tool_result", "text": "", "isError": true }]));
}
