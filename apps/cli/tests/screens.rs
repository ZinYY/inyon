//! The dashboard and the setup screens, drawn into a test backend and compared with the text in
//! tests/snapshots. UPDATE_SNAPSHOTS=1 rewrites them.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use inyon_cli::networks::{Kind, Network};
use inyon_cli::sys::Platform;
use inyon_cli::ui::dashboard::{buffer_text, render, Job, PairedPhone, Status};
use inyon_cli::ui::setup::{Action, Plan, RoleChoice, Step, Wizard};
use inyon_cli::ui::{Theme, VERSION};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn check(name: &str, text: &str) {
    // the header names the running version; the snapshots carry a placeholder so a release bump
    // doesn't change every one of them
    let text = &text.replace(&format!("inyon {VERSION}"), "inyon x.y.z");
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots").join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !file.exists() {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, format!("{text}\n")).unwrap();
        return;
    }
    // a checkout on Windows may have given the file CRLF line endings
    let want = fs::read_to_string(&file).unwrap().replace("\r\n", "\n");
    assert_eq!(text.trim_end(), want.trim_end(), "{name} changed; run with UPDATE_SNAPSHOTS=1 if that's meant");
}

fn draw(width: u16, height: u16, f: impl FnOnce(&mut ratatui::Frame)) -> String {
    let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
    t.draw(f).unwrap();
    buffer_text(t.backend().buffer())
}

fn status() -> Status {
    Status {
        v: 1,
        cli: "0.13.0".into(),
        engine: Some("0.13.0".into()),
        role: "node".into(),
        hub: None,
        machine: Some("k7m2p9x4w1q8z3v6".into()),
        jobs: vec![
            Job { role: "link".into(), service: true, running: true, version: Some("0.13.0".into()) },
            Job { role: "node".into(), service: false, running: false, version: None },
            Job { role: "hub".into(), service: false, running: false, version: None },
        ],
        addresses: vec!["wss://quiet-otter.trycloudflare.com/link".into(), "ws://100.64.0.9:4780/link".into()],
        relays: vec![],
        phones: vec![
            PairedPhone { key: "a1b2c3d4e5f6".into(), name: "Pixel 9".into(), paired: "2026-10-01".into(), last_seen: Some("2026-10-13 08:12".into()) },
            PairedPhone { key: "0f9e8d7c6b5a".into(), name: "Work phone".into(), paired: "2026-09-20".into(), last_seen: None },
        ],
        nodes: None,
        agents: vec!["claude in work:1.0".into(), "codex in ops:0.0".into()],
        pairing_open: false,
        log: vec!["link.log: inyon 0.13.0 link service for k7m2p9x4 on port 4780".into(), "link.log: tunnel up at quiet-otter.trycloudflare.com".into()],
        note: None,
        reach: None,
        background: None,
    }
}

#[test]
fn dashboard_for_a_paired_machine() {
    check("dashboard", &draw(80, 24, |f| render(f, &status(), Theme::plain())));
}

#[test]
fn dashboard_on_a_narrow_terminal_stacks_its_columns() {
    check("dashboard-narrow", &draw(40, 26, |f| render(f, &status(), Theme::plain())));
}

#[test]
fn dashboard_for_a_hub_lists_its_machines() {
    let mut s = status();
    s.role = "hub".into();
    s.jobs = vec![Job { role: "hub".into(), service: true, running: true, version: Some("0.13.0".into()) }];
    s.addresses.clear();
    s.phones.clear();
    s.agents.clear();
    s.nodes = Some(vec!["laptop".into(), "build-box".into()]);
    s.log.clear();
    check("dashboard-hub", &draw(80, 20, |f| render(f, &s, Theme::plain())));
}

#[test]
fn dashboard_before_setup_says_what_to_do() {
    let s = Status {
        role: "none".into(),
        engine: None,
        machine: None,
        jobs: vec![],
        addresses: vec![],
        phones: vec![],
        agents: vec![],
        log: vec![],
        note: Some("The engine isn't installed yet: run `inyon setup`.".into()),
        ..status()
    };
    check("dashboard-empty", &draw(80, 16, |f| render(f, &s, Theme::plain())));
}

fn networks() -> Vec<Network> {
    vec![
        Network { kind: Kind::Lan, ip: "192.168.1.20".into(), name: None },
        Network { kind: Kind::Tailscale, ip: "100.64.0.9".into(), name: Some("laptop.tail1234.ts.net".into()) },
    ]
}

fn wizard() -> Wizard {
    Wizard::new(Plan::new(Platform::Linux, networks()))
}

fn key(w: &mut Wizard, code: KeyCode) -> Action {
    w.key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn setup_role_screen() {
    let mut w = wizard();
    check("setup-role", &draw(80, 22, |f| w.render(f, Theme::plain())));
}

#[test]
fn setup_network_screen_for_a_node() {
    let mut w = wizard();
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::Network);
    check("setup-network-node", &draw(80, 22, |f| w.render(f, Theme::plain())));
}

#[test]
fn setup_network_screen_for_a_hub() {
    let mut w = wizard();
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.plan.role, RoleChoice::Hub);
    assert_eq!(w.step, Step::Network);
    check("setup-network-hub", &draw(80, 22, |f| w.render(f, Theme::plain())));
}

#[test]
fn setup_background_and_summary_screens() {
    let mut w = wizard();
    key(&mut w, KeyCode::Enter);
    // LAN on, VPN off, port 4790
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Char(' '));
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Char(' '));
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Backspace);
    key(&mut w, KeyCode::Char('x'));
    key(&mut w, KeyCode::Char('9'));
    assert_eq!(w.plan.port, "4789");
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::Background);
    check("setup-background", &draw(80, 22, |f| w.render(f, Theme::plain())));
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::Summary);
    assert!(!w.plan.autostart);
    check("setup-summary", &draw(80, 22, |f| w.render(f, Theme::plain())));
    assert_eq!(key(&mut w, KeyCode::Enter), Action::Done);
    assert!(w.plan.lan && !w.plan.vpn && w.plan.tunnel);
}

#[test]
fn setup_sign_in_screen_for_a_hub_and_the_narrow_layout() {
    let mut w = wizard();
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Enter);
    key(&mut w, KeyCode::Enter);
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::SignIn);
    check("setup-sign-in", &draw(80, 22, |f| w.render(f, Theme::plain())));
    check("setup-sign-in-narrow", &draw(38, 18, |f| w.render(f, Theme::plain())));
}

#[test]
fn keys_and_clicks_change_the_plan() {
    let mut w = wizard();
    // a click on the third option (row 9 in the role snapshot) picks "join"
    draw(80, 22, |f| w.render(f, Theme::plain()));
    assert_eq!(w.click(10, 9), Action::Stay);
    assert_eq!(w.plan.role, RoleChoice::Join);
    assert_eq!(key(&mut w, KeyCode::Enter), Action::Stay);
    assert_eq!(w.step, Step::Network);
    // the hub address has to be a web address before the next screen
    assert_eq!(key(&mut w, KeyCode::Enter), Action::Stay);
    assert_eq!(w.step, Step::Network);
    for c in "https://hub.example".chars() {
        key(&mut w, KeyCode::Char(c));
    }
    assert_eq!(w.plan.hub_url, "https://hub.example");
    // and it needs the token, typed under it or in the command pasted whole
    if inyon_cli::paths::env_var("INYON_TOKEN").is_none() {
        assert_eq!(key(&mut w, KeyCode::Enter), Action::Stay);
        assert_eq!(w.step, Step::Network);
        for c in "tok".chars() {
            key(&mut w, KeyCode::Char(c));
        }
        assert_eq!(w.plan.hub_token, "tok");
    }
    assert_eq!(key(&mut w, KeyCode::Enter), Action::Stay);
    assert_eq!(w.step, Step::Background);
    assert_eq!(key(&mut w, KeyCode::Left), Action::Stay);
    assert_eq!(w.step, Step::Network);
    assert_eq!(key(&mut w, KeyCode::Esc), Action::Quit);
    // on Windows the hub can't be chosen
    let mut win = Wizard::new(Plan::new(Platform::Windows, Vec::new()));
    key(&mut win, KeyCode::Down);
    key(&mut win, KeyCode::Char(' '));
    assert_eq!(win.plan.role, RoleChoice::Node);
}

#[test]
fn enter_on_a_checkbox_keeps_the_choice() {
    // Windows starts with VPN off: Space on VPN, then Enter, must keep it on
    let mut w = Wizard::new(Plan::new(Platform::Windows, networks()));
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::Network);
    assert!(!w.plan.vpn);
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Char(' '));
    assert!(w.plan.vpn);
    key(&mut w, KeyCode::Enter);
    assert_eq!(w.step, Step::Background);
    assert!(w.plan.vpn && w.plan.tunnel && !w.plan.lan);

    // Enter alone on a checkbox row moves on without changing it
    let mut w = wizard();
    key(&mut w, KeyCode::Enter);
    let before = w.plan.clone();
    assert_eq!(key(&mut w, KeyCode::Enter), Action::Stay);
    assert_eq!(w.step, Step::Background);
    assert_eq!(w.plan, before);
    // Enter still picks the radio under the cursor
    key(&mut w, KeyCode::Down);
    key(&mut w, KeyCode::Enter);
    assert!(!w.plan.autostart);
}

#[test]
fn the_summary_names_every_vpn() {
    let mut nets = networks();
    nets.push(Network { kind: Kind::Netbird, ip: "100.97.115.176".into(), name: None });
    let plan = Plan::new(Platform::Linux, nets);
    let answers = plan.summary().into_iter().find(|(k, _)| k == "Answers on").unwrap().1;
    assert_eq!(answers, "a tunnel (any network), Tailscale (100.64.0.9), NetBird (100.97.115.176), port 4780");
}
