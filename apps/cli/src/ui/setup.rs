//! `inyon setup`: four short screens (what this machine is, where it answers, whether it runs
//! in the background, a summary), then the engine takes over for the pairing code. Keys and
//! mouse both work; with no terminal, or --yes, the flags answer the same questions.

use std::ops::Range;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{banner, fit, Theme};
use crate::networks::{Kind, Network};
use crate::sys::Platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleChoice {
    /// the server phones sign in to
    Hub,
    /// paired with the phone directly
    Node,
    /// added to a hub
    Join,
}

impl RoleChoice {
    pub fn parse(s: &str) -> Option<RoleChoice> {
        match s {
            "hub" => Some(RoleChoice::Hub),
            "node" | "link" => Some(RoleChoice::Node),
            "join" => Some(RoleChoice::Join),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RoleChoice::Hub => "hub",
            RoleChoice::Node => "node",
            RoleChoice::Join => "join",
        }
    }
}

/// Where a hub answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bind {
    Localhost,
    Lan,
    Vpn(String),
    Own(String),
}

/// Everything setup decides, from the screens or the flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub role: RoleChoice,
    pub tunnel: bool,
    pub lan: bool,
    pub vpn: bool,
    pub own_url: String,
    pub port: String,
    pub bind: Bind,
    pub autostart: bool,
    pub hub_url: String,
    /// the token and certificate from the hub's Add a machine command, when it was pasted whole
    pub hub_token: String,
    pub hub_cert: String,
    /// the hub whose command gave the token above, and that token
    from_command: Option<(String, String)>,
    pub networks: Vec<Network>,
    pub platform: Platform,
}

impl Plan {
    pub fn new(platform: Platform, networks: Vec<Network>) -> Plan {
        Plan {
            role: RoleChoice::Node,
            tunnel: true,
            lan: false,
            vpn: platform != Platform::Windows,
            own_url: String::new(),
            port: "4780".into(),
            bind: Bind::Localhost,
            autostart: true,
            hub_url: String::new(),
            hub_token: String::new(),
            hub_cert: String::new(),
            from_command: None,
            networks,
            platform,
        }
    }

    pub fn vpn_networks(&self) -> Vec<&Network> {
        self.networks.iter().filter(|n| n.kind != Kind::Lan).collect()
    }

    pub fn lan_ip(&self) -> Option<&str> {
        self.networks.iter().find(|n| n.kind == Kind::Lan).map(|n| n.ip.as_str())
    }

    pub fn port_number(&self) -> u16 {
        self.port.parse().unwrap_or(if self.role == RoleChoice::Hub { 4777 } else { 4780 })
    }

    /// Reads `--network` (a comma list: tunnel, lan, vpn, localhost, an address or URL).
    pub fn set_network(&mut self, spec: &str) -> Result<(), String> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
        if self.role == RoleChoice::Hub {
            let Some(first) = parts.first() else { return Ok(()) };
            self.bind = match *first {
                "localhost" => Bind::Localhost,
                "lan" => Bind::Lan,
                "vpn" => Bind::Vpn(self.vpn_networks().first().map(|n| n.ip.clone()).ok_or("no VPN network was found on this machine")?),
                other => Bind::Own(other.to_string()),
            };
            return Ok(());
        }
        self.tunnel = false;
        self.lan = false;
        self.vpn = false;
        for p in parts {
            match p {
                "tunnel" => self.tunnel = true,
                "lan" => self.lan = true,
                "vpn" => self.vpn = true,
                u if u.starts_with("ws://") || u.starts_with("wss://") => self.own_url = u.to_string(),
                other => return Err(format!("--network doesn't know {other}; use tunnel, lan, vpn or a wss:// address")),
            }
        }
        Ok(())
    }

    /// Takes the hub to join from what was pasted: its address, or its whole Add a machine
    /// command, whose token and certificate are kept too.
    pub fn set_join(&mut self, input: &str) -> Result<(), String> {
        let c = crate::commands::parse_add_machine(input)?;
        match c.token {
            Some(t) => {
                self.hub_token = t.clone();
                self.hub_cert = c.cert.unwrap_or_default();
                self.from_command = Some((c.hub.clone(), t));
            }
            // only an address: what another hub's command carried isn't sent to this one, a token
            // typed in apart is
            None => {
                if let Some((_, token)) = self.from_command.take_if(|(hub, _)| *hub != c.hub) {
                    if self.hub_token == token {
                        self.hub_token.clear();
                    }
                    self.hub_cert.clear();
                }
            }
        }
        self.hub_url = c.hub;
        Ok(())
    }

    /// The summary lines, which the last screen and the plain-text run both show.
    pub fn summary(&self) -> Vec<(String, String)> {
        let mut out = vec![(
            "Role".into(),
            match self.role {
                RoleChoice::Hub => "hub, the server phones sign in to".to_string(),
                RoleChoice::Node => "paired with the phone".to_string(),
                RoleChoice::Join => format!("machine on the hub {}", self.hub_url),
            },
        )];
        match self.role {
            RoleChoice::Hub => {
                let where_ = match &self.bind {
                    Bind::Localhost => "this machine only (localhost)".to_string(),
                    Bind::Lan => format!("the LAN{}", self.lan_ip().map(|ip| format!(" ({ip})")).unwrap_or_default()),
                    Bind::Vpn(ip) => format!("the VPN ({ip})"),
                    Bind::Own(a) => a.clone(),
                };
                out.push(("Answers on".into(), format!("{where_}, port {}", self.port_number())));
            }
            RoleChoice::Node => {
                let mut ways = Vec::new();
                if self.tunnel {
                    ways.push("a tunnel (any network)".to_string());
                }
                if self.lan {
                    ways.push("the LAN".to_string());
                }
                if self.vpn {
                    // the link answers on every private network it finds
                    let vpns = self.vpn_networks();
                    if vpns.is_empty() {
                        ways.push("VPN networks".to_string());
                    }
                    ways.extend(vpns.iter().map(|n| format!("{} ({})", n.kind.label(), n.ip)));
                }
                if !self.own_url.is_empty() {
                    ways.push(self.own_url.clone());
                }
                if ways.is_empty() {
                    ways.push("nothing yet".into());
                }
                out.push(("Answers on".into(), format!("{}, port {}", ways.join(", "), self.port_number())));
            }
            RoleChoice::Join => {}
        }
        out.push(("Background".into(), if self.autostart { "starts at login, comes back after a crash".into() } else { "only while this window is open".into() }));
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Role,
    Network,
    Background,
    SignIn,
    Summary,
}

impl Step {
    fn title(self) -> &'static str {
        match self {
            Step::Role => "What is this machine to the app?",
            Step::Network => "Where should it answer?",
            Step::Background => "Keep it running?",
            Step::SignIn => "Signing in",
            Step::Summary => "Ready",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stay,
    Quit,
    Done,
}

/// One row on a screen: what it is and how a key or click changes the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Radio { label: String, note: String, on: bool, disabled: bool },
    Check { label: String, note: String, on: bool },
    Text { label: String, value: String, digits: bool },
    Info(String),
}

pub struct Wizard {
    pub plan: Plan,
    pub step: Step,
    pub cursor: usize,
    /// the engine download running behind the screens, as a line for the footer
    /// where each item was drawn last, for the mouse
    hit: Vec<(u16, usize)>,
    /// the row and columns of "Enter next" and "← back" in the hint line
    next_hit: Option<(u16, Range<u16>)>,
    back_hit: Option<(u16, Range<u16>)>,
    /// why Enter didn't go on (the Join screen's address or token)
    pub problem: Option<String>,
}

/// What was typed into Hub as it's shown: a token in a command typed or pasted there key by key
/// (a terminal without bracketed paste) is dots until Enter reads it.
fn masked(text: &str) -> String {
    let mut out = text.to_string();
    for (key, ends) in [("INYON_TOKEN=", &[' ', '\t', '\'', '"'][..]), ("-Token '", &['\''][..])] {
        let mut from = 0;
        while let Some(i) = out[from..].find(key) {
            let at = from + i + key.len();
            let end = out[at..].find(ends).map_or(out.len(), |e| at + e);
            let dots = "•".repeat(out[at..end].chars().count());
            out.replace_range(at..end, &dots);
            from = at + dots.len();
        }
    }
    out
}

/// The columns `word` covers on a line drawn from column `x`. Every character in the hint line
/// is one cell wide (the arrows and dots included), so a column is a character count, the same
/// way `fit` measures the line.
fn columns(line: &str, word: &str, x: u16) -> Option<Range<u16>> {
    let start = line.find(word)?;
    let from = x + line[..start].chars().count() as u16;
    Some(from..from + word.chars().count() as u16)
}

/// Whether a click at (col, row) lands on a word placed by `columns`.
fn on(hit: &Option<(u16, Range<u16>)>, col: u16, row: u16) -> bool {
    matches!(hit, Some((y, cols)) if *y == row && cols.contains(&col))
}

impl Wizard {
    pub fn new(plan: Plan) -> Wizard {
        Wizard { plan, step: Step::Role, cursor: 0, hit: Vec::new(), next_hit: None, back_hit: None, problem: None }
    }

    fn steps(&self) -> Vec<Step> {
        let mut s = vec![Step::Role, Step::Network, Step::Background];
        if self.plan.role == RoleChoice::Hub {
            s.push(Step::SignIn);
        }
        s.push(Step::Summary);
        s
    }

    fn items(&self) -> Vec<Item> {
        let p = &self.plan;
        match self.step {
            Step::Role => {
                let win = p.platform == Platform::Windows;
                vec![
                    Item::Radio {
                        label: "Pair it with the phone".into(),
                        note: "the app connects to this machine directly".into(),
                        on: p.role == RoleChoice::Node,
                        disabled: false,
                    },
                    Item::Radio {
                        label: "Make it a hub".into(),
                        note: if win { "Linux and macOS only (on Windows, use WSL)".into() } else { "the server phones sign in to; it can add other machines".into() },
                        on: p.role == RoleChoice::Hub,
                        disabled: win,
                    },
                    Item::Radio {
                        label: "Add it to a hub".into(),
                        note: "with the command the hub shows under Add a machine".into(),
                        on: p.role == RoleChoice::Join,
                        disabled: false,
                    },
                ]
            }
            Step::Network => match p.role {
                RoleChoice::Node => {
                    let mut v = vec![
                        Item::Check { label: "Tunnel".into(), note: "a free Cloudflare tunnel, so any network works".into(), on: p.tunnel },
                        Item::Check { label: "LAN".into(), note: p.lan_ip().map(|ip| format!("the same Wi-Fi, at {ip}")).unwrap_or_else(|| "the same Wi-Fi".into()), on: p.lan },
                    ];
                    let vpns = p.vpn_networks();
                    let note = if vpns.is_empty() {
                        "Tailscale, NetBird, ZeroTier, WireGuard (none found now)".to_string()
                    } else {
                        vpns.iter().map(|n| format!("{} {}", n.kind.label(), n.ip)).collect::<Vec<_>>().join(", ")
                    };
                    v.push(Item::Check { label: "VPN".into(), note, on: p.vpn });
                    v.push(Item::Text { label: "Own address".into(), value: p.own_url.clone(), digits: false });
                    v.push(Item::Text { label: "Port".into(), value: p.port.clone(), digits: true });
                    v
                }
                RoleChoice::Hub => {
                    let mut v = vec![
                        Item::Radio {
                            label: "This machine only".into(),
                            note: "localhost; put HTTPS in front to reach it from a phone".into(),
                            on: p.bind == Bind::Localhost,
                            disabled: false,
                        },
                        Item::Radio {
                            label: "LAN".into(),
                            // anyone on the Wi-Fi could read plain http: setup makes a certificate
                            note: format!("{}, over HTTPS", p.lan_ip().map(|ip| format!("at {ip}")).unwrap_or_else(|| "every address".into())),
                            on: p.bind == Bind::Lan,
                            disabled: false,
                        },
                    ];
                    for n in p.vpn_networks() {
                        // plain http only on Tailscale and NetBird; ZeroTier and WireGuard get a certificate as the LAN does
                        let https = if n.ip.parse().is_ok_and(inyon_engine::hub::plain_http_ip) { "" } else { ", over HTTPS" };
                        v.push(Item::Radio {
                            label: n.kind.label().into(),
                            note: format!("{}{https}", n.name.clone().map(|name| format!("{} ({name})", n.ip)).unwrap_or_else(|| n.ip.clone())),
                            on: p.bind == Bind::Vpn(n.ip.clone()),
                            disabled: false,
                        });
                    }
                    v.push(Item::Text { label: "Own address".into(), value: if let Bind::Own(a) = &p.bind { a.clone() } else { String::new() }, digits: false });
                    v.push(Item::Text { label: "Port".into(), value: p.port.clone(), digits: true });
                    v
                }
                RoleChoice::Join => vec![
                    Item::Text { label: "Hub".into(), value: masked(&p.hub_url), digits: false },
                    Item::Text {
                        label: "Token".into(),
                        value: if p.hub_token.is_empty() { String::new() } else { "•".repeat(p.hub_token.chars().count().min(24)) },
                        digits: false,
                    },
                    Item::Info(self.problem.clone().unwrap_or_else(|| {
                        "Paste the command from the hub's Machines page (Add a machine) into Hub, or type the hub's address and the token from that command.".into()
                    })),
                ],
            },
            Step::Background => vec![
                Item::Radio { label: "Run in the background".into(), note: "starts at login and comes back after a crash".into(), on: p.autostart, disabled: false },
                Item::Radio { label: "Only while this window is open".into(), note: "run `inyon pair` whenever you need it".into(), on: !p.autostart, disabled: false },
            ],
            Step::SignIn => vec![
                Item::Info("The hub signs you in from a browser.".into()),
                Item::Info("Setup prints its address and a one-time code; open the address and enter the code to choose a password.".into()),
                Item::Info("`inyon hub code` shows the code again.".into()),
            ],
            Step::Summary => p.summary().into_iter().map(|(k, v)| Item::Info(format!("{k:<12}{v}"))).collect(),
        }
    }

    fn selectable(&self) -> Vec<usize> {
        self.items().iter().enumerate().filter(|(_, i)| !matches!(i, Item::Info(_))).map(|(n, _)| n).collect()
    }

    fn move_cursor(&mut self, delta: i32) {
        let sel = self.selectable();
        if sel.is_empty() {
            return;
        }
        let pos = sel.iter().position(|i| *i == self.cursor).unwrap_or(0) as i32;
        let next = (pos + delta).rem_euclid(sel.len() as i32) as usize;
        self.cursor = sel[next];
    }

    /// Space or Enter on the row under the cursor.
    fn choose(&mut self, index: usize) {
        let p = &mut self.plan;
        match (self.step, p.role, index) {
            (Step::Role, _, 0) => p.role = RoleChoice::Node,
            (Step::Role, _, 1) => {
                if p.platform != Platform::Windows {
                    p.role = RoleChoice::Hub
                }
            }
            (Step::Role, _, 2) => p.role = RoleChoice::Join,
            (Step::Network, RoleChoice::Node, 0) => p.tunnel = !p.tunnel,
            (Step::Network, RoleChoice::Node, 1) => p.lan = !p.lan,
            (Step::Network, RoleChoice::Node, 2) => p.vpn = !p.vpn,
            (Step::Network, RoleChoice::Hub, 0) => p.bind = Bind::Localhost,
            (Step::Network, RoleChoice::Hub, 1) => p.bind = Bind::Lan,
            (Step::Network, RoleChoice::Hub, n) => {
                let vpns: Vec<String> = p.vpn_networks().iter().map(|n| n.ip.clone()).collect();
                if let Some(ip) = vpns.get(n - 2) {
                    p.bind = Bind::Vpn(ip.clone());
                }
            }
            (Step::Background, _, 0) => p.autostart = true,
            (Step::Background, _, 1) => p.autostart = false,
            _ => {}
        }
        if self.step == Step::Role {
            p.port = if p.role == RoleChoice::Hub { "4777".into() } else { "4780".into() };
        }
    }

    /// The text field under the cursor, if the row is one.
    fn text_field(&mut self) -> Option<&mut String> {
        let p = &mut self.plan;
        match (self.step, p.role, self.cursor) {
            (Step::Network, RoleChoice::Node, 3) => Some(&mut p.own_url),
            (Step::Network, RoleChoice::Node, 4) => Some(&mut p.port),
            (Step::Network, RoleChoice::Hub, n) => {
                let count = 2 + p.vpn_networks().len();
                if n == count {
                    if !matches!(p.bind, Bind::Own(_)) {
                        p.bind = Bind::Own(String::new());
                    }
                    match &mut p.bind {
                        Bind::Own(a) => Some(a),
                        _ => None,
                    }
                } else if n == count + 1 {
                    Some(&mut p.port)
                } else {
                    None
                }
            }
            (Step::Network, RoleChoice::Join, 0) => Some(&mut p.hub_url),
            (Step::Network, RoleChoice::Join, 1) => Some(&mut p.hub_token),
            _ => None,
        }
    }

    fn is_digits_field(&self) -> bool {
        matches!(self.items().get(self.cursor), Some(Item::Text { digits: true, .. }))
    }

    pub fn advance(&mut self) -> Action {
        if self.step == Step::Summary {
            return Action::Done;
        }
        if self.step == Step::Network && self.plan.role == RoleChoice::Join {
            // what was pasted into Hub may be the whole command, with the token in it
            let input = self.plan.hub_url.clone();
            if let Err(e) = self.plan.set_join(&input) {
                self.problem = Some(e);
                self.cursor = 0;
                return Action::Stay;
            }
            if self.plan.hub_token.trim().is_empty() && crate::paths::env_var("INYON_TOKEN").is_none() {
                self.problem = Some("The token is missing: paste the hub's whole Add a machine command into Hub, or type its token here.".into());
                self.cursor = 1;
                return Action::Stay;
            }
            self.plan.hub_token = self.plan.hub_token.trim().to_string();
            self.problem = None;
        }
        let steps = self.steps();
        let i = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        self.step = steps[(i + 1).min(steps.len() - 1)];
        self.cursor = self.selectable().first().copied().unwrap_or(0);
        Action::Stay
    }

    pub fn back(&mut self) -> Action {
        let steps = self.steps();
        let i = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        if i == 0 {
            return Action::Stay;
        }
        self.step = steps[i - 1];
        self.cursor = self.selectable().first().copied().unwrap_or(0);
        Action::Stay
    }

    pub fn key(&mut self, key: crossterm::event::KeyEvent) -> Action {
        use crossterm::event::{KeyCode, KeyModifiers};
        let typing = self.text_field().is_some();
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Esc => Action::Quit,
            KeyCode::Char('q') if !typing => Action::Quit,
            KeyCode::Down | KeyCode::Tab => {
                self.move_cursor(1);
                Action::Stay
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.move_cursor(-1);
                Action::Stay
            }
            KeyCode::Char('j') if !typing => {
                self.move_cursor(1);
                Action::Stay
            }
            KeyCode::Char('k') if !typing => {
                self.move_cursor(-1);
                Action::Stay
            }
            KeyCode::Enter | KeyCode::Right => {
                // Enter picks the radio under the cursor; a checkbox keeps what Space made it
                if key.code == KeyCode::Enter && matches!(self.items().get(self.cursor), Some(Item::Radio { .. })) {
                    self.choose(self.cursor);
                }
                self.advance()
            }
            KeyCode::Left => self.back(),
            KeyCode::Char(' ') if !typing => {
                self.choose(self.cursor);
                Action::Stay
            }
            KeyCode::Backspace => {
                if let Some(f) = self.text_field() {
                    f.pop();
                }
                Action::Stay
            }
            KeyCode::Char(c) => {
                let digits = self.is_digits_field();
                let limit = self.field_limit();
                if let Some(f) = self.text_field() {
                    if (!digits || c.is_ascii_digit()) && f.chars().count() < limit {
                        f.push(c);
                    }
                }
                Action::Stay
            }
            _ => Action::Stay,
        }
    }

    /// How long the field under the cursor may get: the Join screen's Hub takes the hub's whole
    /// Add a machine command (some 600 characters), the others an address.
    fn field_limit(&self) -> usize {
        if self.step == Step::Network && self.plan.role == RoleChoice::Join {
            4000
        } else {
            120
        }
    }

    /// Pasted text (a terminal with bracketed paste): typed into the field under the cursor, its
    /// line breaks as spaces.
    pub fn paste(&mut self, text: &str) -> Action {
        let digits = self.is_digits_field();
        let limit = self.field_limit();
        if let Some(f) = self.text_field() {
            for c in text.chars().map(|c| if c == '\r' || c == '\n' { ' ' } else { c }) {
                if f.chars().count() >= limit {
                    break;
                }
                if !digits || c.is_ascii_digit() {
                    f.push(c);
                }
            }
        }
        // a whole command pasted into Hub is read at once, so its token is never on screen
        if self.step == Step::Network && self.plan.role == RoleChoice::Join && self.cursor == 0 {
            let input = self.plan.hub_url.clone();
            if crate::commands::parse_add_machine(&input).is_ok_and(|c| c.token.is_some()) {
                let _ = self.plan.set_join(&input);
            }
        }
        Action::Stay
    }

    /// A click: on a row selects it, on the "Enter next" or "← back" words moves on; anywhere
    /// else on the hint line does nothing.
    pub fn click(&mut self, col: u16, row: u16) -> Action {
        if let Some((_, i)) = self.hit.iter().find(|(y, _)| *y == row) {
            let i = *i;
            self.cursor = i;
            self.choose(i);
            return Action::Stay;
        }
        if on(&self.next_hit, col, row) {
            return self.advance();
        }
        if on(&self.back_hit, col, row) {
            return self.back();
        }
        Action::Stay
    }

    pub fn render(&mut self, f: &mut Frame, theme: Theme) {
        let area = f.area();
        let head = banner(area.width);
        let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(head.len() as u16 + 1), Constraint::Min(3), Constraint::Length(2)]).split(area);
        let mut top: Vec<Line> = head.iter().map(|l| Line::from(Span::styled(l.clone(), theme.strong()))).collect();
        let steps = self.steps();
        let crumbs: Vec<Span> = steps
            .iter()
            .enumerate()
            .flat_map(|(n, s)| {
                let name = match s {
                    Step::Role => "Role",
                    Step::Network => "Network",
                    Step::Background => "Background",
                    Step::SignIn => "Sign in",
                    Step::Summary => "Summary",
                };
                let style = if *s == self.step { theme.strong() } else { theme.faint() };
                let mut v = vec![Span::styled(format!("{} {name}", n + 1), style)];
                if n + 1 < steps.len() {
                    v.push(Span::styled("  ·  ", theme.line()));
                }
                v
            })
            .collect();
        if area.width >= 56 {
            top.push(Line::from(crumbs));
        } else {
            top.push(Line::from(Span::styled(format!("step {} of {}", steps.iter().position(|s| *s == self.step).unwrap_or(0) + 1, steps.len()), theme.faint())));
        }
        f.render_widget(Paragraph::new(top), rows[0]);

        let body = rows[1];
        let mut lines: Vec<Line> = vec![Line::from(Span::styled(self.step.title(), theme.body())), Line::raw("")];
        self.hit.clear();
        let width = body.width as usize;
        for (i, item) in self.items().iter().enumerate() {
            let y = body.y + lines.len() as u16;
            let selected = i == self.cursor;
            let mark = |on: bool, radio: bool| match (on, radio) {
                (true, true) => "●",
                (false, true) => "○",
                (true, false) => "■",
                (false, false) => "□",
            };
            match item {
                Item::Radio { label, note, on, disabled } => {
                    self.hit.push((y, i));
                    let style = if *disabled {
                        theme.faint()
                    } else if selected {
                        theme.selected()
                    } else {
                        theme.body()
                    };
                    lines.push(Line::from(vec![Span::styled(format!(" {} {} ", mark(*on, true), fit(label, width.saturating_sub(5))), style)]));
                    lines.push(Line::from(Span::styled(format!("     {}", fit(note, width.saturating_sub(5))), theme.muted())));
                }
                Item::Check { label, note, on } => {
                    self.hit.push((y, i));
                    let style = if selected { theme.selected() } else { theme.body() };
                    lines.push(Line::from(vec![Span::styled(format!(" {} {} ", mark(*on, false), fit(label, width.saturating_sub(5))), style)]));
                    lines.push(Line::from(Span::styled(format!("     {}", fit(note, width.saturating_sub(5))), theme.muted())));
                }
                Item::Text { label, value, .. } => {
                    self.hit.push((y, i));
                    let shown = if value.is_empty() { "…".to_string() } else { value.clone() };
                    let style = if selected { theme.selected() } else { theme.body() };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {:<12}", label), theme.muted()),
                        Span::styled(format!(" {} ", fit(&shown, width.saturating_sub(16))), style),
                    ]));
                }
                Item::Info(text) => {
                    for l in super::wrap(text, width.saturating_sub(2)) {
                        lines.push(Line::from(Span::styled(format!(" {l}"), theme.body())));
                    }
                }
            }
        }
        f.render_widget(Paragraph::new(lines), body);

        let foot = rows[2];
        let (keys, next) = match self.step {
            Step::Summary => ("Enter set up  ·  ← back  ·  Esc quit", "Enter set up"),
            Step::SignIn => ("Enter next  ·  ← back  ·  Esc quit", "Enter next"),
            _ => ("↑↓ move  ·  Space choose  ·  Enter next  ·  ← back  ·  Esc quit", "Enter next"),
        };
        self.next_hit = columns(keys, next, foot.x).map(|c| (foot.y + 1, c));
        self.back_hit = columns(keys, "← back", foot.x).map(|c| (foot.y + 1, c));
        let lines = vec![Line::default(), Line::from(Span::styled(fit(keys, foot.width as usize), theme.faint()))];
        f.render_widget(Paragraph::new(lines), Rect { x: foot.x, y: foot.y, width: foot.width, height: foot.height });
    }
}

/// Runs the screens in the terminal; Some(plan) when setup was confirmed.
pub fn run_interactive(plan: Plan, theme: Theme) -> std::io::Result<Option<Plan>> {
    use crossterm::event::{self, Event, MouseButton, MouseEventKind};
    let mut w = Wizard::new(plan);
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), event::EnableMouseCapture);
    // a pasted Add a machine command arrives whole, not as keys (where the terminal can)
    let _ = crossterm::execute!(std::io::stdout(), event::EnableBracketedPaste);
    let result = (|| -> std::io::Result<Option<Plan>> {
        loop {
            terminal.draw(|f| w.render(f, theme))?;
            if !event::poll(std::time::Duration::from_millis(250))? {
                continue;
            }
            let action = match event::read()? {
                Event::Key(k) if k.kind != event::KeyEventKind::Release => w.key(k),
                Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => w.click(m.column, m.row),
                Event::Paste(text) => w.paste(&text),
                _ => Action::Stay,
            };
            match action {
                Action::Stay => {}
                Action::Quit => return Ok(None),
                Action::Done => return Ok(Some(w.plan.clone())),
            }
        }
    })();
    let _ = crossterm::execute!(std::io::stdout(), event::DisableMouseCapture);
    let _ = crossterm::execute!(std::io::stdout(), event::DisableBracketedPaste);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(w: &mut Wizard) {
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| w.render(f, Theme::detect(false))).unwrap();
    }

    #[test]
    fn columns_count_cells_from_the_left_edge() {
        let line = "Enter set up  ·  ← back  ·  Esc quit";
        assert_eq!(columns(line, "Enter set up", 0), Some(0..12));
        assert_eq!(columns(line, "← back", 2), Some(19..25));
        assert_eq!(columns(line, "Enter next", 0), None);
    }

    #[test]
    fn only_the_back_word_goes_back_and_only_the_next_word_goes_on() {
        let mut w = Wizard::new(Plan::new(Platform::Linux, Vec::new()));
        draw(&mut w);
        let (row, next) = w.next_hit.clone().unwrap();
        let (_, back) = w.back_hit.clone().unwrap();
        assert!(next.end <= back.start);
        // back on the first screen stays put; between the words nothing happens
        assert_eq!(w.click(back.start, row), Action::Stay);
        assert_eq!(w.step, Step::Role);
        assert_eq!(w.click(next.end, row), Action::Stay);
        assert_eq!(w.step, Step::Role);
        assert_eq!(w.click(next.start, row), Action::Stay);
        assert_eq!(w.step, Step::Network);
        draw(&mut w);
        let (row, _) = w.next_hit.clone().unwrap();
        let (_, back) = w.back_hit.clone().unwrap();
        assert_eq!(w.click(back.end - 1, row), Action::Stay);
        assert_eq!(w.step, Step::Role);
        // on the summary, back must never start the install
        while w.step != Step::Summary {
            w.advance();
        }
        draw(&mut w);
        let (row, next) = w.next_hit.clone().unwrap();
        let (_, back) = w.back_hit.clone().unwrap();
        assert_eq!(w.click(back.start + 2, row), Action::Stay);
        assert_ne!(w.step, Step::Summary);
        while w.step != Step::Summary {
            w.advance();
        }
        draw(&mut w);
        assert_eq!(w.click(next.start, row), Action::Done);
    }

    const UNIX_COMMAND: &str = "INYON_TOKEN=tok_7Hq2 INYON_SUMS_SHA256=ab12 INYON_HUB_CERT=cd34 sh -c 'f=$(mktemp) && curl -fsSLk \"$1/node/install.sh\" -o \"$f\" && sh \"$f\" \"$1\"; exit $s' inyon https://hub.lan:4777 9f9f";
    const WINDOWS_COMMAND: &str = "powershell -NoProfile -ExecutionPolicy Bypass -Command \"[Net.ServicePointManager]::SecurityProtocol=3072; & ([scriptblock]::Create((irm 'https://hub.example/node/install.ps1'))) -Hub 'https://hub.example' -Token 'tok_W1n'\"";

    fn join_screen() -> Wizard {
        let mut w = Wizard::new(Plan::new(Platform::Windows, Vec::new()));
        w.choose(2);
        w.advance();
        assert_eq!((w.step, w.plan.role), (Step::Network, RoleChoice::Join));
        w
    }

    #[test]
    fn join_takes_the_hubs_add_a_machine_command_pasted_whole() {
        for (cmd, hub, token, cert) in [(UNIX_COMMAND, "https://hub.lan:4777", "tok_7Hq2", "cd34"), (WINDOWS_COMMAND, "https://hub.example", "tok_W1n", "")] {
            let mut w = join_screen();
            w.paste(cmd);
            assert_eq!(w.advance(), Action::Stay);
            assert_eq!(w.step, Step::Background, "{:?}", w.problem);
            assert_eq!((w.plan.hub_url.as_str(), w.plan.hub_token.as_str(), w.plan.hub_cert.as_str()), (hub, token, cert));
            // the summary names the hub, never the token
            assert!(w.plan.summary().iter().all(|(_, v)| !v.contains(token)));
        }
    }

    #[test]
    fn join_never_shows_the_token_in_a_command_on_screen() {
        let mut w = join_screen();
        // typed key by key, as a terminal without bracketed paste delivers it
        for c in UNIX_COMMAND.chars() {
            w.key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c)));
        }
        let shown = |w: &Wizard| w.items().iter().map(|i| format!("{i:?}")).collect::<String>();
        assert!(!shown(&w).contains("tok_7Hq2"), "{}", shown(&w));
        assert_eq!(masked(WINDOWS_COMMAND).matches("tok_W1n").count(), 0);
        // pasted, it's read at once: Hub shows the address
        let mut w = join_screen();
        w.paste(UNIX_COMMAND);
        assert_eq!(w.plan.hub_url, "https://hub.lan:4777");
        assert!(!shown(&w).contains("tok_7Hq2"));
    }

    #[test]
    fn a_commands_token_never_goes_to_another_hub_typed_in_after_it() {
        let mut w = join_screen();
        w.paste(UNIX_COMMAND);
        // back to Hub, the address changed to another hub's
        w.plan.hub_url = "https://other.example".into();
        w.plan.set_join("https://other.example").unwrap();
        assert_eq!((w.plan.hub_token.as_str(), w.plan.hub_cert.as_str()), ("", ""));
        // the same hub's address keeps them
        let mut w = join_screen();
        w.paste(UNIX_COMMAND);
        w.plan.set_join("https://hub.lan:4777/").unwrap();
        assert_eq!((w.plan.hub_token.as_str(), w.plan.hub_cert.as_str()), ("tok_7Hq2", "cd34"));
        // a token typed in apart stays with whatever hub is typed
        let mut p = Plan::new(Platform::Linux, Vec::new());
        p.hub_token = "tok_mine".into();
        p.set_join("https://a.example").unwrap();
        p.set_join("https://b.example").unwrap();
        assert_eq!(p.hub_token, "tok_mine");
    }

    #[test]
    fn join_takes_an_address_and_a_token_typed_apart() {
        let mut w = join_screen();
        for c in "https://hub.example/".chars() {
            w.key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c)));
        }
        if crate::paths::env_var("INYON_TOKEN").is_none() {
            assert_eq!(w.advance(), Action::Stay);
            assert!(w.problem.as_deref().is_some_and(|p| p.contains("token is missing")), "{:?}", w.problem);
            assert_eq!(w.cursor, 1);
            w.paste("tok_typed");
        }
        w.advance();
        assert_eq!(w.step, Step::Background);
        assert_eq!(w.plan.hub_url, "https://hub.example");
    }

    #[test]
    fn join_turns_a_phone_link_away_with_where_the_command_is() {
        let mut w = join_screen();
        w.paste("inyon://hub?u=https%3A%2F%2Fhub.example&c=K7Q4MX2T%3A493812&n=studio");
        assert_eq!(w.advance(), Action::Stay);
        assert_eq!(w.step, Step::Network);
        let p = w.problem.clone().unwrap();
        assert!(p.contains("pairs a phone") && p.contains("Machines page (Add a machine)"), "{p}");
        // and with no address at all, it says what to paste
        let mut w = join_screen();
        w.paste("hello");
        w.advance();
        assert!(w.problem.as_deref().is_some_and(|p| p.contains("Add a machine")));
    }

    #[test]
    fn the_flags_take_the_whole_command_too() {
        let f = crate::commands::SetupFlags { yes: true, role: None, network: None, port: None, autostart: None, hub: Some(WINDOWS_COMMAND.into()), no_code: false };
        let p = crate::commands::plan_from_flags(Platform::Windows, Vec::new(), &f).unwrap();
        assert_eq!((p.role, p.hub_url.as_str(), p.hub_token.as_str()), (RoleChoice::Join, "https://hub.example", "tok_W1n"));
        let f = crate::commands::SetupFlags { hub: Some("inyon://hub?u=x".into()), ..f };
        assert!(crate::commands::plan_from_flags(Platform::Windows, Vec::new(), &f).unwrap_err().contains("pairs a phone"));
    }
}
