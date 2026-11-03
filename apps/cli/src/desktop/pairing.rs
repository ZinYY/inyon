//! A pairing code as the window follows it, from the engine's answers to "used" or "expired", and
//! the small readings of the link's settings and of setup's output the window needs. No Win32 here.

use serde_json::{json, Value};

use crate::ui::dashboard::PairedPhone;

/// How long a code works, from the engine.
pub const CODE_MS: f64 = inyon_engine::link::PAIR_MINUTES as f64 * 60_000.0;

/// What the code card shows for one of the engine's answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeCard {
    /// it couldn't make one
    Failed(String),
    /// no code worth scanning yet: the tunnel is starting
    Starting(&'static str),
    /// the QR code and the typed-in code
    Code,
}

pub fn code_card(v: &Value) -> CodeCard {
    use inyon_engine::output::{TUNNEL_SLOW, TUNNEL_STARTING};
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return CodeCard::Failed(e.to_string());
    }
    let waiting = v.get("waiting").is_some();
    let none = |k: &str| v.get(k).and_then(Value::as_array).is_none_or(|a| a.is_empty());
    match (waiting, v.get("qr").is_some(), none("urls") && none("relays")) {
        (true, false, _) => CodeCard::Starting(TUNNEL_STARTING),
        (true, true, true) => CodeCard::Starting(TUNNEL_SLOW),
        _ => CodeCard::Code,
    }
}

/// What the window says when the pairing's engine ends, given how it ended and whether the last
/// answer shown was only "the tunnel is starting" (None: nothing was shown). A usable code stays
/// on screen; a card still waiting for the tunnel is replaced by why it never came.
pub fn pairing_ended(r: Result<(), String>, last_starting: Option<bool>) -> Option<String> {
    match (r, last_starting) {
        (Err(e), None | Some(true)) => Some(e),
        (Ok(()), None) => Some("The engine gave no code.".to_string()),
        (Ok(()), Some(true)) => Some("The engine stopped before the tunnel came up. Activity has the log.".to_string()),
        (_, Some(false)) => None,
    }
}

/// The typed-in code as the app wants it: "ZS5C-3EPG" and "801 356".
pub fn short_code(v: &Value) -> Option<(String, String)> {
    let short = v.get("short")?;
    let id = short.get("id").and_then(Value::as_str)?;
    let code = short.get("code").and_then(Value::as_str)?;
    let (a, b) = id.split_at(id.len().min(4));
    let (c, d) = code.split_at(code.len().min(3));
    Some((format!("{a}-{b}"), format!("{c} {d}")))
}

/// Why the code card went back to idle.
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    /// a phone paired with it
    Used(String),
    Expired,
}

/// The pairing card's state.
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    /// no code open; what happened to the last one, if anything
    Idle(Option<Ended>),
    /// asked, no answer yet
    Asking,
    /// the tunnel is starting
    Starting,
    /// still starting after half a minute
    Slow,
    /// a code; `lan`: it works on the same network or VPN until the tunnel is up
    Code {
        lan: bool,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pairing {
    pub card: Card,
    /// the engine's newest answer that carries a code
    pub answer: Option<Value>,
    /// when the code was asked for; it stops working CODE_MS after
    pub asked_at: f64,
    /// the phones paired when it was asked for: a new one means it was used
    pub known: Vec<String>,
    /// pairing.json was seen open since the code showed
    pub seen_open: bool,
    /// which ask this is, so a cancelled ask's late answers are dropped
    pub ask_no: u64,
}

impl Default for Pairing {
    fn default() -> Self {
        Pairing { card: Card::Idle(None), answer: None, asked_at: 0.0, known: Vec::new(), seen_open: false, ask_no: 0 }
    }
}

impl Pairing {
    /// Whether a code is being made (a second one can't start).
    pub fn busy(&self) -> bool {
        matches!(self.card, Card::Asking | Card::Starting | Card::Slow)
    }

    /// A new code was asked for: the ask's number, or None while one is being made.
    pub fn ask(&mut self, phones: &[PairedPhone], now: f64) -> Option<u64> {
        if self.busy() {
            return None;
        }
        let n = self.ask_no + 1;
        *self = Pairing { card: Card::Asking, answer: None, asked_at: now, known: phones.iter().map(|p| p.key.clone()).collect(), seen_open: false, ask_no: n };
        Some(n)
    }

    /// One of the engine's answers to ask `n` arrived.
    pub fn answer(&mut self, n: u64, v: Value) {
        // a code that has ended (or was cancelled) stays ended: a late answer doesn't bring it back
        if n != self.ask_no || matches!(self.card, Card::Idle(_)) {
            return;
        }
        self.card = match code_card(&v) {
            CodeCard::Failed(e) => Card::Failed(e),
            CodeCard::Starting(t) if t == inyon_engine::output::TUNNEL_SLOW => Card::Slow,
            CodeCard::Starting(_) => Card::Starting,
            CodeCard::Code => Card::Code { lan: v.get("waiting").is_some() },
        };
        if v.get("qr").is_some() {
            self.answer = Some(v);
        }
    }

    pub fn cancel(&mut self) {
        self.card = Card::Idle(None);
        self.answer = None;
    }

    /// Follows the phones and pairing.json: a new phone means the code was used, and a code past
    /// its ten minutes, or one the engine closed, has expired.
    pub fn watch(&mut self, phones: &[PairedPhone], pairing_open: bool, now: f64) -> bool {
        if matches!(self.card, Card::Idle(_) | Card::Failed(_)) {
            return false;
        }
        if let Some(p) = phones.iter().find(|p| !self.known.contains(&p.key)) {
            self.end(Ended::Used(p.name.clone()));
            return true;
        }
        if matches!(self.card, Card::Code { .. }) {
            if pairing_open {
                self.seen_open = true;
            }
            if now >= self.asked_at + CODE_MS || (self.seen_open && !pairing_open) {
                self.end(Ended::Expired);
                return true;
            }
        }
        false
    }

    fn end(&mut self, why: Ended) {
        self.card = Card::Idle(Some(why));
        self.answer = None;
        self.seen_open = false;
    }

    /// The card as the window's pages read it; `qr` draws the code.
    pub fn to_json(&self, qr: &dyn Fn(&str) -> Option<String>) -> Value {
        let (card, extra) = match &self.card {
            Card::Idle(None) => ("idle", json!({})),
            Card::Idle(Some(Ended::Used(name))) => ("used", json!({ "phone": name })),
            Card::Idle(Some(Ended::Expired)) => ("expired", json!({})),
            Card::Asking => ("asking", json!({})),
            Card::Starting => ("starting", json!({})),
            Card::Slow => ("slow", json!({})),
            Card::Code { lan } => ("code", json!({ "lan": lan })),
            Card::Failed(e) => ("failed", json!({ "error": e })),
        };
        let mut v = json!({ "card": card });
        for (k, x) in extra.as_object().into_iter().flatten() {
            v[k] = x.clone();
        }
        if let (Card::Code { .. }, Some(a)) = (&self.card, &self.answer) {
            let text = a.get("qr").and_then(Value::as_str).unwrap_or_default();
            v["qr"] = json!(qr(text));
            v["link"] = json!(a.get("link").and_then(Value::as_str).unwrap_or(text));
            if let Some((id, code)) = short_code(a) {
                v["id"] = json!(id);
                v["code"] = json!(code);
            }
            v["expiresAt"] = json!(self.asked_at + CODE_MS);
        }
        v
    }
}

/// LAN, VPN and tunnel as the link takes them from link.json, with the same defaults (on Windows
/// the VPN is off until setup turns it on, since both need a firewall rule).
pub fn reach(cfg: &crate::state::LinkConfig, windows: bool) -> (bool, bool, bool) {
    (cfg.lan.unwrap_or(false), cfg.vpn.unwrap_or(!windows), cfg.tunnel.unwrap_or(true))
}

/// `--network` for setup from the reach switches; None when nothing would be left on.
pub fn network_flag(lan: bool, vpn: bool, tunnel: bool) -> Option<String> {
    let on: Vec<&str> = [(tunnel, "tunnel"), (lan, "lan"), (vpn, "vpn")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
    (!on.is_empty()).then(|| on.join(","))
}

/// How setup run from the window went, from its exit and what it printed: None when it went
/// well, else the line that says why (its last "!" line, or its last line).
pub fn setup_result(ok: bool, printed: &str) -> Option<String> {
    let lines: Vec<&str> = printed.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if ok {
        // a background service that couldn't be set up says so with "!" and still exits 0
        return lines.iter().find(|l| l.starts_with("! Couldn't set up a background service")).map(|l| l.trim_start_matches("! ").to_string());
    }
    let why = lines.iter().rev().find(|l| l.starts_with('!')).or(lines.last()).map(|l| l.trim_start_matches("! ").to_string());
    Some(why.unwrap_or_else(|| "Setup stopped without saying why. Activity has the log.".into()))
}

/// `doctor`'s checks, each with whether it passed and what it said.
pub fn check_list(v: &Value) -> Vec<Value> {
    v.get("checks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            json!({
                "ok": c.get("ok").and_then(Value::as_bool).unwrap_or(true),
                "check": capitalise(c.get("check").and_then(Value::as_str).unwrap_or("A check")),
                "detail": c.get("detail").and_then(Value::as_str).unwrap_or_default(),
            })
        })
        .collect()
}

/// "22:27" today, "14 Oct" before: state.json's times are "2026-10-14 22:27".
/// The day a phone was paired: "today", or as [`seen_short`] gives a day.
pub fn paired_short(date: &str, today: &str) -> String {
    if date == today {
        "today".into()
    } else {
        seen_short(&format!("{date} 00:00"), today)
    }
}

pub fn seen_short(full: &str, today: &str) -> String {
    match full.split_once(' ') {
        Some((date, time)) if date == today => time.to_string(),
        Some((date, _)) => {
            let mut parts = date.split('-');
            let (_, m, d) = (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok()), parts.next().and_then(|d| d.parse::<u32>().ok()));
            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            match (m, d) {
                (Some(m @ 1..=12), Some(d)) => format!("{d} {}", MONTHS[m - 1]),
                _ => date.to_string(),
            }
        }
        None => full.to_string(),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: f64 = 1_000_000.0;

    fn phone(key: &str, name: &str) -> PairedPhone {
        PairedPhone { key: key.into(), name: name.into(), paired: "x".into(), last_seen: None }
    }

    fn code(lan: bool) -> Value {
        let mut v = json!({ "qr": "https://x/#pair?m=a", "link": "https://x/#pair?m=a", "urls": ["wss://a/link"], "relays": [], "short": { "id": "ZS5C3EPG", "code": "801356" } });
        if lan {
            v["waiting"] = json!("tunnel");
        }
        v
    }

    #[test]
    fn the_code_card_waits_for_the_tunnel_and_then_shows_the_code() {
        let starting = json!({ "waiting": "tunnel", "service": false });
        assert!(matches!(code_card(&starting), CodeCard::Starting(t) if t.starts_with("Starting the tunnel")));
        let slow = json!({ "waiting": "tunnel", "qr": "https://x/#pair", "urls": [], "relays": [] });
        assert!(matches!(code_card(&slow), CodeCard::Starting(t) if t.contains("still starting")));
        let lan = json!({ "waiting": "tunnel", "qr": "https://x/#pair", "urls": ["ws://192.168.1.20:4780/link"], "relays": [] });
        assert_eq!(code_card(&lan), CodeCard::Code);
        assert_eq!(code_card(&json!({ "error": "no" })), CodeCard::Failed("no".into()));
    }

    #[test]
    fn a_pairing_that_ends_while_the_tunnel_starts_says_why_and_a_shown_code_stays() {
        assert_eq!(pairing_ended(Err("no engine".into()), None).as_deref(), Some("no engine"));
        assert_eq!(pairing_ended(Ok(()), None).as_deref(), Some("The engine gave no code."));
        assert_eq!(pairing_ended(Err("killed".into()), Some(true)).as_deref(), Some("killed"));
        assert!(pairing_ended(Ok(()), Some(true)).is_some_and(|w| w.contains("before the tunnel came up")));
        assert_eq!(pairing_ended(Err("killed".into()), Some(false)), None);
        assert_eq!(pairing_ended(Ok(()), Some(false)), None);
    }

    #[test]
    fn a_code_goes_from_asking_to_the_tunnel_to_the_code_and_the_page_gets_all_of_it() {
        let phones = vec![phone("abc", "Nothing A142")];
        let mut p = Pairing::default();
        let n = p.ask(&phones, T0).unwrap();
        assert_eq!(p.ask(&phones, T0), None, "a second press while it's being made does nothing");
        p.answer(n, json!({ "waiting": "tunnel", "service": true }));
        assert_eq!(p.card, Card::Starting);
        p.answer(n, json!({ "waiting": "tunnel", "qr": "https://x/#pair", "urls": [], "relays": [] }));
        assert_eq!(p.card, Card::Slow);
        p.answer(n, code(true));
        assert_eq!(p.card, Card::Code { lan: true });
        p.answer(n, code(false));
        let v = p.to_json(&|t| Some(format!("<svg>{t}</svg>")));
        assert_eq!(v["card"], "code");
        assert_eq!(v["lan"], false);
        assert_eq!(v["qr"], "<svg>https://x/#pair?m=a</svg>");
        assert_eq!((v["id"].as_str(), v["code"].as_str()), (Some("ZS5C-3EPG"), Some("801 356")));
        assert_eq!(v["expiresAt"], T0 + CODE_MS);
    }

    #[test]
    fn a_cancelled_or_used_code_stays_ended_when_late_answers_come() {
        let mut p = Pairing::default();
        let n = p.ask(&[], T0).unwrap();
        p.cancel();
        p.answer(n, code(false));
        assert_eq!(p.card, Card::Idle(None));
        // the next ask gets its answers; the old one's are dropped
        let m = p.ask(&[], T0).unwrap();
        p.answer(n, json!({ "error": "old" }));
        assert_eq!(p.card, Card::Asking);
        p.answer(m, code(false));
        assert!(matches!(p.card, Card::Code { .. }));
        // used: a new phone shows up
        assert!(p.watch(&[phone("new", "Pixel 8")], true, T0 + 1000.0));
        assert_eq!(p.card, Card::Idle(Some(Ended::Used("Pixel 8".into()))));
        assert_eq!(p.to_json(&|_| None), json!({ "card": "used", "phone": "Pixel 8" }));
        p.answer(m, code(false));
        assert!(matches!(p.card, Card::Idle(_)));
    }

    #[test]
    fn an_expired_code_clears_after_ten_minutes_or_when_the_engine_closes_it() {
        let mut p = Pairing::default();
        let n = p.ask(&[], T0).unwrap();
        p.answer(n, code(false));
        assert!(!p.watch(&[], false, T0 + CODE_MS - 1.0));
        assert!(p.watch(&[], false, T0 + CODE_MS));
        assert_eq!(p.card, Card::Idle(Some(Ended::Expired)));
        let mut p = Pairing::default();
        let n = p.ask(&[], T0).unwrap();
        p.answer(n, code(false));
        p.watch(&[], true, T0 + 1.0);
        assert!(p.watch(&[], false, T0 + 2.0), "pairing.json was open, then closed early");
    }

    #[test]
    fn setup_output_reach_and_checks_come_back_as_words() {
        let doctor = json!({ "checks": [{ "ok": true, "check": "agents", "detail": "claude" }, { "ok": false, "check": "firewall", "detail": "no rule" }] });
        assert_eq!(check_list(&doctor), vec![json!({ "ok": true, "check": "Agents", "detail": "claude" }), json!({ "ok": false, "check": "Firewall", "detail": "no rule" })]);
        assert_eq!(setup_result(true, "✓ psmux is installed"), None);
        assert_eq!(setup_result(false, "✓ psmux\n! Couldn't reach Cloudflare\nsomething else"), Some("Couldn't reach Cloudflare".into()));
        assert!(setup_result(false, "").is_some());
        assert_eq!(network_flag(true, false, true).as_deref(), Some("tunnel,lan"));
        assert_eq!(network_flag(true, false, false).as_deref(), Some("lan"), "the LAN alone is a choice");
        assert_eq!(network_flag(false, false, false), None, "the last way in stays on");
        let cfg = crate::state::LinkConfig::default();
        assert_eq!(reach(&cfg, true), (false, false, true));
        let cfg = crate::state::LinkConfig { lan: Some(true), tunnel: Some(false), ..Default::default() };
        assert_eq!(reach(&cfg, true), (true, false, false));
    }

    #[test]
    fn last_seen_is_short() {
        assert_eq!(seen_short("2026-10-22 22:27", "2026-10-22"), "22:27");
        assert_eq!(seen_short("2026-10-14 09:01", "2026-10-22"), "14 Oct");
        assert_eq!(seen_short("22:27", "2026-10-22"), "22:27");
        // a phone paired today was paired today, not at midnight
        assert_eq!(paired_short("2026-10-22", "2026-10-22"), "today");
        assert_eq!(paired_short("2026-10-14", "2026-10-22"), "14 Oct");
    }
}
