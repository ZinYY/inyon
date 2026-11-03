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
}

pub fn record(controls: &mut Option<Controls>, output: &str, at: u64) {
    let mut next = controls.take().unwrap_or_default();
    next.record(output, at);
    *controls = (next.model.is_some() || next.effort.is_some()).then_some(next);
}

impl Controls {
    /// Only call for Claude's local-command stdout, never a command request or an assistant reply.
    pub fn record(&mut self, output: &str, at: u64) {
        let output = output.trim();
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
}
