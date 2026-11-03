//! Port of agents/status.ts: the numbers an agent reports (Claude's status line payload, Codex's
//! token_count events) as an AgentStatus.

use indexmap::IndexMap;
use inyon_protocol::{AgentStatus, Credits, IntegrationState, Num, RateWindow, Stale, StaleContext, StaleLimits};
use serde_json::Value;

use crate::claude::transcript::string_of;
use crate::js::json::Js;
use crate::js::{date, js_round};
use crate::tmux::tmux::StatusPane;

fn num(v: Option<&Value>) -> Option<f64> {
    v.num().filter(|x| x.is_finite())
}

/// `a ?? 0` added up, as the TS adds token counts.
fn add(vals: &[Option<&Value>]) -> f64 {
    vals.iter().map(|v| v.present().num().unwrap_or(0.0)).sum()
}

pub(crate) fn empty_status(updated_at: f64) -> AgentStatus {
    AgentStatus {
        model: None,
        model_id: None,
        effort: None,
        fast: None,
        version: None,
        context_pct: None,
        context_used: None,
        context_size: None,
        cost_usd: None,
        duration_ms: None,
        api_duration_ms: None,
        lines_added: None,
        lines_removed: None,
        five_hour: None,
        seven_day: None,
        credits: None,
        plan: None,
        output_style: None,
        stale: None,
        updated_at: updated_at as u64,
    }
}

fn to_unix(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => {
            let v = n.as_f64()?;
            Some(if v > 1e12 { js_round(v / 1000.0) } else { v })
        }
        Value::String(s) => {
            let t = date::parse(s);
            (!t.is_nan()).then(|| js_round(t / 1000.0))
        }
        _ => None,
    }
}

/// Claude Code's status line payload → AgentStatus.
pub fn status_from_claude(d: &Value, ts: f64) -> AgentStatus {
    let d = Some(d);
    let cw = d.at("context_window");
    let cu = cw.at("current_usage");
    let used = num(cu.at("input_tokens")).map(|_| add(&[cu.at("input_tokens"), cu.at("cache_creation_input_tokens"), cu.at("cache_read_input_tokens")]));
    let rl = d.at("rate_limits");
    let win =
        |w: Option<&Value>| if w.truthy() { num(w.at("used_percentage")).map(|pct| RateWindow { pct: Num(pct), resets_at: to_unix(w.at("resets_at")).map(Num) }) } else { None };
    let n = |v: Option<&Value>| num(v).map(Num);
    let mut s = empty_status(ts * 1000.0);
    s.model = string_of(d.at("model").at("display_name"));
    s.model_id = string_of(d.at("model").at("id"));
    s.effort = string_of(d.at("effort").at("level"));
    s.fast = Some(d.at("fast_mode").truthy());
    s.version = string_of(d.at("version"));
    s.context_pct = n(cw.at("used_percentage"));
    s.context_used = used.map(Num);
    s.context_size = n(cw.at("context_window_size"));
    s.cost_usd = n(d.at("cost").at("total_cost_usd"));
    s.duration_ms = n(d.at("cost").at("total_duration_ms"));
    s.api_duration_ms = n(d.at("cost").at("total_api_duration_ms"));
    s.lines_added = n(d.at("cost").at("total_lines_added"));
    s.lines_removed = n(d.at("cost").at("total_lines_removed"));
    s.five_hour = win(rl.at("five_hour"));
    s.seven_day = win(rl.at("seven_day"));
    s.output_style = string_of(d.at("output_style").at("name"));
    s
}

/// The status line snapshot Claude last wrote for a pane, minus anything that is no longer true.
/// The file stays behind when Claude stops writing it, so every number has to earn its place.
/// Takes the probe's integration, statusPanes and compactPanes.
pub fn claude_status(
    integration: IntegrationState,
    status_panes: &IndexMap<String, StatusPane>,
    compact_panes: &IndexMap<String, f64>,
    pane: &str,
    started_at: f64,
    session_id: Option<&str>,
) -> Option<AgentStatus> {
    let st = status_panes.get(pane)?;
    // Claude only writes it while the integration is in its settings; otherwise it's a leftover
    if integration != IntegrationState::Installed {
        return None;
    }
    // from an earlier process in this pane, or from before a /clear or /resume
    if st.ts < started_at - 5.0 {
        return None;
    }
    let sid = Some(&st.data).at("session_id");
    if let Some(want) = session_id.filter(|s| !s.is_empty()) {
        if sid.truthy() && sid.str() != Some(want) {
            return None;
        }
    }
    let mut status = status_from_claude(&st.data, st.ts);
    // limits belong to the account, not the session: after /login they describe someone else
    if let (Some(a), Some(now)) = (st.account.as_deref().filter(|a| !a.is_empty()), st.account_now.as_deref().filter(|a| !a.is_empty())) {
        if a != now {
            status.five_hour = None;
            status.seven_day = None;
            let stale = status.stale.get_or_insert(Stale { limits: None, context: None });
            stale.limits = Some(StaleLimits::Account);
        }
    }
    // the snapshot predates the last compaction, so its context size is the pre-compaction one
    if let Some(&compacted) = compact_panes.get(pane) {
        if compacted != 0.0 && compacted >= started_at - 5.0 && compacted > st.ts {
            status.context_pct = None;
            status.context_used = None;
            let stale = status.stale.get_or_insert(Stale { limits: None, context: None });
            stale.context = Some(StaleContext::Compacted);
        }
    }
    Some(status)
}

/// The turn's model and effort, for `status_from_codex`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexContext {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Codex token_count event (+ the turn's model/effort) → AgentStatus.
pub fn status_from_codex(ev: &Value, ctx: &CodexContext, ts_ms: f64) -> AgentStatus {
    let ev = Some(ev);
    let info = ev.at("info");
    let last = info.at("last_token_usage");
    let size = num(info.at("model_context_window"));
    let used = num(last.at("input_tokens")).map(|_| add(&[last.at("input_tokens"), last.at("output_tokens")]));
    let rl = ev.at("rate_limits");
    let win = |w: Option<&Value>| {
        if !w.truthy() {
            return None;
        }
        let pct = num(w.at("used_percent"))?;
        let resets_at = match num(w.at("resets_in_seconds")) {
            Some(s) => Some(js_round(ts_ms / 1000.0 + s)),
            None => to_unix(w.at("resets_at")),
        };
        Some(RateWindow { pct: Num(pct), resets_at: resets_at.map(Num) })
    };
    let credits = rl.at("credits");
    let credits = if credits.truthy() && (credits.at("has_credits").truthy() || credits.at("unlimited").truthy()) {
        Some(Credits { balance: string_of(credits.at("balance")), unlimited: Some(credits.at("unlimited").truthy()) })
    } else {
        None
    };
    let (primary, secondary) = (rl.at("primary"), rl.at("secondary"));
    fn pick<'a>(ws: [Option<&'a Value>; 2], m: f64, fallback: Option<&'a Value>) -> Option<&'a Value> {
        ws.into_iter().find(|w| w.at("window_minutes").num() == Some(m)).flatten().or(if fallback.at("window_minutes").present().is_none() { fallback } else { None })
    }
    let mut s = empty_status(ts_ms);
    s.model = ctx.model.clone();
    s.model_id = ctx.model.clone();
    s.effort = ctx.effort.clone();
    s.credits = credits;
    s.plan = rl.at("plan_type").str().map(str::to_owned);
    s.context_used = used.map(Num);
    s.context_size = size.map(Num);
    s.context_pct = match (used, size) {
        (Some(u), Some(z)) if z != 0.0 => Some(Num((u / z * 100.0).min(100.0))),
        _ => None,
    };
    // Some subscription plans expose only a weekly primary bucket.
    s.five_hour = win(pick([primary, secondary], 300.0, primary));
    s.seven_day = win(pick([primary, secondary], 10080.0, secondary));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// review: numbers where the TS expects strings pass through (as text), not dropped
    #[test]
    fn numbers_in_string_fields_pass_through_as_text() {
        let s = status_from_claude(&json!({ "model": { "display_name": 5, "id": 7 }, "effort": { "level": 2 }, "version": 2.1, "output_style": { "name": 0 } }), 1.0);
        assert_eq!(
            (s.model.as_deref(), s.model_id.as_deref(), s.effort.as_deref(), s.version.as_deref(), s.output_style.as_deref()),
            (Some("5"), Some("7"), Some("2"), Some("2.1"), Some("0"))
        );
        let c = status_from_codex(&json!({ "rate_limits": { "credits": { "has_credits": true, "balance": 12.5 } } }), &CodexContext::default(), 0.0);
        assert_eq!(c.credits.unwrap().balance.as_deref(), Some("12.5"));
        let none = status_from_codex(&json!({ "rate_limits": { "credits": { "has_credits": true, "balance": null } } }), &CodexContext::default(), 0.0);
        assert_eq!(none.credits.unwrap().balance, None);
    }

    /// Codex weekly-only primary quota is not labeled a five-hour window (codex.test.ts)
    #[test]
    fn codex_weekly_only_primary_quota_is_not_labeled_a_five_hour_window() {
        let now = crate::clock::now_ms() as f64;
        let status = status_from_codex(
            &json!({ "rate_limits": { "primary": { "used_percent": 64, "window_minutes": 10080, "resets_at": 1791956344 }, "secondary": null } }),
            &CodexContext::default(),
            now,
        );
        assert_eq!(status.five_hour, None);
        assert_eq!(status.seven_day, Some(RateWindow { pct: Num(64.0), resets_at: Some(Num(1791956344.0)) }));
        let both = status_from_codex(
            &json!({ "rate_limits": { "primary": { "used_percent": 10, "window_minutes": 300 }, "secondary": { "used_percent": 20, "window_minutes": 10080 } } }),
            &CodexContext::default(),
            now,
        );
        assert_eq!(both.five_hour.unwrap().pct, Num(10.0));
        assert_eq!(both.seven_day.unwrap().pct, Num(20.0));
    }

    /// describe('claudeStatus'), from claude/integration.test.ts
    mod claude_status_tests {
        use super::*;

        fn payload(session: &str) -> Value {
            json!({
                "session_id": session,
                "model": { "id": "claude-opus", "display_name": "Opus" },
                "context_window": { "context_window_size": 1000, "used_percentage": 90, "current_usage": { "input_tokens": 900 } },
                "rate_limits": { "five_hour": { "used_percentage": 100, "resets_at": 4102444800u64 }, "seven_day": { "used_percentage": 93, "resets_at": 4102444800u64 } },
            })
        }
        struct Base {
            integration: IntegrationState,
            status_panes: IndexMap<String, StatusPane>,
            compact_panes: IndexMap<String, f64>,
        }
        fn base(ts: f64, account_now: &str, session: &str) -> Base {
            let mut status_panes = IndexMap::new();
            status_panes.insert("%9".into(), StatusPane { ts, data: payload(session), account: Some("A".into()), account_now: Some(account_now.into()) });
            Base { integration: IntegrationState::Installed, status_panes, compact_panes: IndexMap::new() }
        }
        fn st(b: &Base, started_at: f64, session: &str) -> Option<AgentStatus> {
            claude_status(b.integration, &b.status_panes, &b.compact_panes, "%9", started_at, Some(session))
        }

        /// fresh snapshot passes through
        #[test]
        fn fresh_snapshot_passes_through() {
            let s = st(&base(1000.0, "A", "s1"), 900.0, "s1").unwrap();
            assert_eq!(s.five_hour.unwrap().pct, Num(100.0));
            assert_eq!(s.context_pct, Some(Num(90.0)));
            assert_eq!(s.stale, None);
        }

        /// leftover file is ignored once the integration is gone from settings
        #[test]
        fn leftover_file_is_ignored_once_the_integration_is_gone_from_settings() {
            let mut b = base(1000.0, "A", "s1");
            b.integration = IntegrationState::Removed;
            assert_eq!(st(&b, 900.0, "s1"), None);
        }

        /// snapshot from another session or an earlier process is ignored
        #[test]
        fn snapshot_from_another_session_or_an_earlier_process_is_ignored() {
            assert_eq!(st(&base(1000.0, "A", "s1"), 900.0, "s2"), None);
            assert_eq!(st(&base(1000.0, "A", "s1"), 2000.0, "s1"), None);
        }

        /// Ghostex #248: a session's model chip showed another session's model. Each pane reads
        /// its own snapshot, and a pane without one shows none rather than its neighbour's.
        #[test]
        fn each_pane_shows_its_own_model_never_anothers() {
            let mut b = base(1000.0, "A", "s1");
            let mut other = payload("s2");
            other["model"] = json!({ "id": "claude-haiku", "display_name": "Haiku" });
            b.status_panes.insert("%4".into(), StatusPane { ts: 1000.0, data: other, account: Some("A".into()), account_now: Some("A".into()) });
            let of = |pane: &str, session: &str| claude_status(b.integration, &b.status_panes, &b.compact_panes, pane, 900.0, Some(session)).and_then(|s| s.model);
            assert_eq!(of("%9", "s1").as_deref(), Some("Opus"));
            assert_eq!(of("%4", "s2").as_deref(), Some("Haiku"));
            assert_eq!(of("%5", "s3"), None);
        }

        /// limits are dropped after switching account
        #[test]
        fn limits_are_dropped_after_switching_account() {
            let s = st(&base(1000.0, "B", "s1"), 900.0, "s1").unwrap();
            assert_eq!(s.five_hour, None);
            assert_eq!(s.seven_day, None);
            assert_eq!(s.context_pct, Some(Num(90.0)));
            assert_eq!(s.stale.unwrap().limits, Some(StaleLimits::Account));
        }

        /// context is dropped when the snapshot predates a compaction
        #[test]
        fn context_is_dropped_when_the_snapshot_predates_a_compaction() {
            let mut pr = base(1000.0, "A", "s1");
            pr.compact_panes.insert("%9".into(), 1500.0);
            let s = st(&pr, 900.0, "s1").unwrap();
            assert_eq!(s.context_pct, None);
            assert_eq!(s.context_used, None);
            assert_eq!(s.five_hour.unwrap().pct, Num(100.0));
            assert_eq!(s.stale.unwrap().context, Some(StaleContext::Compacted));
            // a snapshot written after the compaction is current again
            pr.status_panes.get_mut("%9").unwrap().ts = 1600.0;
            assert_eq!(st(&pr, 900.0, "s1").unwrap().context_pct, Some(Num(90.0)));
        }
    }
}
