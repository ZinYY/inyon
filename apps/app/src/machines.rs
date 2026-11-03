//! The Machines pages worked out from the hub's machines, as pages/Machines.tsx and its sheets
//! (NodeUpdate, SetupPanel, ProvidersSheet, PhoneSheet, Sheets.tsx) work them out. Pure, so each
//! rule has a test; the screens in ui/machines.rs only draw what these return.

use inyon_chat_core::groups::matches_query;
use inyon_chat_core::overview::all_panes;
use inyon_protocol::{AgentKind, HostConfig, HostKind, HostState, HostStatus, IntegrationState, NodeUpdateState, OpenCodeProvider, ProviderVia, TmuxState, AGENT_KINDS};

use crate::api::{ConfigFile, PhoneCode, SetupAgent, UploadUsage};
use crate::format;

/// Past this many machines the list gets a search box.
pub const SEARCH_FROM: usize = 8;

const NBSP: char = '\u{a0}';

/// "studio · 2 sessions · 2 agents · responds in 38 ms", wrapping only between items: each dot
/// stays with the item before it and each count with its word, so no line starts with a dot. The
/// time is how long the machine took to report its sessions, not a network round trip.
pub fn meta_line(place: &str, sessions: usize, agents: usize, ms: Option<u64>) -> String {
    let nb = |t: String| t.replace(' ', &NBSP.to_string());
    let mut parts = vec![place.to_string(), nb(format::plural(sessions, "session"))];
    if agents > 0 {
        parts.push(nb(format::plural(agents, "agent")));
    }
    if let Some(ms) = ms {
        parts.push(nb(format!("responds in {}", format::response_time(ms))));
    }
    parts.join(&format!("{NBSP}· "))
}

/// An SSH server's address, and the paired computer it's reached through if it isn't the phone.
pub fn ssh_address(config: &HostConfig, hosts: &[HostState]) -> String {
    let target = config.target.clone().unwrap_or_default();
    match &config.via {
        Some(via) => format!("{target} via {}", hosts.iter().find(|h| &h.config.id == via).map_or("a removed computer", |h| h.config.name.as_str())),
        None => target,
    }
}

/// Where a machine in the list is: this hub, the paired machine's own name, or its SSH address.
pub fn place(h: &HostState, hosts: &[HostState]) -> String {
    match h.config.kind {
        HostKind::Local => "This hub".into(),
        HostKind::Node => h.hostname.clone().unwrap_or_else(|| "Paired machine".into()),
        HostKind::Link => h.hostname.clone().unwrap_or_else(|| "Paired".into()),
        HostKind::Ssh => ssh_address(&h.config, hosts),
    }
}

/// Panes running an agent on a machine.
pub fn agent_count(h: &HostState) -> usize {
    all_panes(std::slice::from_ref(h)).iter().filter(|r| r.pane.agent.is_some()).count()
}

/// "just now", "4 min ago", "3 h ago", "2 d ago".
fn ago(ms: u64, now: f64) -> String {
    let s = ((now - ms as f64) / 1000.0).max(0.0);
    if s < 60.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).round())
    } else if s < 86400.0 {
        format!("{} h ago", (s / 3600.0).round())
    } else {
        format!("{} d ago", (s / 86400.0).round())
    }
}

/// One line about a machine: `online` when it answers, when it was last seen when not, and the
/// reason over a bare "Connecting…".
pub fn host_line(h: &HostState, online: String, now: f64) -> String {
    if h.status == HostStatus::Online {
        return online;
    }
    if let Some(seen) = h.last_seen {
        return format!("Offline · last seen {}", ago(seen, now));
    }
    match (&h.error, h.status) {
        (Some(e), _) => e.clone(),
        (None, HostStatus::Error) => "Not reachable".into(),
        _ => "Connecting…".into(),
    }
}

/// The list's line under a machine's name.
pub fn row_line(h: &HostState, hosts: &[HostState], now: f64) -> String {
    if waiting_to_pair(h) {
        return "Waiting for its pairing command".into();
    }
    host_line(h, meta_line(&place(h, hosts), h.sessions.len(), agent_count(h), h.latency_ms), now)
}

/// A machine added with a pairing command that hasn't run it yet.
pub fn waiting_to_pair(h: &HostState) -> bool {
    h.config.kind == HostKind::Node && h.config.paired_at.is_none() && h.status != HostStatus::Online
}

/// Online and short of what Inyon needs: its page opens on Setup.
pub fn needs_setup(h: &HostState) -> bool {
    h.status == HostStatus::Online && (h.tmux != TmuxState::Ok || h.agents.is_empty())
}

/// A short label for the machine list: the update itself is on the machine's page.
pub fn node_update_label(h: &HostState) -> Option<&'static str> {
    let u = h.node_update.as_ref()?;
    match u.state {
        Some(NodeUpdateState::Updated) => None,
        Some(NodeUpdateState::Updating) => Some("Updating…"),
        Some(NodeUpdateState::Failed) => Some("Update failed"),
        None => Some("Update available"),
    }
}

/// Online machines first, then the rest, each group in the order they were added.
pub fn online_first(hosts: &[HostState]) -> Vec<&HostState> {
    let mut v: Vec<&HostState> = hosts.iter().collect();
    v.sort_by_key(|h| h.status != HostStatus::Online);
    v
}

/// The list as shown: online first, narrowed by the search box.
pub fn shown<'a>(hosts: &'a [HostState], q: &str) -> Vec<&'a HostState> {
    online_first(hosts)
        .into_iter()
        .filter(|h| {
            if q.is_empty() {
                return true;
            }
            let kind = h.machine_kind.map(|k| format!("{k:?}").to_lowercase()).unwrap_or_default();
            let agents: Vec<&str> = h.agents.iter().map(|a| a.as_str()).collect();
            let text = format!("{} {} {kind} {} {}", h.config.name, h.hostname.as_deref().unwrap_or(""), h.os.as_deref().unwrap_or(""), agents.join(" "));
            matches_query(&text, q)
        })
        .collect()
}

/// The grey line at the top of a machine's page: how it's reached, its inyon and system.
pub fn page_line(h: &HostState, hosts: &[HostState]) -> String {
    let c = &h.config;
    let mut s = match c.kind {
        HostKind::Local => "This hub".to_string(),
        HostKind::Node if waiting_to_pair(h) => "Not paired yet".into(),
        HostKind::Node => format!("Paired machine{}", h.hostname.as_ref().map(|n| format!(" · {n}")).unwrap_or_default()),
        HostKind::Link => format!("Paired{}", h.hostname.as_ref().map(|n| format!(" · {n}")).unwrap_or_default()),
        HostKind::Ssh => {
            let via = c.via.as_ref().map(|v| hosts.iter().find(|x| &x.config.id == v).map_or("a removed computer".to_string(), |x| x.config.name.clone()));
            format!("ssh {}{}{}", c.target.as_deref().unwrap_or(""), c.port.map(|p| format!(" -p {p}")).unwrap_or_default(), via.map(|v| format!(" from {v}")).unwrap_or_default())
        }
    };
    if let Some(v) = &h.node_version {
        s.push_str(&format!(" · inyon {v}"));
    }
    if let Some(os) = &h.os {
        s.push_str(&format!(" · {os}"));
    }
    if let Some(ms) = h.latency_ms {
        s.push_str(&format!(" · responds in {}", format::response_time(ms)));
    }
    s
}

/// What the update card on a machine's page says, and whether it has a button.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateCard {
    pub state: UpdateLook,
    pub title: String,
    pub detail: Option<String>,
    /// "Update" or "Try again", when the machine can update itself from here
    pub button: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateLook {
    Available,
    Updating,
    Updated,
    Failed,
}

/// The update card (NodeUpdate.tsx). `refused` is the hub's answer when it turned the update down.
pub fn update_card(h: &HostState, refused: Option<&str>) -> Option<UpdateCard> {
    let u = h.node_update.as_ref()?;
    let online = h.status == HostStatus::Online;
    let name = &h.config.name;
    let failed = u.state == Some(NodeUpdateState::Failed) || refused.is_some();
    let state = match u.state {
        Some(NodeUpdateState::Updating) => UpdateLook::Updating,
        Some(NodeUpdateState::Updated) => UpdateLook::Updated,
        _ if failed => UpdateLook::Failed,
        _ => UpdateLook::Available,
    };
    let title = match state {
        UpdateLook::Updating => "Updating inyon…".to_string(),
        UpdateLook::Updated => format!("Updated to inyon {}", u.latest),
        UpdateLook::Failed => "Couldn't update inyon".into(),
        UpdateLook::Available => format!("inyon {} · update available", h.node_version.as_deref().unwrap_or("")),
    };
    let detail = match state {
        UpdateLook::Updating => Some("It restarts and reconnects by itself, usually within a minute.".to_string()),
        UpdateLook::Updated => None,
        UpdateLook::Failed => Some(refused.map(str::to_string).or_else(|| u.error.clone()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Something went wrong.".into())),
        UpdateLook::Available if !u.supported => Some(format!("This version can't update itself. Run the setup command on {name} again to get {}.", u.latest)),
        UpdateLook::Available if online => Some(format!("Version {} is ready. inyon restarts for a moment while it updates.", u.latest)),
        UpdateLook::Available => Some(format!("Version {} is ready. Update it once {name} is back online.", u.latest)),
    };
    let button = (u.supported && !matches!(state, UpdateLook::Updating | UpdateLook::Updated)).then_some(if failed { "Try again" } else { "Update" });
    Some(UpdateCard { state, title, detail, button })
}

/// Whether the machine menu offers "Update to inyon x": the machine can do it now.
pub fn can_update(h: &HostState) -> Option<String> {
    let u = h.node_update.as_ref()?;
    (u.supported && !matches!(u.state, Some(NodeUpdateState::Updating | NodeUpdateState::Updated)) && h.status == HostStatus::Online).then(|| u.latest.clone())
}

/// The Integration tab's state line.
pub fn integration_text(s: IntegrationState) -> &'static str {
    match s {
        IntegrationState::Installed => "On",
        IntegrationState::Missing => "Off",
        IntegrationState::Unknown => "Unknown",
        IntegrationState::Removed => "~/.claude/settings.json was replaced and no longer runs the Claude integration, so live limits and context stopped updating.",
        IntegrationState::Outdated => "The Claude integration on this machine is an older version. Update it for accurate limits after account switches and compaction.",
    }
}

/// `/home/sam/…` as `~/…`, as the Config and Uploads tabs show paths.
pub fn tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/home/") {
        if let Some(i) = rest.find('/') {
            return format!("~{}", &rest[i..]);
        }
        return "~".into();
    }
    path.to_string()
}

/// A folder as the New session sheet offers it: the home folder as `~`, on Linux, macOS or as root.
pub fn home_folder(path: &str) -> String {
    for prefix in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return match rest.find('/') {
                Some(i) => format!("~{}", &rest[i..]),
                None => "~".into(),
            };
        }
    }
    match path.strip_prefix("/root") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The folders sessions on a machine run in, newest first, at most six.
pub fn recent_folders(hosts: &[HostState], host: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for r in all_panes(hosts).iter().filter(|r| r.host.config.id == host) {
        let f = home_folder(&r.pane.cwd);
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out.truncate(6);
    out
}

/// "codex-cli 0.49.0" and "2.1.296 (Claude Code)" as just the number.
pub fn short_version(v: Option<&str>) -> String {
    let Some(v) = v else { return String::new() };
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            let mut end = i;
            let mut dots = 0;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            // one or more ".digits" after the first number
            while end + 1 < b.len() && b[end] == b'.' && b[end + 1].is_ascii_digit() {
                end += 1;
                while end < b.len() && b[end].is_ascii_digit() {
                    end += 1;
                }
                dots += 1;
            }
            if dots > 0 {
                return v[start..end].to_string();
            }
            i = end;
        } else {
            i += 1;
        }
    }
    v.to_string()
}

/// An agent's row in the Setup tab: its version, whether it's current, and its account.
pub fn setup_state(a: &SetupAgent) -> String {
    if !a.installed {
        return "Not installed".into();
    }
    // OpenCode signs in per provider; its free models need none
    let account = if a.kind == AgentKind::Opencode {
        match a.providers.as_deref() {
            Some(p) if !p.is_empty() => p.join(", "),
            _ => "free models only".into(),
        }
    } else if a.signed_in {
        "signed in".into()
    } else {
        "not signed in".into()
    };
    let version = short_version(a.version.as_deref());
    let current = (a.latest.is_some() && !a.outdated).then_some("up to date".to_string());
    [Some(version), current, Some(account)].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// How a connected OpenCode provider got that way, in a word or two.
pub fn provider_how(p: &OpenCodeProvider) -> String {
    match p.via {
        Some(ProviderVia::Env) => format!("From {}", p.env.as_ref().and_then(|e| e.first()).map_or("the environment", |s| s.as_str())),
        Some(ProviderVia::Credential) => match p.r#type.as_deref() {
            Some("oauth") => "Signed in".into(),
            Some("api") => "API key".into(),
            _ => "Connected".into(),
        },
        None if p.id == "opencode" => "Free models, no sign-in".into(),
        None => "From OpenCode’s config".into(),
    }
}

/// The providers not connected yet, narrowed by the search box (name or id).
pub fn providers_to_connect<'a>(all: &'a [OpenCodeProvider], q: &str) -> Vec<&'a OpenCodeProvider> {
    let q = q.trim().to_lowercase();
    all.iter().filter(|p| !p.connected && (q.is_empty() || p.name.to_lowercase().contains(&q) || p.id.contains(&q))).collect()
}

/// The age the clean button cleans: the hub's keep time, or 7 days when it has none or keeps
/// uploads forever.
pub fn clean_days(u: &UploadUsage) -> f64 {
    match u.days {
        Some(d) if d > 0.0 => d,
        _ => 7.0,
    }
}

/// "Clean older than 7 days (3)": the count is the files already past the hub's keep time.
pub fn clean_label(u: &UploadUsage) -> String {
    let d = clean_days(u);
    let expired = match (u.expired, u.days) {
        (Some(n), Some(days)) if n > 0 && days > 0.0 => format!(" ({n})"),
        _ => String::new(),
    };
    format!("Clean older than {d} day{}{expired}", if d == 1.0 { "" } else { "s" })
}

/// The agents a machine's config files belong to, in the order the hub lists them.
pub fn config_groups(files: &[ConfigFile]) -> Vec<AgentKind> {
    let mut out = vec![];
    for f in files {
        if !out.contains(&f.agent) {
            out.push(f.agent);
        }
    }
    out
}

/// The file the Config tab opens first: one that exists, or the first one.
pub fn first_config(files: &[ConfigFile]) -> Option<String> {
    files.iter().find(|f| f.exists).or(files.first()).map(|f| f.path.clone())
}

/// Why a JSON config won't save, or nothing when it parses (or is empty).
pub fn json_error(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(text).err().map(|e| e.to_string())
}

/// What the New session sheet offers: the installed agents in the catalog's order, then a terminal.
pub fn agent_choices(installed: &[AgentKind]) -> Vec<Option<AgentKind>> {
    let mut v: Vec<Option<AgentKind>> = AGENT_KINDS.iter().copied().filter(|k| installed.contains(k)).map(Some).collect();
    v.push(None);
    v
}

/// The agent a new session starts with: Claude Code when it's there, else the first installed.
pub fn default_agent(installed: &[AgentKind]) -> Option<AgentKind> {
    if installed.contains(&AgentKind::Claude) {
        Some(AgentKind::Claude)
    } else {
        installed.first().copied()
    }
}

/// The pairing command for the system picked, or the hub's only one.
pub fn pair_command(p: &crate::api::Pairing, windows: bool) -> &str {
    match &p.commands {
        Some(c) if windows => &c.windows,
        Some(c) => &c.unix,
        None => &p.command,
    }
}

/// "K7Q4MX2T" as K7Q4-MX2T, the way the phone asks for it.
pub fn show_id(id: &str) -> String {
    if id.len() > 4 && id.is_char_boundary(4) {
        format!("{}-{}", &id[..4], &id[4..])
    } else {
        id.to_string()
    }
}

/// "493812" as 493 812.
pub fn show_code(code: &str) -> String {
    if code.len() > 3 && code.is_char_boundary(3) {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// Whole minutes left on a phone code, never below zero.
pub fn minutes_left(c: &PhoneCode, now: f64) -> u64 {
    ((c.expires_at - now) / 60_000.0).round().max(0.0) as u64
}

/// What a hub's pairing link says about the hub: its id and its certificate's fingerprint, which the
/// phone shows before it pairs, so the two screens can be compared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HubLink {
    pub url: String,
    pub hub_id: Option<String>,
    pub tls_fingerprint: Option<String>,
}

fn query_value(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then(|| crate::route::decode(&v.replace('+', " ")))
    })
}

/// `inyon://hub?u=…&c=…&h=…&f=…` (or the same after `#hub?` on a web address), as hubs.ts
/// `parseHubLink` reads it.
pub fn parse_hub_link(text: &str) -> Option<HubLink> {
    let t = text.trim();
    let q = t.strip_prefix("inyon://hub?").or_else(|| {
        let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://"))?;
        let (before, after) = rest.split_once("#hub?")?;
        (!before.is_empty() && !before.contains(char::is_whitespace)).then_some(after)
    })?;
    let url = query_value(q, "u").unwrap_or_default();
    let code = query_value(q, "c").unwrap_or_default();
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    if host.is_empty() || host.contains('/') || host.contains(char::is_whitespace) {
        return None;
    }
    let (id, digits) = code.split_once(':')?;
    if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) || digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hex = |s: Option<String>, n: usize| s.map(|v| v.to_lowercase()).filter(|v| v.len() == n && v.bytes().all(|b| b.is_ascii_hexdigit()));
    Some(HubLink { url, hub_id: hex(query_value(q, "h"), 16), tls_fingerprint: hex(query_value(q, "f"), 64) })
}

/// A hub id or fingerprint in groups of four, to compare on two screens: "3f9c 1a7e 2b6d 4c80".
pub fn groups_of(hex: &str) -> String {
    hex.as_bytes().chunks(4).take(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

/// The address a phone sends a camera to: the download page, which hands the code to the app.
pub fn phone_qr_link(link: &str) -> String {
    format!("{}#hub?{}", inyon_protocol::WEB_URL, link.strip_prefix("inyon://hub?").unwrap_or(link))
}

/// A QR code as one SVG path, a module per unit with a one-module border: `(size, d)` for a
/// `0 0 size size` view box. None when the text is too long for a code.
pub fn qr_path(text: &str) -> Option<(usize, String)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width();
    let mut d = String::new();
    for (i, c) in code.to_colors().into_iter().enumerate() {
        if c == qrcode::Color::Dark {
            d.push_str(&format!("M{} {}h1v1h-1z", i % w + 1, i / w + 1));
        }
    }
    Some((w + 2, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_phone_code_draws_as_one_path() {
        let (size, d) = qr_path("https://inyon.abhashchakraborty.tech/#hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812").unwrap();
        // the smallest version that holds it, plus the border
        assert!(size >= 23 && size % 4 == 3, "{size}");
        // the finder pattern's corner sits just inside the border
        assert!(d.starts_with("M1 1h1v1h-1z"));
        assert!(qr_path(&"x".repeat(4000)).is_none());
    }

    const NOW: f64 = 1_793_448_000_000.0;

    fn host(v: serde_json::Value) -> HostState {
        let mut base = json!({ "config": { "id": "box", "name": "studio", "kind": "local" }, "status": "online",
            "integration": "installed", "tmux": "ok", "agents": ["claude"], "sessions": [], "updatedAt": 1 });
        merge(&mut base, v);
        serde_json::from_value(base).unwrap()
    }

    fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
        match (a, b) {
            (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                for (k, v) in b {
                    merge(a.entry(k).or_insert(serde_json::Value::Null), v);
                }
            }
            (a, b) => *a = b,
        }
    }

    #[test]
    fn the_meta_line_wraps_only_between_items() {
        let s = meta_line("studio", 2, 1, Some(38));
        assert_eq!(s, "studio\u{a0}· 2\u{a0}sessions\u{a0}· 1\u{a0}agent\u{a0}· responds\u{a0}in\u{a0}38\u{a0}ms");
        // no line can start with a dot: every dot has a no-break space before it
        assert!(!s.contains(" ·"));
        assert_eq!(meta_line("This hub", 1, 0, None), "This hub\u{a0}· 1\u{a0}session");
    }

    #[test]
    fn machines_that_answer_come_first_and_the_search_narrows_them() {
        let hosts = vec![
            host(json!({ "config": { "id": "a", "name": "old-box", "kind": "ssh", "target": "me@old" }, "status": "error", "error": "Connection refused" })),
            host(json!({ "config": { "id": "b", "name": "studio" }, "os": "Linux" })),
            host(json!({ "config": { "id": "c", "name": "laptop", "kind": "node" }, "machineKind": "laptop", "agents": ["codex"] })),
        ];
        let names = |v: Vec<&HostState>| v.iter().map(|h| h.config.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(shown(&hosts, "")), ["studio", "laptop", "old-box"]);
        assert_eq!(names(shown(&hosts, "codex")), ["laptop"]);
        assert_eq!(names(shown(&hosts, "linux")), ["studio"]);
        assert_eq!(names(shown(&hosts, "nothing like it")), Vec::<String>::new());
        assert_eq!(row_line(&hosts[0], &hosts, NOW), "Connection refused");
        assert_eq!(place(&hosts[0], &hosts), "me@old");
    }

    #[test]
    fn an_offline_machine_says_when_it_was_last_seen() {
        let h = host(json!({ "status": "connecting", "lastSeen": (NOW as u64) - 3 * 3_600_000 }));
        assert_eq!(row_line(&h, &[], NOW), "Offline · last seen 3 h ago");
        assert_eq!(row_line(&host(json!({ "status": "connecting" })), &[], NOW), "Connecting…");
        assert_eq!(row_line(&host(json!({ "status": "error" })), &[], NOW), "Not reachable");
    }

    #[test]
    fn a_machine_waiting_for_its_pairing_command_says_so() {
        let h = host(json!({ "config": { "kind": "node" }, "status": "connecting" }));
        assert!(waiting_to_pair(&h));
        assert_eq!(row_line(&h, &[], NOW), "Waiting for its pairing command");
        assert_eq!(page_line(&h, &[]), "Not paired yet");
        let paired = host(json!({ "config": { "kind": "node", "pairedAt": 5 }, "status": "connecting", "hostname": "mbp" }));
        assert!(!waiting_to_pair(&paired));
        assert_eq!(page_line(&paired, &[]), "Paired machine · mbp");
    }

    #[test]
    fn the_page_line_says_how_a_machine_is_reached() {
        let jump = host(json!({ "config": { "id": "j", "name": "jump", "kind": "node", "pairedAt": 1 } }));
        let ssh = host(
            json!({ "config": { "id": "s", "name": "srv", "kind": "ssh", "target": "deploy@srv", "port": 2222, "via": "j" }, "nodeVersion": "0.23.1", "os": "Ubuntu", "latencyMs": 38 }),
        );
        assert_eq!(page_line(&ssh, &[jump, ssh.clone()]), "ssh deploy@srv -p 2222 from jump · inyon 0.23.1 · Ubuntu · responds in 38 ms");
        assert_eq!(page_line(&host(json!({})), &[]), "This hub");
    }

    #[test]
    fn setup_comes_first_on_a_machine_without_tmux_or_agents() {
        assert!(needs_setup(&host(json!({ "tmux": "missing" }))));
        assert!(needs_setup(&host(json!({ "agents": [] }))));
        assert!(!needs_setup(&host(json!({}))));
        assert!(!needs_setup(&host(json!({ "status": "error", "agents": [] }))));
    }

    #[test]
    fn the_update_card_follows_the_hub() {
        let u = |state: serde_json::Value, extra: serde_json::Value| {
            let mut v = json!({ "nodeVersion": "0.22.0", "nodeUpdate": { "latest": "0.23.1", "supported": true, "state": state } });
            merge(&mut v, extra);
            host(v)
        };
        let c = update_card(&u(json!(null), json!({})), None).unwrap();
        assert_eq!((c.title.as_str(), c.button), ("inyon 0.22.0 · update available", Some("Update")));
        assert_eq!(c.detail.as_deref(), Some("Version 0.23.1 is ready. inyon restarts for a moment while it updates."));
        let c = update_card(&u(json!("updating"), json!({})), None).unwrap();
        assert_eq!((c.state, c.title.as_str(), c.button), (UpdateLook::Updating, "Updating inyon…", None));
        let c = update_card(&u(json!("updated"), json!({})), None).unwrap();
        assert_eq!((c.title.as_str(), c.detail, c.button), ("Updated to inyon 0.23.1", None, None));
        let c = update_card(&u(json!("failed"), json!({ "nodeUpdate": { "error": "Disk full" } })), None).unwrap();
        assert_eq!((c.title.as_str(), c.detail.as_deref(), c.button), ("Couldn't update inyon", Some("Disk full"), Some("Try again")));
        // the hub turning it down reads as a failure until it says something new
        let c = update_card(&u(json!(null), json!({})), Some("Not now")).unwrap();
        assert_eq!((c.state, c.detail.as_deref()), (UpdateLook::Failed, Some("Not now")));
        let c = update_card(&u(json!(null), json!({ "nodeUpdate": { "supported": false } })), None).unwrap();
        assert_eq!(c.button, None);
        assert_eq!(c.detail.as_deref(), Some("This version can't update itself. Run the setup command on studio again to get 0.23.1."));
        let c = update_card(&u(json!(null), json!({ "status": "connecting" })), None).unwrap();
        assert_eq!(c.detail.as_deref(), Some("Version 0.23.1 is ready. Update it once studio is back online."));
        assert_eq!(update_card(&host(json!({})), None), None);
        // the list's short label, and the menu's item
        assert_eq!(node_update_label(&u(json!(null), json!({}))), Some("Update available"));
        assert_eq!(node_update_label(&u(json!("updated"), json!({}))), None);
        assert_eq!(can_update(&u(json!(null), json!({}))).as_deref(), Some("0.23.1"));
        assert_eq!(can_update(&u(json!("updating"), json!({}))), None);
        assert_eq!(can_update(&u(json!(null), json!({ "status": "connecting" }))), None);
    }

    #[test]
    fn paths_and_versions_read_short() {
        assert_eq!(tilde("/home/sam/.claude/settings.json"), "~/.claude/settings.json");
        assert_eq!(tilde("/etc/x"), "/etc/x");
        assert_eq!(home_folder("/Users/sam/code"), "~/code");
        assert_eq!(home_folder("/root"), "~");
        assert_eq!(home_folder("/rootless"), "/rootless");
        assert_eq!(short_version(Some("codex-cli 0.49.0")), "0.49.0");
        assert_eq!(short_version(Some("2.1.296 (Claude Code)")), "2.1.296");
        assert_eq!(short_version(Some("v12")), "v12");
        assert_eq!(short_version(None), "");
    }

    #[test]
    fn the_setup_tab_names_each_agents_state() {
        let a = |v: serde_json::Value| serde_json::from_value::<SetupAgent>(v).unwrap();
        assert_eq!(setup_state(&a(json!({ "kind": "claude", "installed": false }))), "Not installed");
        assert_eq!(
            setup_state(&a(json!({ "kind": "claude", "installed": true, "version": "2.1.296 (Claude Code)", "latest": "2.1.296", "signedIn": true }))),
            "2.1.296 · up to date · signed in"
        );
        assert_eq!(setup_state(&a(json!({ "kind": "codex", "installed": true, "version": "codex-cli 0.48.0", "latest": "0.49.0", "outdated": true }))), "0.48.0 · not signed in");
        assert_eq!(setup_state(&a(json!({ "kind": "opencode", "installed": true, "version": "1.0.0" }))), "1.0.0 · free models only");
        assert_eq!(setup_state(&a(json!({ "kind": "opencode", "installed": true, "providers": ["Anthropic", "OpenAI"] }))), "Anthropic, OpenAI");
    }

    #[test]
    fn providers_say_how_they_connected_and_the_search_narrows_the_rest() {
        let list: Vec<OpenCodeProvider> = serde_json::from_value(json!([
            { "id": "anthropic", "name": "Anthropic", "connected": true, "via": "credential", "type": "oauth" },
            { "id": "openai", "name": "OpenAI", "connected": true, "via": "env", "env": ["OPENAI_API_KEY"] },
            { "id": "opencode", "name": "OpenCode Zen", "connected": true },
            { "id": "groq", "name": "Groq", "connected": false },
            { "id": "google", "name": "Google", "connected": false },
        ]))
        .unwrap();
        assert_eq!(list.iter().filter(|p| p.connected).map(provider_how).collect::<Vec<_>>(), ["Signed in", "From OPENAI_API_KEY", "Free models, no sign-in"]);
        assert_eq!(providers_to_connect(&list, "").len(), 2);
        assert_eq!(providers_to_connect(&list, " GRO").iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["groq"]);
        assert!(providers_to_connect(&list, "anthropic").is_empty());
    }

    #[test]
    fn uploads_clean_at_the_hubs_keep_time() {
        let u = |days: Option<f64>, expired: Option<u64>| UploadUsage { dir: "/home/sam/.inyon/uploads".into(), files: 4, bytes: 2048.0, days, expired };
        assert_eq!(clean_label(&u(Some(14.0), Some(3))), "Clean older than 14 days (3)");
        assert_eq!(clean_label(&u(Some(1.0), Some(0))), "Clean older than 1 day");
        // kept forever: a week, and no count
        assert_eq!(clean_label(&u(Some(0.0), Some(2))), "Clean older than 7 days");
        assert_eq!(clean_days(&u(None, None)), 7.0);
    }

    #[test]
    fn config_files_group_by_agent_and_json_is_checked() {
        let files: Vec<ConfigFile> = serde_json::from_value(json!([
            { "agent": "claude", "label": "settings.json", "path": "/home/s/.claude/settings.json", "format": "json", "exists": false, "size": 0 },
            { "agent": "codex", "label": "config.toml", "path": "/home/s/.codex/config.toml", "format": "toml", "exists": true, "size": 120 },
            { "agent": "claude", "label": "CLAUDE.md", "path": "/home/s/.claude/CLAUDE.md", "format": "md", "exists": true, "size": 40 },
        ]))
        .unwrap();
        assert_eq!(config_groups(&files), [AgentKind::Claude, AgentKind::Codex]);
        assert_eq!(first_config(&files).as_deref(), Some("/home/s/.codex/config.toml"));
        assert_eq!(json_error("{\"a\": 1}"), None);
        assert_eq!(json_error("  "), None);
        assert!(json_error("{\"a\": }").is_some());
    }

    #[test]
    fn a_new_session_offers_the_installed_agents_then_a_terminal() {
        assert_eq!(agent_choices(&[AgentKind::Codex, AgentKind::Claude]), [Some(AgentKind::Claude), Some(AgentKind::Codex), None]);
        assert_eq!(default_agent(&[AgentKind::Codex, AgentKind::Claude]), Some(AgentKind::Claude));
        assert_eq!(default_agent(&[AgentKind::Codex]), Some(AgentKind::Codex));
        assert_eq!(default_agent(&[]), None);
        let hosts = vec![host(json!({ "sessions": [
            { "id": "$1", "name": "a", "attached": 0, "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "activity": 1, "panes": [
                { "id": "%1", "index": 0, "active": true, "command": "zsh", "cwd": "/home/sam/code/app", "title": "", "width": 80, "height": 24, "dead": false },
                { "id": "%2", "index": 1, "active": false, "command": "zsh", "cwd": "/home/sam/code/app", "title": "", "width": 80, "height": 24, "dead": false },
                { "id": "%3", "index": 2, "active": false, "command": "zsh", "cwd": "C:\\work", "title": "", "width": 80, "height": 24, "dead": false }
            ] }] }
        ] }))];
        assert_eq!(recent_folders(&hosts, "box"), ["~/code/app", "C:\\work"]);
        assert!(recent_folders(&hosts, "other").is_empty());
    }

    #[test]
    fn a_phone_code_reads_as_the_app_asks_for_it() {
        assert_eq!(show_id("K7Q4MX2T"), "K7Q4-MX2T");
        assert_eq!(show_code("493812"), "493 812");
        let c = PhoneCode {
            id: "K7Q4MX2T".into(),
            code: "493812".into(),
            link: "inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio&h=3F9C1A7E2B6D4C80".into(),
            expires_at: NOW + 9.6 * 60_000.0,
        };
        assert_eq!(minutes_left(&c, NOW), 10);
        assert_eq!(minutes_left(&c, NOW + 3_600_000.0), 0);
        let link = parse_hub_link(&c.link).unwrap();
        assert_eq!(link.url, "https://hub.example");
        assert_eq!(link.hub_id.as_deref(), Some("3f9c1a7e2b6d4c80"));
        assert_eq!(link.tls_fingerprint, None);
        assert_eq!(groups_of("3f9c1a7e2b6d4c80ffff"), "3f9c 1a7e 2b6d 4c80");
        assert_eq!(phone_qr_link(&c.link), "https://inyon.tech/#hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio&h=3F9C1A7E2B6D4C80");
        // the same code after a web address, and ones that aren't a hub's
        assert!(parse_hub_link("https://inyon.tech/#hub?u=http%3A%2F%2F10.0.0.2%3A8080&c=ABCDEFGH%3A123456").is_some());
        assert_eq!(parse_hub_link("inyon://hub?u=https%3A%2F%2Fhub.example%2Fpath&c=K7Q4MX2T%3A493812"), None);
        assert_eq!(parse_hub_link("inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A4938"), None);
        assert_eq!(parse_hub_link("inyon://pair?m=abc"), None);
    }
}
