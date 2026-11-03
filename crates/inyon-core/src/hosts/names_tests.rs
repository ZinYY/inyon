//! hosts/names.test.ts

use super::*;
use inyon_protocol::AgentKind::{Claude, Codex, Gemini};

#[test]
fn a_rename_waits_for_the_agent_then_the_agents_own_title_is_the_name_everywhere() {
    crate::test_support::rt::run(|| async {
        set_session_name("h", "s1", "Abhash Website", Some("Portfolio setup"), Some(Claude));
        // until Claude has it, Inyon shows the new name
        assert_eq!(session_name("h", Some("s1"), Some(Claude)).as_deref(), Some("Abhash Website"));
        assert_eq!(pending_name("h", Some("s1")).map(|e| e.name).as_deref(), Some("Abhash Website"));
        // Claude took it: its title shows from here on, so a rename from the phone or the keyboard wins too
        name_taken("h", "s1");
        assert_eq!(session_name("h", Some("s1"), Some(Claude)), None);
        assert_eq!(pending_name("h", Some("s1")), None);
    });
}

#[test]
fn the_automatic_name_puts_back_the_title_the_agent_made_up_then_forgets_it() {
    crate::test_support::rt::run(|| async {
        set_session_name("h", "s2", "Mine", Some("Fix the parser"), Some(Codex));
        name_taken("h", "s2");
        set_session_name("h", "s2", "", None, Some(Codex));
        assert_eq!(pending_name("h", Some("s2")).map(|e| e.name).as_deref(), Some("Fix the parser"));
        name_taken("h", "s2");
        assert_eq!(pending_name("h", Some("s2")), None);
        assert_eq!(session_name("h", Some("s2"), Some(Codex)), None);
    });
}

#[test]
fn agents_without_a_title_of_their_own_keep_the_name_in_inyon() {
    crate::test_support::rt::run(|| async {
        set_session_name("h", "s3", "Notes", None, Some(Gemini));
        assert_eq!(session_name("h", Some("s3"), Some(Gemini)).as_deref(), Some("Notes"));
        assert_eq!(pending_name("h", Some("s3")), None);
    });
}

#[test]
fn claudes_activity_mark_isnt_part_of_the_title() {
    assert_eq!(bare_title(Some("✳ Abhash Website")), "Abhash Website");
    assert_eq!(bare_title(Some("⠂ Abhash Website")), "Abhash Website");
    assert_eq!(bare_title(Some("Respond to greeting")), "Respond to greeting");
    assert_eq!(bare_title(Some("\"Quoted\" name")), "\"Quoted\" name");
}

#[test]
fn names_are_saved_and_read_back_in_the_ts_format() {
    crate::test_support::rt::run(|| async {
        use crate::platform::store::kv;
        kv::set_value("names", &serde_json::json!({ "h old": "Plain", "h s": { "name": "N", "was": "W", "pending": true } })).unwrap();
        assert_eq!(session_name("h", Some("old"), Some(Gemini)).as_deref(), Some("Plain"));
        set_session_name("h", "s4", "Four", None, Some(Gemini));
        assert_eq!(crate::js::stringify(&kv::get_value("names").unwrap()), r#"{"h old":{"name":"Plain"},"h s":{"name":"N","was":"W","pending":true},"h s4":{"name":"Four"}}"#);
    });
}
