//! agents/opencode-db.test.ts, test for test: each first checks the rows such a database gives
//! back (the cache, the session picker, the stamp, the shape and the session row), then runs the
//! reader's real shell script (sqlite3 or python3, whichever the machine has) against a SQLite file
//! laid out like OpenCode's, as the TS does.

use super::*;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{host, rt, sh, TempDir};
use serde_json::json;

const S: &str = "ses_test0000000000000000001";
const OLDER: &str = "ses_test0000000000000000002";

/// A home whose OpenCode data (XDG_DATA_HOME) holds the TS test's database, built with Python's
/// sqlite3: two conversations in /work, the first with a user message whose summary carries two
/// megabytes of diff.
fn database() -> TempDir {
    let root = TempDir::new("inyon-ocdb-");
    let script = format!(
        r#"import json, sqlite3
db = sqlite3.connect("{}/opencode/opencode.db")
db.executescript("""create table session (id text primary key, title text, directory text, parent_id text, time_created integer, time_updated integer, time_archived integer, agent text, model text, revert text, cost real, summary_additions integer, summary_deletions integer, summary_files integer, share_url text);
create table message (id text primary key, session_id text, time_created integer, time_updated integer, data text);
create table part (id text primary key, message_id text, session_id text, time_created integer, time_updated integer, data text)""")
db.execute("insert into session values ('{S}', 'Greeting', '/work', null, 1000, 5000, null, 'build', 'x/y', null, 0.5, 3, 1, 2, 'https://opencode.ai/s/abc')")
db.execute("insert into session values ('{OLDER}', 'Older talk', '/work', null, 500, 900, null, null, null, null, 0, null, null, null, null)")
diff = "x" * 2000000
db.execute("insert into message values (?, ?, ?, ?, ?)", ("msg_1", "{S}", 1000, 1000, json.dumps({{"role": "user", "time": {{"created": 1000}}, "summary": {{"diffs": [{{"file": "Cargo.lock", "patch": diff}}]}}}})))
db.execute("insert into message values (?, ?, ?, ?, ?)", ("msg_2", "{S}", 2000, 5000, json.dumps({{"role": "assistant", "time": {{"created": 2000}}, "modelID": "y", "providerID": "x", "cost": 0.5}})))
db.execute("insert into part values (?, ?, ?, ?, ?, ?)", ("prt_1", "msg_1", "{S}", 1000, 1000, json.dumps({{"type": "text", "text": "hi"}})))
db.execute("insert into part values (?, ?, ?, ?, ?, ?)", ("prt_2", "msg_2", "{S}", 2000, 2000, json.dumps({{"type": "text", "text": 'hello "there"\nline | two'}})))
db.commit()
"#,
        root.s()
    );
    std::fs::create_dir_all(root.path("opencode")).unwrap();
    let (code, _, err) = sh("python3 -", Some(script.as_bytes()), &[]);
    assert_eq!(code, 0, "{err}");
    root
}

/// a machine whose OpenCode data lives in `data_dir`
fn machine(data_dir: &str) {
    Sandbox::install_with(data_dir, &format!("export XDG_DATA_HOME={}; ", crate::hosts::host::shq(data_dir)));
}

fn sessions() -> Vec<Value> {
    vec![
        json!({ "id": OLDER, "title": "Older talk", "directory": "/work", "parent_id": null, "time_created": 500, "time_updated": 900, "agent": null, "model": null }),
        json!({ "id": S, "title": "Greeting", "directory": "/work", "parent_id": null, "time_created": 1000, "time_updated": 5000, "agent": "build", "model": "x/y" }),
    ]
}

/// a conversation is read in milliseconds, whatever its diff summaries weigh
#[test]
fn a_conversation_is_read_in_milliseconds_whatever_its_diff_summaries_weigh() {
    // the rows as the two queries return them (the summary was left in the database)
    let messages = vec![
        json!({ "id": "msg_1", "time_created": 1000, "time_updated": 1000, "data": "{\"role\":\"user\",\"time\":{\"created\":1000}}" }),
        json!({ "id": "msg_2", "time_created": 2000, "time_updated": 5000, "data": "{\"role\":\"assistant\",\"time\":{\"created\":2000},\"modelID\":\"y\",\"providerID\":\"x\",\"cost\":0.5}" }),
    ];
    let parts = vec![
        json!({ "id": "prt_2", "message_id": "msg_2", "time_created": 2000, "time_updated": 2000, "data": "{\"type\":\"text\",\"text\":\"hello \\\"there\\\"\\nline | two\"}" }),
        json!({ "id": "prt_1", "message_id": "msg_1", "time_created": 1000, "time_updated": 1000, "data": "{\"type\":\"text\",\"text\":\"hi\"}" }),
        json!({ "id": "prt_bad", "message_id": "msg_1", "time_created": 1, "time_updated": 1, "data": "not json" }),
    ];
    let mut cache = OcdbCache::new();
    cache.merge(&messages, &parts);
    assert_eq!(cache.stamp, 5000.0);
    let list = cache.list(None);
    let texts: Vec<&str> = list.iter().map(|m| m["parts"][0]["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["hi", "hello \"there\"\nline | two"]);
    assert_eq!(list[1]["info"], json!({ "role": "assistant", "time": { "created": 2000 }, "modelID": "y", "providerID": "x", "cost": 0.5, "id": "msg_2" }));
    assert_eq!(list[0]["parts"][0]["messageID"], "msg_1");
    // /undo back to msg_2 hides it and what came after
    assert_eq!(cache.list(Some("msg_2")).len(), 1);
    cache.retain(&HashSet::from(["msg_2".to_owned()]));
    assert_eq!(cache.list(None).len(), 1);
    let (m, p) = ocdb_rows_queries(S, 2000.7).unwrap();
    assert!(m.contains("json_remove(data, '$.summary')") && m.contains("time_updated >= 2000 "));
    assert!(p.starts_with("select json_group_array(json_object('id', id, 'message_id', message_id, "));

    let root = database();
    rt::run(|| async {
        machine(&root.s());
        let h = host("ocdb", "local");
        let ids = |rows: &[Value]| rows.iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let t0 = std::time::Instant::now();
        let (messages, parts) = ocdb_rows_of(&h, S, 0.0).await.unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_secs(4), "{:?}", t0.elapsed());
        assert_eq!(ids(&messages), ["msg_1", "msg_2"]);
        assert_eq!(ids(&parts), ["prt_1", "prt_2"]);
        // the summary stays in the database: nothing in the chat shows it
        assert_eq!(serde_json::from_str::<Value>(messages[0]["data"].as_str().unwrap()).unwrap(), json!({ "role": "user", "time": { "created": 1000 } }));
        let mut cache = OcdbCache::new();
        cache.merge(&messages, &parts);
        assert_eq!(cache.stamp, 5000.0);
        let texts: Vec<String> = cache.list(None).iter().map(|m| m["parts"][0]["text"].as_str().unwrap().to_owned()).collect();
        assert_eq!(texts, ["hi", "hello \"there\"\nline | two"]);
        // only what moved since the last read
        assert_eq!(ids(&ocdb_rows_of(&h, S, 2000.0).await.unwrap().0), ["msg_2"]);
    });
}

/// the folder's conversations come newest first, and the pane title names the one on screen
#[test]
fn the_folders_conversations_come_newest_first_and_the_pane_title_names_the_one_on_screen() {
    let rows = ocdb_sessions(&sessions());
    assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [S, OLDER]);
    let proc = OpenCodeProc { cwd: Some("/work".into()), started_at: 0.0, args: None };
    assert_eq!(guess_open_code_session(&proc, &rows, None, Some("OC | Older talk")), Some(format!("{OCDB}{OLDER}")));
    assert_eq!(guess_open_code_session(&proc, &rows, None, Some("OpenCode")).as_deref(), Some(OCDB));
    assert!(ocdb_sessions_query("/it's", 30).contains("directory = '/it''s'"));

    let root = database();
    rt::run(|| async {
        machine(&root.s());
        let h = host("ocdb", "local");
        let rows = ocdb_sessions_of(&h, "/work", 30).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), [S, OLDER]);
        assert_eq!(find_open_code_session(&h, &proc, None, Some("OC | Older talk")).await.unwrap(), Some(format!("{OCDB}{OLDER}")));
        assert_eq!(find_open_code_session(&h, &proc, None, Some("OpenCode")).await.unwrap().as_deref(), Some(OCDB));
    });
}

#[test]
fn without_a_title_it_goes_by_the_session_flag_the_screen_and_what_moved() {
    let rows = ocdb_sessions(&sessions());
    let proc = |args: &str, started_at: f64| OpenCodeProc { cwd: Some("/work".into()), started_at, args: Some(args.into()) };
    assert_eq!(guess_open_code_session(&OpenCodeProc { cwd: None, ..proc("", 0.0) }, &rows, None, None), None);
    assert_eq!(guess_open_code_session(&proc("opencode -s ses_abc", 0.0), &rows, None, None), Some(format!("{OCDB}ses_abc")));
    let screen = vec!["  Older talk".to_owned(), "> ".to_owned()];
    assert_eq!(guess_open_code_session(&proc("", 0.0), &rows, Some(&screen), None), Some(format!("{OCDB}{OLDER}")));
    // nothing on screen: the one that moved since the process started
    assert_eq!(guess_open_code_session(&proc("", 4.0), &rows, None, None), Some(format!("{OCDB}{S}")));
    assert_eq!(guess_open_code_session(&proc("", 100.0), &rows, None, None).as_deref(), Some(OCDB));
    assert_eq!(guess_open_code_session(&proc("opencode -c", 100.0), &rows, None, None), Some(format!("{OCDB}{S}")));
}

/// stamp, shape and the session row (title, cost, lines changed, share link)
#[test]
fn stamp_shape_and_the_session_row_title_cost_lines_changed_share_link() {
    assert_eq!(ocdb_stamp(&[json!({ "t": 5000 })]), 5000.0);
    let shape = ocdb_shape(&[json!({ "id": "msg_1" }), json!({ "id": "msg_2" })], &[json!({ "revert": null })]);
    assert_eq!(shape.ids, HashSet::from(["msg_1".to_owned(), "msg_2".to_owned()]));
    assert_eq!(shape.revert, None);
    assert_eq!(ocdb_shape(&[], &[json!({ "revert": "{\"messageID\":\"msg_2\"}" })]).revert.as_deref(), Some("msg_2"));
    let info = ocdb_session_info(&[
        json!({ "title": "Greeting", "cost": 0.5, "summary_additions": 3, "summary_deletions": 1, "summary_files": 2, "share_url": "https://opencode.ai/s/abc" }),
    ]);
    assert_eq!(
        info,
        Some(OcdbSessionInfo {
            title: Some("Greeting".into()),
            cost: Some(0.5),
            additions: Some(3.0),
            deletions: Some(1.0),
            files: Some(2.0),
            share: Some("https://opencode.ai/s/abc".into())
        })
    );
    let older = ocdb_session_info(&[json!({ "title": "Older talk", "cost": 0, "summary_additions": null, "summary_deletions": null, "summary_files": null, "share_url": null })]);
    assert_eq!(older, Some(OcdbSessionInfo { title: Some("Older talk".into()), cost: Some(0.0), ..Default::default() }));
    assert_eq!(ocdb_rows_queries("ses_nothing'; drop", 0.0), None);
    assert_eq!(ocdb_stamp_query("nope"), None);

    let root = database();
    rt::run(|| async {
        machine(&root.s());
        let h = host("ocdb", "local");
        assert_eq!(ocdb_stamp_of(&h, S).await.unwrap(), 5000.0);
        let shape = ocdb_shape_of(&h, S).await.unwrap();
        assert_eq!(shape.ids, HashSet::from(["msg_1".to_owned(), "msg_2".to_owned()]));
        assert_eq!(shape.revert, None);
        assert_eq!(ocdb_session_info_of(&h, S).await.unwrap(), info);
        assert_eq!(ocdb_session_info_of(&h, OLDER).await.unwrap(), older);
        assert_eq!(ocdb_rows_of(&h, "ses_nothing", 0.0).await.unwrap(), (vec![], vec![]));
    });
}

/// a machine without the database answers with nothing, not an error
#[test]
fn a_machine_without_the_database_answers_with_nothing_not_an_error() {
    assert_eq!(ocdb_query_rows("@@NODB\n"), Vec::<Value>::new());
    assert_eq!(ocdb_query_rows(""), Vec::<Value>::new());
    assert_eq!(ocdb_sessions(&ocdb_query_rows("@@NODB")), vec![]);
    assert_eq!(ocdb_stamp(&[]), 0.0);
    // `opencode db` wraps the array in a result set
    assert_eq!(ocdb_query_rows("[{\"rows\":\"[{\\\"t\\\":7}]\"}]"), vec![json!({ "t": 7 })]);
    assert_eq!(ocdb_query_rows("[{\"t\":7}]"), vec![json!({ "t": 7 })]);
    assert!(ocdb_query_command("select 1").starts_with("Q='select 1'\n"));
    assert!(is_ocdb(Some("opencode-db:ses_a")) && !is_ocdb(None));
    assert_eq!(ocdb_session("opencode-db:ses_a1"), Some("ses_a1"));
    assert_eq!(ocdb_session("opencode-db:"), None);

    let bare = TempDir::new("inyon-ocdb-bare-");
    rt::run(|| async {
        machine(&bare.s());
        let h = host("bare", "local");
        assert_eq!(ocdb_sessions_of(&h, "/work", 30).await.unwrap(), vec![]);
        assert_eq!(ocdb_stamp_of(&h, S).await.unwrap(), 0.0);
    });
}

/// a row with no time_updated leaves the stamp where it was
#[test]
fn a_row_with_no_time_updated_leaves_the_stamp_where_it_was() {
    let mut cache = OcdbCache::new();
    cache.merge(
        &[
            json!({ "id": "msg_1", "time_created": 1000, "data": "{\"role\":\"user\"}" }),
            json!({ "id": "msg_2", "time_created": 2000, "time_updated": 3000, "data": "{\"role\":\"assistant\"}" }),
        ],
        &[json!({ "id": "prt_1", "message_id": "msg_2", "time_created": 2000, "time_updated": null, "data": "{\"type\":\"text\",\"text\":\"hi\"}" })],
    );
    assert_eq!(cache.stamp, 3000.0);
    cache.merge(&[], &[json!({ "id": "prt_2", "message_id": "msg_2", "time_created": 2100, "time_updated": 4000, "data": "{\"type\":\"text\",\"text\":\"more\"}" })]);
    assert_eq!(cache.stamp, 4000.0);
    let list = cache.list(None);
    let shape: Vec<(Value, usize)> = list.iter().map(|m| (m["info"]["id"].clone(), m["parts"].as_array().unwrap().len())).collect();
    assert_eq!(shape, vec![(json!("msg_1"), 0), (json!("msg_2"), 2)]);
}
