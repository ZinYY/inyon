//! `chat.ts`: a flat transcript grouped into conversation rows, user bubbles and assistant turns.

use std::collections::HashMap;

use inyon_protocol::{AssistantBlock, ChatItem, ToolResultMeta};
use serde_json::Value;

/// One tool call in a turn, with its result once it came.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// the `tool_result` item that answered it
    pub result: Option<ChatItem>,
    /// when the call was made
    pub ts: Option<String>,
}

impl ToolCall {
    /// The result's metadata (a patch, the file written, the task it started).
    pub fn meta(&self) -> Option<&ToolResultMeta> {
        match &self.result {
            Some(ChatItem::ToolResult { meta, .. }) => meta.as_ref(),
            _ => None,
        }
    }

    pub fn done(&self) -> bool {
        self.result.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text {
        key: String,
        text: String,
    },
    Thinking {
        key: String,
        text: String,
    },
    Tools {
        key: String,
        calls: Vec<ToolCall>,
    },
    /// AskUserQuestion, ExitPlanMode, TodoWrite
    Special {
        key: String,
        call: ToolCall,
    },
    /// a `notice` item that belongs to the turn
    Notice {
        key: String,
        item: ChatItem,
    },
}

impl Part {
    pub fn key(&self) -> &str {
        match self {
            Part::Text { key, .. } | Part::Thinking { key, .. } | Part::Tools { key, .. } | Part::Special { key, .. } | Part::Notice { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistantTurn {
    pub key: String,
    pub parts: Vec<Part>,
    pub ts: String,
    pub duration_ms: Option<f64>,
    /// the turn hasn't ended yet
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// a `user` item
    User {
        key: String,
        item: ChatItem,
    },
    Assistant(AssistantTurn),
    Command {
        key: String,
        name: String,
        args: String,
        output: Option<String>,
    },
    /// a `notice` item
    Notice {
        key: String,
        item: ChatItem,
    },
    /// a `goal` item
    Goal {
        key: String,
        item: ChatItem,
    },
}

impl Row {
    pub fn key(&self) -> &str {
        match self {
            Row::User { key, .. } | Row::Command { key, .. } | Row::Notice { key, .. } | Row::Goal { key, .. } => key,
            Row::Assistant(t) => &t.key,
        }
    }
}

const SPECIAL: [&str; 3] = ["AskUserQuestion", "ExitPlanMode", "TodoWrite"];

/// Slash commands that only change a setting (the app's own controls send these). The header shows
/// the setting itself, so they aren't messages in the conversation. A goal shows as its own row.
const SETTINGS: [&str; 11] = ["model", "models", "effort", "fast", "permissions", "approvals", "mode", "agent", "agents", "output-style", "goal"];

/// Groups a flat transcript into conversation rows: user bubbles and assistant turns.
pub fn build_rows(items: &[ChatItem]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut results: HashMap<&str, &ChatItem> = HashMap::new();
    for i in items {
        if let ChatItem::ToolResult { tool_use_id, .. } = i {
            results.insert(tool_use_id, i);
        }
    }

    // the open turn, by its place in `rows`
    let mut turn: Option<usize> = None;
    // the last command was a setting, so the output after it is hidden too
    let mut hidden = false;
    fn close(rows: &mut [Row], turn: &mut Option<usize>) {
        if let Some(Row::Assistant(t)) = turn.and_then(|i| rows.get_mut(i)) {
            t.open = false;
        }
        *turn = None;
    }
    fn open<'r>(rows: &'r mut Vec<Row>, turn: &mut Option<usize>, seq: i64, ts: &str) -> &'r mut AssistantTurn {
        let i = *turn.get_or_insert_with(|| {
            rows.push(Row::Assistant(AssistantTurn { key: format!("a{seq}"), parts: Vec::new(), ts: ts.to_string(), duration_ms: None, open: true }));
            rows.len() - 1
        });
        match &mut rows[i] {
            Row::Assistant(t) => t,
            _ => unreachable!("the open turn is an assistant row"),
        }
    }
    fn current(rows: &mut [Row], turn: Option<usize>) -> Option<&mut AssistantTurn> {
        match turn.and_then(|i| rows.get_mut(i)) {
            Some(Row::Assistant(t)) => Some(t),
            _ => None,
        }
    }

    for i in items {
        if !matches!(i, ChatItem::Command { .. }) {
            hidden = false;
        }
        match i {
            ChatItem::User { seq, .. } => {
                close(&mut rows, &mut turn);
                rows.push(Row::User { key: format!("u{seq}"), item: i.clone() });
            }
            ChatItem::Assistant { seq, uuid, ts, block, .. } => {
                let t = open(&mut rows, &mut turn, *seq, ts);
                match block {
                    AssistantBlock::Text { text } => t.parts.push(Part::Text { key: uuid.clone(), text: text.clone() }),
                    AssistantBlock::Thinking { text } => t.parts.push(Part::Thinking { key: uuid.clone(), text: text.clone() }),
                    AssistantBlock::ToolUse { id, name, input } => {
                        let call =
                            ToolCall { id: id.clone(), name: name.clone(), input: input.clone(), result: results.get(id.as_str()).map(|r| (*r).clone()), ts: Some(ts.clone()) };
                        if SPECIAL.contains(&name.as_str()) {
                            t.parts.push(Part::Special { key: uuid.clone(), call });
                        } else if let Some(Part::Tools { calls, .. }) = t.parts.last_mut() {
                            calls.push(call);
                        } else {
                            t.parts.push(Part::Tools { key: uuid.clone(), calls: vec![call] });
                        }
                    }
                }
            }
            ChatItem::TurnEnd { duration_ms, .. } => {
                if let Some(t) = current(&mut rows, turn) {
                    t.duration_ms = Some(duration_ms.0);
                }
                close(&mut rows, &mut turn);
            }
            ChatItem::Command { seq, name, args, output, .. } => {
                // a setting's own output ("Set model to …") goes with it
                if name.is_empty() && hidden {
                    continue;
                }
                hidden = SETTINGS.contains(&name.trim_start_matches('/').to_lowercase().as_str());
                if hidden {
                    continue;
                }
                if name.is_empty() {
                    if let Some(Row::Command { output: prev, .. }) = rows.last_mut() {
                        if prev.is_none() {
                            prev.clone_from(output);
                            continue;
                        }
                    }
                }
                close(&mut rows, &mut turn);
                rows.push(Row::Command { key: format!("c{seq}"), name: name.clone(), args: args.clone(), output: output.clone() });
            }
            ChatItem::Goal { seq, .. } => {
                // keep it in order: replies after it start a new turn rather than joining the one above
                close(&mut rows, &mut turn);
                rows.push(Row::Goal { key: format!("g{seq}"), item: i.clone() });
            }
            ChatItem::Notice { seq, uuid, subtype, .. } => match current(&mut rows, turn) {
                Some(t) if subtype == "interrupted" => {
                    t.parts.push(Part::Notice { key: uuid.clone(), item: i.clone() });
                    close(&mut rows, &mut turn);
                }
                // a background job finishing mid-turn doesn't end the turn
                Some(t) if subtype == "task" => t.parts.push(Part::Notice { key: format!("n{seq}"), item: i.clone() }),
                _ => {
                    close(&mut rows, &mut turn);
                    rows.push(Row::Notice { key: format!("n{seq}"), item: i.clone() });
                }
            },
            ChatItem::ToolResult { .. } => {}
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::item;
    use serde_json::json;

    fn types(rows: &[Row]) -> Vec<&'static str> {
        rows.iter()
            .map(|r| match r {
                Row::User { .. } => "user",
                Row::Assistant(_) => "assistant",
                Row::Command { .. } => "command",
                Row::Notice { .. } => "notice",
                Row::Goal { .. } => "goal",
            })
            .collect()
    }
    fn turn(r: &Row) -> &AssistantTurn {
        match r {
            Row::Assistant(t) => t,
            other => panic!("expected an assistant row, got {other:?}"),
        }
    }
    fn part_types(t: &AssistantTurn) -> Vec<&'static str> {
        t.parts
            .iter()
            .map(|p| match p {
                Part::Text { .. } => "text",
                Part::Thinking { .. } => "thinking",
                Part::Tools { .. } => "tools",
                Part::Special { .. } => "special",
                Part::Notice { .. } => "notice",
            })
            .collect()
    }
    fn result_text(c: &ToolCall) -> Option<&str> {
        match &c.result {
            Some(ChatItem::ToolResult { text, .. }) => Some(text),
            _ => None,
        }
    }

    #[test]
    fn empty_transcript_no_rows() {
        assert!(build_rows(&[]).is_empty());
    }

    #[test]
    fn a_user_message_and_the_reply_become_two_rows() {
        let u = item::user("hi");
        let rows = build_rows(&[u.clone(), item::thinking("hmm"), item::text("hello"), item::end(4200.0)]);
        assert_eq!(types(&rows), ["user", "assistant"]);
        assert_eq!(rows[0].key(), format!("u{}", u.seq()));
        let a = turn(&rows[1]);
        assert_eq!(part_types(a), ["thinking", "text"]);
        assert_eq!(a.duration_ms, Some(4200.0));
        assert!(!a.open);
    }

    #[test]
    fn the_last_turn_stays_open_until_it_ends() {
        let rows = build_rows(&[item::user("hi"), item::text("working on it")]);
        assert!(turn(&rows[1]).open);
    }

    #[test]
    fn consecutive_tool_calls_share_a_group_with_their_results_matched_by_id() {
        let rows = build_rows(&[item::tool("t1"), item::tool_named("t2", "Read"), item::result("t2", "file"), item::result("t1", "out"), item::text("done"), item::tool("t3")]);
        let t = turn(&rows[0]);
        assert_eq!(part_types(t), ["tools", "text", "tools"]);
        let Part::Tools { calls, .. } = &t.parts[0] else { panic!() };
        let got: Vec<_> = calls.iter().map(|c| (c.id.as_str(), c.name.as_str(), result_text(c))).collect();
        assert_eq!(got, [("t1", "Bash", Some("out")), ("t2", "Read", Some("file"))]);
        // t3 hasn't answered yet
        let Part::Tools { calls, .. } = &t.parts[2] else { panic!() };
        assert!(calls[0].result.is_none());
    }

    #[test]
    fn questions_plans_and_todos_stand_on_their_own_and_split_tool_groups() {
        let rows = build_rows(&[
            item::tool("t1"),
            item::tool_with("q", "AskUserQuestion", json!({ "questions": [] })),
            item::tool("t2"),
            item::tool_named("p", "ExitPlanMode"),
            item::tool_named("d", "TodoWrite"),
        ]);
        let t = turn(&rows[0]);
        assert_eq!(part_types(t), ["tools", "special", "tools", "special", "special"]);
        let Part::Special { call, .. } = &t.parts[1] else { panic!() };
        assert_eq!(call.name, "AskUserQuestion");
    }

    #[test]
    fn a_new_user_message_ends_the_turn_above_it() {
        let rows = build_rows(&[item::user("a"), item::text("one"), item::user("b"), item::text("two")]);
        assert_eq!(types(&rows), ["user", "assistant", "user", "assistant"]);
        assert!(!turn(&rows[1]).open);
        assert!(turn(&rows[3]).open);
    }

    #[test]
    fn a_queued_message_is_a_user_row_in_transcript_order() {
        let rows = build_rows(&[item::text("busy"), item::user_queued("later please"), item::text("ok")]);
        assert_eq!(types(&rows), ["assistant", "user", "assistant"]);
        let Row::User { item: ChatItem::User { queued, .. }, .. } = &rows[1] else { panic!() };
        assert_eq!(*queued, Some(true));
    }

    fn command(r: &Row) -> (&str, Option<&str>) {
        match r {
            Row::Command { name, output, .. } => (name, output.as_deref()),
            other => panic!("expected a command row, got {other:?}"),
        }
    }

    #[test]
    fn a_nameless_command_right_after_one_fills_in_its_output() {
        let rows = build_rows(&[item::command("compact", "", None), item::command("", "", Some("Compacted"))]);
        assert_eq!(rows.len(), 1);
        assert_eq!(command(&rows[0]), ("compact", Some("Compacted")));
    }

    #[test]
    fn output_never_overwrites_a_command_that_already_has_some() {
        let rows = build_rows(&[item::command("cost", "", Some("$1")), item::command("", "", Some("other"))]);
        assert_eq!(rows.iter().map(|r| command(r).1).collect::<Vec<_>>(), [Some("$1"), Some("other")]);
    }

    #[test]
    fn a_command_ends_the_turn_above_it() {
        let rows = build_rows(&[item::text("hi"), item::command("cost", "", None), item::text("next")]);
        assert_eq!(types(&rows), ["assistant", "command", "assistant"]);
        assert_eq!(command(&rows[1]).0, "cost");
    }

    #[test]
    fn a_setting_changed_with_the_apps_controls_isnt_shown_nor_its_output() {
        let rows = build_rows(&[
            item::text("hi"),
            item::command("model", "opus", None),
            item::command("", "", Some("Set model to Opus")),
            item::command("/effort", "high", None),
            item::command("compact", "", None),
            item::command("", "", Some("Compacted")),
        ]);
        assert_eq!(types(&rows), ["assistant", "command"]);
        assert_eq!(command(&rows[1]), ("compact", Some("Compacted")));
    }

    #[test]
    fn set_met_and_cleared_goals_keep_their_flags_in_order() {
        let rows =
            build_rows(&[item::goal("tests pass", false, None, None), item::goal("tests pass", true, Some("green"), None), item::goal("tests pass", false, None, Some(true))]);
        assert_eq!(types(&rows), ["goal", "goal", "goal"]);
        let flags: Vec<_> = rows
            .iter()
            .map(|r| match r {
                Row::Goal { item: ChatItem::Goal { met, cleared, reason, .. }, .. } => (*met, cleared.unwrap_or(false), reason.clone()),
                _ => panic!(),
            })
            .collect();
        assert_eq!(flags, [(false, false, None), (true, false, Some("green".into())), (false, true, None)]);
    }

    #[test]
    fn replies_after_a_goal_start_a_new_turn() {
        let rows = build_rows(&[item::user("go"), item::text("first"), item::goal("ship it", false, None, None), item::text("second")]);
        assert_eq!(types(&rows), ["user", "assistant", "goal", "assistant"]);
        assert_eq!(turn(&rows[1]).parts.len(), 1);
        assert!(matches!(&turn(&rows[3]).parts[0], Part::Text { text, .. } if text == "second"));
    }

    #[test]
    fn an_interruption_joins_the_turn_and_ends_it() {
        let rows = build_rows(&[item::text("doing"), item::notice("interrupted"), item::text("again")]);
        assert_eq!(types(&rows), ["assistant", "assistant"]);
        assert_eq!(part_types(turn(&rows[0])), ["text", "notice"]);
        assert!(!turn(&rows[0]).open);
    }

    #[test]
    fn a_background_job_finishing_mid_turn_stays_inside_the_turn() {
        let rows = build_rows(&[item::text("doing"), item::notice_text("task", "job done"), item::text("more")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(part_types(turn(&rows[0])), ["text", "notice", "text"]);
        assert!(turn(&rows[0]).open);
    }

    #[test]
    fn other_notices_and_any_notice_between_turns_are_rows_of_their_own() {
        let rows = build_rows(&[item::text("a"), item::notice("compact"), item::notice("interrupted"), item::notice("task")]);
        assert_eq!(types(&rows), ["assistant", "notice", "notice", "notice"]);
    }

    #[test]
    fn keys_are_stable_across_rebuilds_and_unique() {
        let items = [item::user("a"), item::text("b"), item::command("c", "", None), item::goal("d", false, None, None), item::notice("compact"), item::text("e")];
        let keys: Vec<String> = build_rows(&items).iter().map(|r| r.key().to_string()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        assert_eq!(build_rows(&items).iter().map(|r| r.key().to_string()).collect::<Vec<_>>(), keys);
    }

    #[test]
    fn a_long_conversation_builds_without_dropping_anything() {
        let mut items = Vec::new();
        for i in 0..500 {
            items.extend([item::user(&format!("q{i}")), item::tool(&format!("t{i}")), item::result(&format!("t{i}"), "ok"), item::text(&format!("a{i}")), item::end(1000.0)]);
        }
        let rows = build_rows(&items);
        assert_eq!(rows.len(), 1000);
        assert!(rows.iter().all(|r| match r {
            Row::Assistant(t) => !t.open && t.parts.len() == 2,
            _ => true,
        }));
    }
}
