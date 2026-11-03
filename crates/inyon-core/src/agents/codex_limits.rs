//! Codex's plan limits for the account a machine is signed in to, with no session running: asked
//! of `codex app-server`, the JSON-RPC server Codex's own clients use, the way its TUI asks at
//! start. Runs on that machine with its own login; only the numbers come back. Reset credits the
//! account has banked are counted, never redeemed.

use inyon_protocol::{AccountLimits, AgentKind};
use serde_json::{json, Value};

use crate::agents::status::{status_from_codex, CodexContext};
use crate::hosts::host::{ExecOptions, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{self, Js};

/// Starts the app-server, asks, and keeps its input open until the answer is in (it quits at the
/// end of its input without answering), 15 s at most.
pub const SCRIPT: &str = r#"
command -v codex >/dev/null 2>&1 || { echo "@@NOCODEX"; exit 0; }
o=$(mktemp 2>/dev/null) || { echo "@@NOTEMP"; exit 0; }
{
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"inyon","title":"Inyon","version":"1"}}}' '{"jsonrpc":"2.0","method":"initialized"}' '{"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read"}'
  i=0
  while [ $i -lt 150 ] && ! grep -q '"id":2' "$o" 2>/dev/null; do sleep 0.1; i=$((i + 1)); done
} | codex app-server >"$o" 2>/dev/null
# the numbers only: not the account's id
grep '"id":2' "$o" | head -n 1 | sed 's/"accountId":"[^"]*",\{0,1\}//'
rm -f "$o"
"#;

/// What the machine said: the limits, nothing (no Codex there), or why it couldn't tell.
pub fn parse(stdout: &str, now_ms: u64) -> Result<Option<AccountLimits>, String> {
    if stdout.contains("@@NOCODEX") {
        return Ok(None);
    }
    let Some(reply) = stdout.lines().find_map(|l| json::parse(l.trim()).filter(|v| Some(v).at("id").num() == Some(2.0))) else {
        return Err("Codex didn't answer about its limits".into());
    };
    let r = Some(&reply);
    if let Some(e) = r.at("error").at("message").str() {
        return Err(format!("Codex: {e}"));
    }
    let rl = r.at("result").at("rateLimits");
    if rl.is_none() {
        return Err("Codex didn't answer about its limits".into());
    }
    // the session log's names, so a window is read the same way as there
    let window = |w: Option<&Value>| {
        w.filter(|w| w.is_object()).map(|w| json!({ "used_percent": w.get("usedPercent"), "window_minutes": w.get("windowDurationMins"), "resets_at": w.get("resetsAt") }))
    };
    let credits = rl.at("credits").filter(|c| c.is_object()).map(|c| json!({ "has_credits": c.get("hasCredits"), "unlimited": c.get("unlimited"), "balance": c.get("balance") }));
    let ev = json!({ "rate_limits": { "primary": window(rl.at("primary")), "secondary": window(rl.at("secondary")), "credits": credits, "plan_type": rl.at("planType") } });
    let st = status_from_codex(&ev, &CodexContext::default(), now_ms as f64);
    let resets = r.at("result").at("rateLimitResetCredits").at("availableCount").num().map(|n| n.max(0.0) as u32).filter(|n| *n > 0);
    Ok(Some(AccountLimits { kind: AgentKind::Codex, five_hour: st.five_hour, seven_day: st.seven_day, plan: st.plan, credits: st.credits, resets, updated_at: now_ms }))
}

/// Asks the machine. The outer error is the machine's (unreachable); the inner one Codex's.
pub async fn fetch(host: &Host) -> HubResult<Result<Option<AccountLimits>, String>> {
    let r = host.exec(SCRIPT, ExecOptions::timeout(25_000)).await?;
    Ok(parse(&r.stdout, crate::clock::now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_protocol::{Credits, Num, RateWindow};

    /// codex-cli 0.162.1's answer on a free plan (the account id left out)
    const FREE: &str = r#"{"id":2,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"codex","limitName":null,"normalModelSlug":null,"primary":{"usedPercent":12,"windowDurationMins":43200,"resetsAt":1794955078},"secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":null},"individualLimit":null,"spendControlReached":false,"planType":"free","rateLimitReachedType":null},"rateLimitResetCredits":{"availableCount":0,"credits":[]},"rateLimitUpsell":null}}"#;

    #[test]
    fn the_app_servers_answer_becomes_the_same_windows_a_session_reports() {
        let plus = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":41,"windowDurationMins":300,"resetsAt":1794000000},"secondary":{"usedPercent":7,"windowDurationMins":10080,"resetsAt":1794500000},"credits":{"hasCredits":true,"unlimited":false,"balance":"12.50"},"planType":"plus"},"rateLimitResetCredits":{"availableCount":2,"credits":[]}}}"#;
        let got = parse(&format!("{{\"id\":1,\"result\":{{}}}}\n{plus}\n"), 5000).unwrap().unwrap();
        assert_eq!(got.five_hour, Some(RateWindow { pct: Num(41.0), resets_at: Some(Num(1794000000.0)) }));
        assert_eq!(got.seven_day, Some(RateWindow { pct: Num(7.0), resets_at: Some(Num(1794500000.0)) }));
        assert_eq!((got.plan.as_deref(), got.resets, got.updated_at), (Some("plus"), Some(2), 5000));
        assert_eq!(got.credits, Some(Credits { balance: Some("12.50".into()), unlimited: Some(false) }));
        // a free plan's 30-day window is neither a 5-hour nor a weekly one, and no resets are banked
        let free = parse(FREE, 5000).unwrap().unwrap();
        assert_eq!((free.five_hour, free.seven_day, free.plan.as_deref(), free.resets, free.credits), (None, None, Some("free"), None, None));
    }

    #[test]
    fn no_codex_is_nothing_and_no_answer_is_an_error() {
        assert_eq!(parse("@@NOCODEX\n", 1), Ok(None));
        assert!(parse("", 1).is_err());
        assert_eq!(parse(r#"{"id":2,"error":{"code":-32600,"message":"not signed in"}}"#, 1), Err("Codex: not signed in".into()));
    }
}
