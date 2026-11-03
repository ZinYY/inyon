//! Port of agents/claude-usage.ts: plan limits for the account a machine is signed in to, from the
//! same endpoint Claude Code's /usage reads. Runs on that machine with its own Claude login: the
//! token is read there, handed to curl on stdin (so it never shows in `ps`), and never comes back
//! to the hub.

use inyon_protocol::RateWindow;
use serde_json::Value;

use crate::clock::now_ms;
use crate::hosts::host::{ExecOptions, Host};
use crate::hosts::http_error::HubResult;
use crate::js::json::{self, value_string, Js};
use crate::js::{date, js_round, JsStr};
use crate::re;

/// The script, byte for byte the TS's (fixtures/expected/scripts.json checks it).
pub const SCRIPT: &str = r#"
d="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
tok=""
[ -f "$d/.credentials.json" ] && tok=$(sed -n 's/.*"accessToken" *: *"\([^"]*\)".*/\1/p' "$d/.credentials.json" | head -n 1)
if [ -z "$tok" ] && command -v security >/dev/null 2>&1; then
  tok=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null | sed -n 's/.*"accessToken" *: *"\([^"]*\)".*/\1/p')
fi
[ -n "$tok" ] || { echo "@@NOLOGIN"; exit 0; }
# only the plan fields leave the machine, never the token
c="$d/.credentials.json"
[ -f "$c" ] && echo "@@PLAN $(sed -n 's/.*"subscriptionType" *: *"\([^"]*\)".*/\1/p' "$c" | head -n 1) $(sed -n 's/.*"rateLimitTier" *: *"\([^"]*\)".*/\1/p' "$c" | head -n 1)"
command -v curl >/dev/null 2>&1 || { echo "@@NOCURL"; exit 0; }
printf 'Authorization: Bearer %s\n' "$tok" | curl -sS -m 10 -H @- -H 'anthropic-beta: oauth-2025-04-20' https://api.anthropic.com/api/oauth/usage
"#;

/// What the endpoint said, and when it was asked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountUsage {
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    /// unix ms
    pub at: u64,
    pub error: Option<String>,
    /// "Max 20x", "Pro", …
    pub plan: Option<String>,
}

/// subscriptionType + rateLimitTier from the login file, e.g. max + default_claude_max_20x → "Max 20x"
pub fn plan_label(sub: Option<&str>, tier: Option<&str>) -> Option<String> {
    let sub = sub.filter(|s| !s.is_empty())?;
    let mut chars = sub.chars();
    let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let name = format!("{first}{}", chars.as_str().to_lowercase());
    let x = tier.and_then(|t| re!(r"_(\d+x)$").captures(t)).map(|m| m[1].to_owned());
    Some(match x {
        Some(x) if sub.to_lowercase() == "max" => format!("{name} {x}"),
        _ => name,
    })
}

fn unix(v: Option<&Value>) -> Option<f64> {
    let t = date::parse(v.str()?);
    (!t.is_nan()).then(|| js_round(t / 1000.0))
}

fn win(w: Option<&Value>) -> Option<RateWindow> {
    let pct = w.at("utilization").num()?;
    Some(RateWindow { pct: pct.into(), resets_at: unix(w.at("resets_at")).map(Into::into) })
}

/// What `fetchClaudeUsage` makes of the script's output.
pub fn read_usage(stdout: &str, stderr: &str, now: u64) -> AccountUsage {
    let fail = |e: String| AccountUsage { at: now, error: Some(e), ..Default::default() };
    let m = re!(r"^@@PLAN ?(\S*) ?(\S*)$", "m").captures(stdout);
    let group = |i: usize| m.as_ref().and_then(|m| m.get(i)).map(|g| g.as_str()).filter(|s| !s.is_empty());
    let plan = plan_label(group(1), group(2));
    let out = re!(r"^@@PLAN.*$", "m").replacen(stdout, 1, "").jtrim().to_owned();
    if out.contains("@@NOLOGIN") {
        return fail("Not signed in to a Claude subscription".into());
    }
    if out.contains("@@NOCURL") {
        return fail("curl is not installed".into());
    }
    // `JSON.parse` failing, or reading off null, lands in the same catch
    match json::parse(&out).filter(|d| !d.is_null()) {
        Some(d) => {
            let d = Some(&d);
            if d.at("error").truthy() {
                let msg = d.at("error").at("message").filter(|m| !m.is_null()).map_or_else(|| "Usage request failed".to_owned(), value_string);
                return fail(msg);
            }
            AccountUsage { five_hour: win(d.at("five_hour")), seven_day: win(d.at("seven_day")), at: now, error: None, plan }
        }
        None => {
            let text = if !stderr.is_empty() {
                stderr
            } else if !out.is_empty() {
                &out
            } else {
                "Usage request failed"
            };
            fail(text.jtrim().slice16(0, Some(200)).to_owned())
        }
    }
}

/// `fetchClaudeUsage(host)`
pub async fn fetch_claude_usage(host: &Host) -> HubResult<AccountUsage> {
    let r = host.exec(SCRIPT, ExecOptions::timeout(15000)).await?;
    Ok(read_usage(&r.stdout, &r.stderr, now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_names_read_as_the_app_shows_them() {
        assert_eq!(plan_label(Some("max"), Some("default_claude_max_20x")).as_deref(), Some("Max 20x"));
        assert_eq!(plan_label(Some("pro"), Some("default_claude_max_20x")).as_deref(), Some("Pro"));
        assert_eq!(plan_label(None, Some("x")), None);
    }

    #[test]
    fn the_usage_answer_becomes_limits_or_a_reason() {
        let u = read_usage("@@PLAN max default_claude_max_5x\n{\"five_hour\":{\"utilization\":42,\"resets_at\":\"2026-10-19T10:00:00Z\"},\"seven_day\":null}\n", "", 5);
        assert_eq!(u.plan.as_deref(), Some("Max 5x"));
        assert_eq!(u.five_hour, Some(RateWindow { pct: 42.0.into(), resets_at: Some(1_792_404_000.0.into()) }));
        assert_eq!(u.seven_day, None);
        assert_eq!(read_usage("@@NOLOGIN\n", "", 5).error.as_deref(), Some("Not signed in to a Claude subscription"));
        assert_eq!(read_usage("{\"error\":{\"message\":\"rate limited\"}}", "", 5).error.as_deref(), Some("rate limited"));
        assert_eq!(read_usage("<html>", "curl: (6) no host", 5).error.as_deref(), Some("curl: (6) no host"));
    }
}
