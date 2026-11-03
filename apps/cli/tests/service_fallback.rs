//! Linux with no user systemd: install falls back to a nohup process plus cron. Nothing real
//! runs here. Every program, the crontab and the process table are fakes, and home is a
//! throwaway folder.

mod common;

use common::{bin, lock, sys, FakeProc, Home};
use inyon_cli::service::{install_service, move_old_service, old_services, uninstall_service, Installed, Role};
use inyon_cli::sys::Platform;

const CRONTAB: &str = "MAILTO=me\n0 * * * * backup\n";

fn setup() -> (Home, std::path::PathBuf) {
    let home = Home::new(&["crontab", "sh"]);
    home.set_crontab(CRONTAB);
    // the program being installed, somewhere outside ~/.inyon/bin
    let program = home.dir.join("download").join("inyon");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    (home, program)
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn uninstall_stops_the_fallback_process_it_started_and_removes_the_cron_entry() {
    let _g = lock();
    let (home, program) = setup();
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    assert!(install_service(&sys, Role::Link, false).ok);
    assert_eq!(home.read(".inyon/node/link.pid").trim(), "4242");
    assert!(proc.table().contains_key(&4242));
    assert!(home.crontab_text().contains("# inyon link"));
    // after a restart cron starts it again, and that copy records its PID too
    let line = home.crontab_text().lines().find(|l| l.contains("# inyon link")).unwrap().to_string();
    assert!(line.starts_with("@reboot nohup "), "{line}");
    assert!(line.contains(&format!("& echo $! > '{}' # inyon link", home.dir.join(".inyon/node/link.pid").display())), "{line}");
    assert!(line.contains(&format!("'{}' 'link'", bin(&home).display())), "{line}");

    let r = uninstall_service(&sys, Role::Link);
    assert_eq!(r, Installed { ok: true, how: "the background service is removed".into() });
    assert_eq!(*proc.signals.borrow(), vec![(4242, "SIGTERM".to_string())]);
    assert!(proc.table().is_empty());
    assert!(!home.exists(".inyon/node/link.pid"));
    assert_eq!(home.crontab_text(), CRONTAB);
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn a_recorded_pid_that_now_belongs_to_another_program_is_left_alone() {
    let _g = lock();
    let (home, program) = setup();
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    install_service(&sys, Role::Link, false);
    proc.set(4242, &["/usr/bin/vim", "notes.txt"]);
    assert!(uninstall_service(&sys, Role::Link).ok);
    assert!(proc.signals.borrow().is_empty());
    assert_eq!(proc.table()[&4242], vec!["/usr/bin/vim", "notes.txt"]);
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn a_fallback_process_that_will_not_stop_makes_uninstall_say_so() {
    let _g = lock();
    let (home, program) = setup();
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    install_service(&sys, Role::Link, false);
    proc.ignore_signals.set(true);
    let r = uninstall_service(&sys, Role::Link);
    assert!(!r.ok);
    assert!(r.how.contains("PID 4242"), "{}", r.how);
    assert_eq!(proc.signal_names(), vec!["SIGTERM", "SIGKILL"]);
    // still recorded, so a later uninstall can try again
    assert!(home.exists(".inyon/node/link.pid"));
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn installing_again_replaces_the_running_fallback_instead_of_adding_one() {
    let _g = lock();
    let (home, program) = setup();
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    install_service(&sys, Role::Link, false);
    install_service(&sys, Role::Link, false);
    assert_eq!(*proc.signals.borrow(), vec![(4242, "SIGTERM".to_string())]);
    assert_eq!(proc.table().keys().copied().collect::<Vec<_>>(), vec![4243]);
    assert_eq!(home.crontab_text().matches("# inyon link").count(), 1);
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn tether_nodes_fallback_link_moves_to_inyons_the_old_copy_stopped_its_cron_line_replaced() {
    let _g = lock();
    let (home, program) = setup();
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    // what tether-node left: its PID in ~/.tether-node/link.pid and its own crontab line
    home.put(".tether-node/link.pid", "777\n");
    proc.set(777, &["/home/me/.tether-node/bin/tether-node", "link"]);
    home.set_crontab(&format!("{CRONTAB}@reboot nohup '/home/me/.tether-node/bin/tether-node' 'link' >> x 2>&1 & echo $! > y # tether-node link\n"));
    assert_eq!(old_services(&sys), vec![Role::Link]);

    assert!(move_old_service(&sys, Role::Link).ok);
    assert_eq!(*proc.signals.borrow(), vec![(777, "SIGTERM".to_string())]);
    assert!(!home.crontab_text().contains("# tether-node link"));
    assert_eq!(home.crontab_text().matches("# inyon link").count(), 1);
    assert!(home.crontab_text().starts_with(CRONTAB));
    // the folder moved too, with a link left at the old path
    assert_eq!(home.read(".tether-node/link.pid").trim(), "4242");
    assert_eq!(old_services(&sys), Vec::<Role>::new());
}

#[test]
fn a_program_that_is_not_installed_is_a_failed_step_not_a_crash() {
    // a container or minimal server has no systemctl; setup must fall back instead of dying
    let r = inyon_cli::sys::run(&["inyon-no-such-program-here", "--user", "show-environment"]);
    assert!(!r.ok);
    assert!(r.out.contains("isn't available here"));
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn a_node_the_hub_removed_can_take_down_its_own_unit_because_the_stop_comes_last() {
    let _g = lock();
    let (home, program) = setup();
    // a stop sent from inside the unit ends the process that sent it: whatever is still on disk
    // at that moment stays for ever
    std::fs::write(
        home.fakes.join("systemctl"),
        "#!/bin/sh\nPATH=/usr/bin:/bin\nprintf '%s\\n' \"$*\" >> \"$HOME/systemctl.log\"\nif [ \"$2\" = stop ]; then for f in .config/systemd/user/inyon-node.service .inyon/node/env; do [ -e \"$HOME/$f\" ] && echo \"$f\" >> \"$HOME/left-at-stop\"; done; fi\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.fakes.join("systemctl"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    home.unit("inyon-node", "/home/me/.inyon/bin/inyon connect");
    home.put(".inyon/node/env", "INYON_HUB=http://hub.lan:4777\nINYON_TOKEN=t\n");
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, &program);
    assert!(inyon_cli::commands::disconnect(&sys).is_ok());
    let log = home.read("systemctl.log");
    assert_eq!(log.lines().last(), Some("--user stop inyon-node.service"), "{log}");
    assert!(!log.contains("--now"), "{log}");
    assert_eq!(home.read("left-at-stop"), "");
    assert!(!home.exists(".config/systemd/user/inyon-node.service"));
    assert!(!home.exists(".inyon/node/env"));
}
