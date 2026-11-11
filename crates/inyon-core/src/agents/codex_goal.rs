//! Goal state can change without a rollout event (notably automatic limits).
use inyon_protocol::ChatGoal;
use serde_json::Value;
use crate::hosts::host::shq;

pub fn watch_command(path: &str, thread: &str) -> Option<String> {
    let (home, _) = path.rsplit_once("/sessions/")?;
    let db = format!("{home}/goals_1.sqlite");
    Some(format!("for p in python3 python; do if \"$p\" -c 'import sys; sys.exit(sys.version_info[0] != 3)' </dev/null >/dev/null 2>&1; then exec \"$p\" -u -c {} {} {}; fi; done",
        shq(include_str!("codex_goal.py")), shq(&db), shq(thread)))
}

/// Unknown/malformed results must never remove the last known goal.
pub fn snapshot(line: &str, thread: &str) -> Option<Option<ChatGoal>> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("threadId")?.as_str()? != thread { return None; }
    let goal = value.get("goal")?;
    if goal.is_null() { return Some(None); }
    let goal: ChatGoal = serde_json::from_value(goal.clone()).ok()?;
    if goal.condition.trim().is_empty() || goal.status.as_ref().is_none_or(|s| s.trim().is_empty()) { return None; }
    Some(Some(goal))
}
