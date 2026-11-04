//! Claude acknowledges local settings in its transcript before its status-line payload changes.
//! Keep each confirmed value's own time: changing effort must not revive an older model pick.

use inyon_protocol::{agents, AgentKind, AgentStatus, Num};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confirmed {
    value: String,
    at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Controls {
    model: Option<Confirmed>,
    effort: Option<Confirmed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<NativeBranch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct NativeBranch {
    session: String,
    parent: String,
    at: u64,
}

pub fn record(controls: &mut Option<Controls>, output: &str, at: u64) {
    let mut next = controls.take().unwrap_or_default();
    next.record(output, at);
    *controls = (next.model.is_some() || next.effort.is_some() || next.branch.is_some()).then_some(next);
}

impl Controls {
    /// Only call for Claude's local-command stdout, never a command request or an assistant reply.
    pub fn record(&mut self, output: &str, at: u64) {
        let output = output.trim();
        if let Some(c) = crate::re!(r#"^Branched conversation [\s\S]*?You are now in the new branch \(session ([0-9a-f-]{36})\)\. Use /resume ([0-9a-f-]{36})\b"#).captures(output) {
            if self.branch.as_ref().is_none_or(|old| at >= old.at) {
                self.branch = Some(NativeBranch { session: c[1].to_owned(), parent: c[2].to_owned(), at });
            }
        }
        if let Some(rest) = output.strip_prefix("Set model to ") {
            let value = if let Some(quoted) = rest.strip_prefix('`') {
                quoted.split_once('`').map(|(name, _)| name)
            } else {
                Some(rest.split(" and saved").next().unwrap_or(rest).split(" with ").next().unwrap_or(rest).trim())
            };
            if let Some(value) = value.filter(|v| !v.is_empty() && !v.contains('\n')) {
                if self.model.as_ref().is_none_or(|old| at >= old.at) {
                    self.model = Some(Confirmed { value: value.to_owned(), at });
                }
            }
        }
        if let Some(c) = crate::re!(r"^Set effort(?: level)? to (low|medium|high|xhigh|max|auto|default)\b").captures(output) {
            if self.effort.as_ref().is_none_or(|old| at >= old.at) {
                self.effort = Some(Confirmed { value: c[1].to_owned(), at });
            }
        }
    }

    /// /branch keeps the current model/effort but may leave statusLine on its parent.
    /// Only Claude's own successful command, in this live process, proves inheritance.
    /// /resume, another parent, a later parent report and a restarted process cannot.
    /// Never transfer conversation identity, cost, context usage or account limits.
    pub fn branch_status(&self, session: &str, started_at: u64, parent_id: &str, parent: &AgentStatus) -> Option<AgentStatus> {
        let b = self.branch.as_ref()?;
        if b.session != session || b.parent != parent_id || b.session == b.parent
            || b.at < started_at || parent.updated_at < started_at || parent.updated_at > b.at {
            return None;
        }
        let mut status = crate::agents::status::empty_status(b.at as f64);
        status.model = parent.model.clone();
        status.model_id = parent.model_id.clone();
        status.effort = parent.effort.clone();
        self.status(Some(&status))
    }

    pub fn status(&self, live: Option<&AgentStatus>) -> Option<AgentStatus> {
        let mut out = live.cloned().unwrap_or_else(|| crate::agents::status::empty_status(0.0));
        let reported_at = out.updated_at;
        if let Some(pick) = self.model.as_ref().filter(|p| p.at > reported_at) {
            let model = agents()[&AgentKind::Claude].models.iter().find(|m| m.label.eq_ignore_ascii_case(&pick.value) || m.id == pick.value);
            let changed = out.model.as_deref() != Some(pick.value.as_str());
            out.model = Some(pick.value.clone());
            // The old model's id must not override the new display name in the model menu.
            out.model_id = model.map(|m| m.id.clone());
            if changed {
                out.context_size = model.and_then(|m| m.context);
                out.context_pct = match (out.context_used, out.context_size) {
                    (Some(used), Some(size)) if size.0 > 0.0 => Some(Num(100.0 * used.0 / size.0)),
                    _ => None,
                };
            }
        }
        if let Some(pick) = self.effort.as_ref().filter(|p| p.at > reported_at) {
            out.effort = Some(pick.value.clone());
        }
        // updated_at remains the status-line time, not a claim that usage/limits were refreshed.
        (live.is_some() || out.model.is_some() || out.effort.is_some()).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn live(at: u64) -> AgentStatus {
        serde_json::from_value(json!({"model":"Opus 5.5","modelId":"claude-opus-5-5","effort":"high","contextUsed":40000,"contextSize":1000000,"costUsd":0.5,"updatedAt":at})).unwrap()
    }

    #[test]
    fn confirmed_commands_override_old_status_without_a_new_turn() {
        let mut c = Controls::default();
        c.record("Set model to `Sonnet 5.5` and saved as your default for new sessions", 2000);
        c.record("Set effort level to low (saved as your default for new sessions): Fast", 3000);
        let s = c.status(Some(&live(1000))).unwrap();
        assert_eq!(s.model.as_deref(), Some("Sonnet 5.5"));
        assert_eq!(s.model_id.as_deref(), Some("sonnet"));
        assert_eq!(s.effort.as_deref(), Some("low"));
        assert_eq!(s.cost_usd, Some(Num(0.5)));
        assert_eq!(s.context_used, Some(Num(40000.0)));
        assert_eq!(s.updated_at, 1000);
    }

    #[test]
    fn each_field_defers_to_a_newer_report_and_old_history_cannot_revert_it() {
        let mut c = Controls::default();
        c.record("Set model to `Sonnet 5.5`", 1000);
        c.record("Set effort level to low", 3000);
        c.record("Set effort level to high", 500);
        let s = c.status(Some(&live(2000))).unwrap();
        assert_eq!(s.model.as_deref(), Some("Opus 5.5"));
        assert_eq!(s.effort.as_deref(), Some("low"));
        assert_eq!(c.status(Some(&live(4000))).unwrap(), live(4000));
    }

    #[test]
    fn requests_errors_and_confirmation_prompts_do_not_claim_success() {
        let mut c = Controls::default();
        for text in ["/model sonnet", "Switch model to Sonnet?", "Could not set model to Sonnet", "Set effort level to invalid"] {
            c.record(text, 2000);
        }
        assert_eq!(c.status(Some(&live(1000))).unwrap(), live(1000));
        assert!(c.status(None).is_none());
    }

    #[test]
    fn native_branch_inherits_only_controls_from_its_proven_parent() {
        let child = "11111111-1111-4111-8111-111111111111";
        let parent = "22222222-2222-4222-8222-222222222222";
        let mut controls = None;
        record(&mut controls, &format!("Branched conversation \"Study music\". You are now in the new branch (session {child}). Use /resume {parent} (\"Music\") to return to the original."), 3000);
        // Saved chat metadata must retain the proof across a reload.
        let c: Controls = serde_json::from_str(&serde_json::to_string(&controls.unwrap()).unwrap()).unwrap();
        let inherited = c.branch_status(child, 1000, parent, &live(2000)).unwrap();
        assert_eq!(inherited.model.as_deref(), Some("Opus 5.5"));
        assert_eq!(inherited.model_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(inherited.effort.as_deref(), Some("high"));
        assert!(inherited.cost_usd.is_none());
        assert!(inherited.context_used.is_none());
        assert!(inherited.context_size.is_none());
        assert!(inherited.five_hour.is_none());
        assert_eq!(inherited.updated_at, 3000);
        for (session, started, parent_id, status) in [
            (parent, 1000, parent, live(2000)), // old branch record copied into another session
            (child, 4000, parent, live(2000)), // /resume in a restarted process
            (child, 1000, child, live(2000)), // unrelated status identity
            (child, 1000, parent, live(4000)), // /resume an old branch after later parent work
            (child, 2500, parent, live(2000)), // parent status predates this process
        ] {
            assert!(c.branch_status(session, started, parent_id, &status).is_none());
        }
        let mut c = c;
        c.record("Set effort level to low", 4000);
        assert_eq!(c.branch_status(child, 1000, parent, &live(2000)).unwrap().effort.as_deref(), Some("low"));
    }
}
