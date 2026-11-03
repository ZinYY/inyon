//! The edges of the JSON mapping that the samples in fixtures/protocol don't all reach.

mod common;

use common::round_trip;
use inyon_protocol::*;
use serde_json::json;

#[test]
fn whole_numbers_are_written_as_javascript_writes_them() {
    assert_eq!(serde_json::to_string(&Num(42.0)).unwrap(), "42");
    assert_eq!(serde_json::to_string(&Num(-0.0)).unwrap(), "0");
    assert_eq!(serde_json::to_string(&Num(4.25)).unwrap(), "4.25");
    assert_eq!(serde_json::to_string(&Num(f64::NAN)).unwrap(), "null");
    round_trip::<RateWindow>(&json!({ "pct": 13, "resetsAt": 1792040400.5 })).unwrap();
}

#[test]
fn null_and_left_out_stay_apart() {
    let base = json!({ "working": false, "width": 80 });
    for (field, value) in [("remote", json!(null)), ("remote", json!("https://x.y/r")), ("background", json!(null)), ("background", json!("1 shell"))] {
        let mut v = base.clone();
        v[field] = value;
        round_trip::<ScreenState>(&v).unwrap();
    }
    round_trip::<ScreenState>(&base).unwrap();
    let s: ScreenState = serde_json::from_value(json!({ "working": true, "width": 80, "background": null })).unwrap();
    assert_eq!((s.background, s.remote), (Some(None), None));
}

#[test]
fn literal_types_reject_other_values() {
    round_trip::<PairedPhone>(&json!({ "id": "a", "name": "b", "pairedAt": 1, "lastSeen": 2, "thisPhone": true })).unwrap();
    assert!(serde_json::from_value::<PairedPhone>(json!({ "id": "a", "name": "b", "pairedAt": 1, "lastSeen": 2, "thisPhone": false })).is_err());
    assert!(serde_json::from_value::<Stale>(json!({ "limits": "plan" })).is_err());
    assert!(serde_json::from_value::<ServerMsg>(json!({ "t": "chat.nope", "key": "a:b" })).is_err());
}

#[test]
fn messages_are_tagged_like_the_typescript_unions() {
    let m = ClientMsg::TermOpen { id: "t1".into(), host: "local".into(), pane: "%1".into(), cols: 80, rows: 24 };
    assert_eq!(serde_json::to_value(&m).unwrap(), json!({ "t": "term.open", "id": "t1", "host": "local", "pane": "%1", "cols": 80, "rows": 24 }));
    let item = ChatItem::TurnEnd { seq: 4, uuid: "u".into(), ts: "t".into(), duration_ms: Num(1500.0) };
    assert_eq!(serde_json::to_value(&item).unwrap(), json!({ "kind": "turn_end", "seq": 4, "uuid": "u", "ts": "t", "durationMs": 1500 }));
    assert_eq!((item.seq(), item.uuid()), (4, "u"));
    assert_eq!(serde_json::to_value(ServerMsg::Pong).unwrap(), json!({ "t": "pong" }));
}

#[test]
fn record_keys_keep_their_order() {
    let meta: ToolResultMeta = serde_json::from_str(r#"{"answers":{"Colour":"Red","Animal":"Cat","Ber":"x"}}"#).unwrap();
    assert_eq!(serde_json::to_string(&meta).unwrap(), r#"{"answers":{"Colour":"Red","Animal":"Cat","Ber":"x"}}"#);
}

#[test]
fn a_number_written_as_null_reads_back_as_nan() {
    // JSON.stringify writes NaN (a tmux field that wasn't a number) as null
    assert!(serde_json::from_str::<Num>("null").unwrap().0.is_nan());
    assert_eq!(serde_json::from_str::<Option<Num>>("null").unwrap(), None);
    let pane: PaneInfo =
        serde_json::from_value(json!({ "id": "%1", "index": 0, "active": true, "command": "zsh", "cwd": "/", "title": "t", "width": null, "height": 40, "dead": false })).unwrap();
    assert!(pane.width.0.is_nan());
}
