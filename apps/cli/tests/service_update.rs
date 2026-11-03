//! How `inyon update` restarts each job, and how tether-node's services move to inyon's.
//! Nothing runs: every command goes to a fake that records it, the process table is a fake, and
//! home is a throwaway folder. The fakes are shell scripts, so the tests that run one are skipped
//! on Windows; the PowerShell each step would run is checked as text, on the other systems.

mod common;

use common::{lock, sys, FakeProc, Home};
use inyon_cli::service::{detach_update, move_old_service, old_services, restart_service, Installed, Role};
use inyon_cli::sys::Platform;

const BIN: &str = "/home/me/.inyon/bin/inyon";

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn setup(fakes: &[&str]) -> Home {
    Home::new(fakes)
}

#[test]
#[cfg_attr(windows, ignore)]
fn linux_systemctl_restarts_the_jobs_own_unit_or_tether_nodes_while_it_has_not_moved() {
    let _g = lock();
    let home = setup(&["systemctl", "launchctl", "loginctl", "sh", "powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, home.dir.join("x").as_path());
    home.unit("inyon-link", &format!("\"{BIN}\" \"link\""));
    assert!(restart_service(&sys, Role::Link, &v(&[BIN])).ok);
    assert_eq!(home.calls(), vec![v(&["systemctl", "--user", "restart", "inyon-link.service"])]);

    // both of tether-node's installers called their unit tether-node: what it runs says which job
    home.clear_calls();
    home.unit("tether-node", "\"/home/me/.tether-node/bin/tether-node\" \"link\"");
    assert!(!restart_service(&sys, Role::Node, &v(&[BIN])).ok);
    assert!(home.calls().is_empty());
    home.unit("tether-node", "/home/me/.bun/bin/bun /home/me/.tether-node/tether-node.ts");
    assert!(restart_service(&sys, Role::Node, &v(&[BIN])).ok);
    assert_eq!(home.calls(), vec![v(&["systemctl", "--user", "restart", "tether-node.service"])]);
}

#[test]
#[cfg_attr(windows, ignore)]
fn macos_launchd_restarts_the_job_under_its_own_label_or_the_old_one() {
    let _g = lock();
    let home = setup(&["systemctl", "launchctl", "sh"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Darwin, home.dir.join("x").as_path());
    let uid = inyon_cli::sys::uid();
    home.put("Library/LaunchAgents/dev.tether.node.plist", "");
    assert!(restart_service(&sys, Role::Node, &v(&[BIN])).ok);
    assert_eq!(home.calls(), vec![v(&["launchctl", "kickstart", "-k", &format!("gui/{uid}/dev.tether.node")])]);
    home.clear_calls();
    home.put("Library/LaunchAgents/tech.abhashchakraborty.inyon.node.plist", "");
    restart_service(&sys, Role::Node, &v(&[BIN]));
    assert_eq!(home.calls(), vec![v(&["launchctl", "kickstart", "-k", &format!("gui/{uid}/tech.abhashchakraborty.inyon.node")])]);
    // the phone link isn't set up here
    home.clear_calls();
    assert!(!restart_service(&sys, Role::Link, &v(&[BIN])).ok);
    assert!(home.calls().is_empty());
}

#[test]
#[cfg_attr(windows, ignore)]
fn windows_the_logon_task_is_stopped_and_started_and_the_old_copy_stopped_never_the_updater_itself() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("x").as_path());
    // no task or shortcut of inyon's own: only tether-node's may be there
    std::env::set_var("FAKE_PS_NO", "exit 0 } else { exit 1 }");
    let exe = "C:\\Users\\me\\.tether-node\\tether-node.exe";
    assert!(restart_service(&sys, Role::Node, &v(&[exe])).ok);
    let calls = home.calls();
    assert!(calls[0].last().unwrap().contains("Get-ScheduledTask -TaskName 'Inyon node'"), "{calls:?}");
    let ps = calls[1].last().unwrap().clone();
    assert_eq!(calls[0][0], "powershell.exe");
    assert!(ps.contains("$exe = 'C:\\Users\\me\\.tether-node\\tether-node.exe'"), "{ps}");
    assert!(ps.contains("'Inyon node', 'Tether node' | ForEach-Object"), "{ps}");
    assert!(ps.contains("Get-Process -Name 'inyon', 'inyon-engine', 'tether-node'"), "{ps}");
    assert!(ps.contains(&format!("$_.Id -ne {}", std::process::id())), "{ps}");
    assert!(ps.find("Stop-ScheduledTask").unwrap() < ps.find("Start-ScheduledTask").unwrap());
    // install.ps1's Startup-folder fallback has no task: start it the way that does
    assert!(ps.contains("Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe) -WindowStyle Hidden"), "{ps}");
    home.clear_calls();
    restart_service(&sys, Role::Link, &v(&["C:\\Users\\o'brien\\.inyon\\bin\\inyon.exe"]));
    let ps = home.calls()[1].last().unwrap().clone();
    assert!(ps.contains("'Inyon link', 'Tether link'"), "{ps}");
    assert!(ps.contains("$exe = 'C:\\Users\\o''brien\\.inyon\\bin\\inyon.exe'"), "{ps}");
    assert!(ps.contains("-ArgumentList 'link'"), "{ps}");
    // the link's tunnel is stopped with it
    assert!(ps.contains("Name='cloudflared.exe'"), "{ps}");
    std::env::remove_var("FAKE_PS_NO");
}

/// A Windows program's first bytes: the DOS header pointing at a PE header with its subsystem.
fn fake_exe(subsystem: u16) -> Vec<u8> {
    let mut exe = vec![0u8; 512];
    exe[..2].copy_from_slice(b"MZ");
    exe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    exe[0x80..0x84].copy_from_slice(b"PE\0\0");
    let at = 0x80 + 4 + 20 + 68;
    exe[at..at + 2].copy_from_slice(&subsystem.to_le_bytes());
    exe
}

// the sign-in task ran inyon.exe in a console window that stayed open while the link ran: from
// 0.20.3 it opens the Inyon app, a copy of inyon marked to run without a console
#[test]
#[cfg_attr(windows, ignore)]
fn windows_the_sign_in_task_opens_the_inyon_app_with_no_console() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("x").as_path());
    let bin = home.dir.join(".inyon").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut program = fake_exe(3);
    program.extend_from_slice(inyon_cli::engine::COPY_MARK.as_bytes());
    std::fs::write(bin.join("inyon"), &program).unwrap();
    let cli = bin.join("inyon").to_string_lossy().into_owned();
    let app = bin.join("inyon-app").to_string_lossy().into_owned();
    // nothing here plays the app, so it never reports the link running: that's said, not hidden
    let r = restart_service(&sys, Role::Link, &v(&[&cli]));
    assert!(!r.ok && r.how.contains("the Inyon app didn't start"), "{}", r.how);
    assert_eq!(std::fs::read(&app).unwrap(), inyon_cli::engine::make_copy(&program).unwrap(), "the app is inyon without a console");
    let calls = home.calls();
    let register = calls.iter().map(|c| c.last().unwrap().clone()).find(|ps| ps.contains("Register-ScheduledTask")).expect("the task is registered");
    assert!(register.contains(&format!("New-ScheduledTaskAction -Execute '{app}' -Argument 'app --background link'")), "{register}");
    assert!(register.contains(&format!("$l.TargetPath = '{app}'")), "the Startup-folder fallback opens the app too: {register}");
    assert!(!register.contains("Start-ScheduledTask"), "registering starts nothing; the app does: {register}");
    let start = calls.last().unwrap().last().unwrap().clone();
    assert!(start.find("Get-Process -Name 'inyon', 'inyon-engine', 'tether-node'").unwrap() < start.find("Start-ScheduledTask").unwrap(), "older copies stop first: {start}");
}

#[test]
#[cfg_attr(windows, ignore)]
fn no_service_manager_the_node_named_in_node_json_is_stopped_and_the_new_one_started_with_nohup() {
    let _g = lock();
    let home = setup(&["sh"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, home.dir.join("x").as_path());
    home.put(".inyon/node/node.json", r#"{"pid":777,"version":"0.6.3"}"#);
    proc.set(777, &["/home/me/.bun/bin/bun", "/home/me/.tether-node/tether-node.ts"]);
    assert!(restart_service(&sys, Role::Node, &v(&[BIN])).ok);
    assert_eq!(*proc.signals.borrow(), vec![(777, "SIGTERM".to_string())]);
    let pid = home.dir.join(".inyon/node/node.pid");
    let last = home.calls().last().unwrap().clone();
    assert_eq!(last, v(&["sh", "-c", &format!("nohup '{BIN}' 'connect' >> '{}/.inyon/node/node.log' 2>&1 < /dev/null & echo $! > '{}'", home.dir.display(), pid.display())]));
}

#[test]
fn a_pid_in_node_json_that_now_belongs_to_another_program_is_left_alone() {
    let _g = lock();
    let home = setup(&["sh"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, home.dir.join("x").as_path());
    home.put(".inyon/node/node.json", r#"{"pid":777}"#);
    proc.set(777, &["/usr/bin/vim"]);
    assert!(!restart_service(&sys, Role::Node, &v(&[BIN])).ok);
    assert!(proc.signals.borrow().is_empty());
    assert!(home.calls().is_empty());
}

#[test]
#[cfg_attr(windows, ignore)]
fn the_app_starts_the_update_on_its_own_a_transient_unit_on_linux_with_systemd() {
    let _g = lock();
    let home = setup(&["systemd-run", "systemctl"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, home.dir.join("x").as_path());
    assert!(detach_update(&sys, &v(&[BIN])).ok);
    let last = home.calls().last().unwrap().clone();
    assert_eq!(&last[..4], &v(&["systemd-run", "--user", "--collect", "--quiet"])[..]);
    assert_eq!(&last[last.len() - 2..], &v(&[BIN, "update"])[..]);
    assert!(!home.exists(".inyon/node/update.log"));
}

#[test]
#[cfg_attr(windows, ignore)]
fn without_systemd_the_update_runs_detached_in_a_session_of_its_own_logging_to_a_file() {
    let _g = lock();
    let home = setup(&["inyon"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, home.dir.join("x").as_path());
    let program = home.fakes.join("inyon");
    assert!(detach_update(&sys, &v(&[&program.to_string_lossy()])).ok);
    // the detached copy runs on its own: wait for it to have written its line
    let mut log = String::new();
    for _ in 0..40 {
        log = home.read(".inyon/node/update.log");
        if log.contains("inyon ran update") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(log.contains("inyon ran update"), "{log}");
    let calls = home.calls();
    assert_eq!(calls, vec![v(&["inyon", "update"])]);
}

#[test]
#[cfg_attr(windows, ignore)]
fn on_windows_the_update_becomes_a_one_off_task_outside_the_task_it_was_asked_from() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("x").as_path());
    let cli = home.dir.join("inyon.exe").to_string_lossy().into_owned();
    assert!(detach_update(&sys, &v(&[&cli])).ok);
    let ps = home.calls()[0].last().unwrap().clone();
    assert!(ps.contains(&format!("New-ScheduledTaskAction -Execute '{cli}' -Argument '\"update\"'")), "{ps}");
    assert!(ps.contains("Start-ScheduledTask -TaskName 'Inyon update'"), "{ps}");
    // with the engine next to it, through the engine, so no console window opens
    let engine = home.dir.join("inyon-engine.exe").to_string_lossy().into_owned();
    std::fs::write(&engine, "").unwrap();
    home.clear_calls();
    assert!(detach_update(&sys, &v(&[&cli])).ok);
    let ps = home.calls()[0].last().unwrap().clone();
    assert!(ps.contains(&format!("New-ScheduledTaskAction -Execute '{engine}' -Argument '\"hidden\" \"{cli}\" \"update\"'")), "{ps}");
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn tether_nodes_systemd_unit_moves_to_inyons_stopped_first_removed_once_the_new_one_runs() {
    let _g = lock();
    let home = setup(&["systemctl", "loginctl", "crontab"]);
    let proc = FakeProc::new(&home);
    let program = home.dir.join("download").join("inyon");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    let sys = sys(&proc, Platform::Linux, &program);
    home.unit("tether-node", "\"/home/me/.tether-node/bin/tether-node\" \"link\"");
    assert_eq!(old_services(&sys), vec![Role::Link]);
    let r = move_old_service(&sys, Role::Link);
    assert!(r.ok, "{}", r.how);
    let calls = home.calls();
    let at = |cmd: &[&str]| calls.iter().position(|c| c == &v(cmd));
    let stop = at(&["systemctl", "--user", "stop", "tether-node.service"]).unwrap();
    let start = at(&["systemctl", "--user", "enable", "--now", "inyon-link.service"]).unwrap();
    let disable = at(&["systemctl", "--user", "disable", "tether-node.service"]).unwrap();
    assert!(start > stop);
    assert!(disable > start);
    let unit = home.read(".config/systemd/user/inyon-link.service");
    assert!(unit.lines().any(|l| l.starts_with("ExecStart=") && l.ends_with(" \"link\"")), "{unit}");
    assert!(!home.exists(".config/systemd/user/tether-node.service"));
    assert_eq!(old_services(&sys), Vec::<Role>::new());
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn if_inyons_own_unit_will_not_start_tether_nodes_is_started_again_and_kept() {
    let _g = lock();
    let home = setup(&["systemctl", "loginctl"]);
    let proc = FakeProc::new(&home);
    let program = home.dir.join("download").join("inyon");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    let sys = sys(&proc, Platform::Linux, &program);
    home.unit("tether-node", "/home/me/.bun/bin/bun /home/me/.tether-node/tether-node.ts");
    home.refuse("enable");
    assert_eq!(old_services(&sys), vec![Role::Node]);
    let r = move_old_service(&sys, Role::Node);
    assert_eq!(r, Installed { ok: false, how: "Failed to enable".into() });
    assert_eq!(home.calls().last().unwrap(), &v(&["systemctl", "--user", "start", "tether-node.service"]));
    assert!(home.exists(".config/systemd/user/tether-node.service"));
}

#[test]
#[cfg_attr(windows, ignore)]
fn windows_the_move_to_the_built_in_engine_runs_in_a_task_of_its_own_never_the_updates() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("inyon.exe").as_path());
    // registering the update's task again would end an update running in it
    assert!(inyon_cli::commands::migrate(&sys, true).is_ok());
    let ps = home.calls()[0].last().unwrap().clone();
    assert!(ps.contains("Register-ScheduledTask -TaskName 'Inyon engine hand-over'"), "{ps}");
    assert!(ps.contains("Start-ScheduledTask -TaskName 'Inyon engine hand-over'"), "{ps}");
    assert!(!ps.contains("Inyon update"), "{ps}");
    // and when it's done it removes its own task, never the update's
    home.clear_calls();
    assert!(inyon_cli::commands::migrate(&sys, false).is_ok());
    let all: Vec<String> = home.calls().iter().map(|c| c.join(" ")).collect();
    assert!(all.iter().any(|c| c.contains("Unregister-ScheduledTask -TaskName 'Inyon engine hand-over'")), "{all:?}");
    assert!(!all.iter().any(|c| c.contains("Inyon update")), "{all:?}");
}

#[test]
#[cfg_attr(windows, ignore)]
fn windows_the_move_to_the_built_in_engine_waits_while_an_update_restarts_the_jobs() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("inyon.exe").as_path());
    // what 0.18's update leaves: the Bun engine under the engine's name, and it's still checking the restart
    home.put(&format!(".inyon/bin/inyon-engine{}", std::env::consts::EXE_SUFFIX), "the Bun engine");
    home.put(".inyon/node/update-status", "running\n");
    // no tether-node tasks to move: only the engine's move is left
    std::env::set_var("FAKE_PS_NO", "Tether");
    assert!(inyon_cli::commands::wants_hand_over(&sys));
    // the link's icon and the hub connection's start together: one of them asks
    assert!(!inyon_cli::commands::wants_hand_over(&sys));
    let said = inyon_cli::commands::migrate(&sys, false).unwrap()["message"].as_str().unwrap().to_string();
    assert!(said.contains("an update is still running"), "{said}");
    let all: Vec<String> = home.calls().iter().map(|c| c.join(" ")).collect();
    assert!(!all.iter().any(|c| c.contains("Register-ScheduledTask") || c.contains("Stop-ScheduledTask")), "nothing restarted: {all:?}");
    // it didn't run, so the next start of a tray icon tries again rather than an hour later
    assert!(!home.exists(".inyon/node/engine-hand-over"));
    // the update said it's done, but the link it restarted still answers as the old version
    home.put(".inyon/node/update-status", "done 0.19.0\n");
    proc.set(4242, &["inyon-engine.exe", "link"]);
    home.put(".inyon/node/state.json", r#"{"pid":4242,"version":"0.6.3"}"#);
    let said = inyon_cli::commands::migrate(&sys, false).unwrap()["message"].as_str().unwrap().to_string();
    std::env::remove_var("FAKE_PS_NO");
    assert!(said.contains("an update is still running"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn the_bun_switches_are_no_longer_read_and_the_notes_they_left_go() {
    let _g = lock();
    let home = setup(&["systemctl", "loginctl", "systemd-run"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, std::path::Path::new(BIN));
    // a machine 0.22 set up on the Bun engine and the Bun hub, which an update left in place
    home.put(".inyon/node/legacy-engine", "INYON_LEGACY_ENGINE=1 when the services were set up\n");
    home.put(".inyon/node/hub-impl", "INYON_HUB_IMPL=bun when the hub was set up\n");
    home.put(".inyon/hub.env", "INYON_HUB_IMPL=bun\n");
    home.put(".inyon/bin/inyon-engine", "the Bun engine");
    std::env::set_var("INYON_LEGACY_ENGINE", "1");
    let line = inyon_cli::engine::retired().unwrap();
    assert!(line.contains("INYON_LEGACY_ENGINE and INYON_HUB_IMPL=bun are no longer read"), "{line}");
    assert!(!home.exists(".inyon/node/legacy-engine") && !home.exists(".inyon/node/hub-impl"));
    // setting up goes ahead on the hub built into inyon, and the service doesn't carry the switch
    assert!(inyon_cli::commands::service_install(&sys, Some("link")).is_ok());
    assert!(inyon_cli::commands::service_install(&sys, Some("hub")).is_ok());
    assert!(inyon_cli::commands::hub_start(&sys).is_ok());
    let unit = home.read(".config/systemd/user/inyon-hub.service");
    assert!(unit.contains(&format!("ExecStart=\"{BIN}\" \"hub\"")) && !unit.contains("LEGACY"), "{unit}");
    assert!(!home.read(".config/systemd/user/inyon-link.service").contains("LEGACY"));
    // an update started on its own doesn't take it along either
    home.clear_calls();
    assert!(detach_update(&sys, &v(&[BIN])).ok);
    let last = home.calls().last().unwrap().clone();
    assert!(!last.iter().any(|a| a.contains("LEGACY") || a.contains("HUB_IMPL")), "{last:?}");
    // the Bun engine left beside inyon is nothing's any more
    assert!(inyon_cli::engine::stale_engine().is_some());
    std::env::remove_var("INYON_LEGACY_ENGINE");
    // with only the notes, it says the machine moved; with nothing, nothing
    home.put(".inyon/hub.env", "PORT=4777\n");
    home.put(".inyon/node/hub-impl", "INYON_HUB_IMPL=bun when the hub was set up\n");
    assert!(inyon_cli::engine::retired().is_some_and(|l| l.contains("removed in 0.23")));
    assert_eq!(inyon_cli::engine::retired(), None);
    // INYON_HUB_IMPL=rust asked for what runs anyway
    std::env::set_var("INYON_HUB_IMPL", "rust");
    assert_eq!(inyon_cli::engine::retired(), None);
    std::env::remove_var("INYON_HUB_IMPL");
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn restarting_a_hub_that_is_not_there_says_so_without_fetching_anything() {
    let _g = lock();
    let home = setup(&["systemctl", "loginctl", "systemd-run"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Linux, std::path::Path::new(BIN));
    // a machine with only the link; the engine isn't here and couldn't be fetched
    std::env::set_var("INYON_DOWNLOAD", "http://127.0.0.1:9/nothing");
    let r = inyon_cli::commands::hub_restart(&sys);
    std::env::remove_var("INYON_DOWNLOAD");
    assert!(r.as_ref().is_err_and(|e| e.contains("no background service or process found")), "{r:?}");
    assert!(!home.dir.join(".inyon/bin/inyon-engine").exists());
}

#[test]
#[cfg_attr(windows, ignore)]
fn quit_on_windows_ends_the_task_then_the_engine_then_its_tunnel_as_restart_does() {
    let _g = lock();
    let home = setup(&["schtasks.exe", "powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, std::path::Path::new(BIN));
    proc.set(4242, &["inyon-engine.exe", "link"]);
    inyon_cli::service::quit(&sys, Role::Link, Some(4242));
    let calls = home.calls();
    assert_eq!(calls[0], v(&["schtasks.exe", "/End", "/TN", "Inyon link"]));
    assert_eq!(proc.signals.borrow().as_slice(), &[(4242, "SIGKILL".to_string())]);
    // cloudflared from ~/.inyon/bin, which kept answering for a link that had stopped
    let tunnel = calls.iter().position(|c| c[0] == "powershell.exe" && c.last().unwrap().contains("Name='cloudflared.exe'")).expect("the tunnel is stopped");
    assert!(calls[tunnel].last().unwrap().contains("Stop-Process"));
    // Restart stops it the same way
    home.clear_calls();
    std::env::set_var("FAKE_PS_NO", "Get-ScheduledTask -TaskName 'Inyon link' -ErrorAction SilentlyContinue) -or");
    restart_service(&sys, Role::Link, &v(&[BIN]));
    std::env::remove_var("FAKE_PS_NO");
    assert!(home.calls().iter().any(|c| c.last().unwrap().contains("Name='cloudflared.exe'")));
    // the hub connection has no tunnel to stop
    home.clear_calls();
    inyon_cli::service::quit(&sys, Role::Node, None);
    assert_eq!(home.calls(), vec![v(&["schtasks.exe", "/End", "/TN", "Inyon node"])]);
}

// both sign-in tasks open the app and one app runs both jobs, so one task's process is the app:
// ending that task to stop the hub connection killed the phone link with it
#[test]
#[cfg_attr(windows, ignore)]
fn windows_with_the_app_running_a_stop_or_uninstall_leaves_its_task_running() {
    let _g = lock();
    let home = setup(&["schtasks.exe", "powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, std::path::Path::new(BIN));
    proc.set(5150, &["inyon-app.exe", "app", "--background", "link"]);
    let seen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    home.put(
        ".inyon/node/app.json",
        &format!(r#"{{"pid":5150,"version":"0.20.3","started":{seen},"seen":{seen},"running":["link","hub connection"],"jobs":[{{"role":"link"}},{{"role":"node"}}]}}"#),
    );
    inyon_cli::service::quit(&sys, Role::Node, None);
    assert!(!home.calls().iter().any(|c| c.contains(&"/End".to_string())), "{:?}", home.calls());
    home.clear_calls();
    std::env::set_var("FAKE_PS_NO", "Get-ScheduledTask -TaskName 'Inyon node'");
    inyon_cli::service::uninstall_service(&sys, Role::Node);
    std::env::remove_var("FAKE_PS_NO");
    let calls = home.calls();
    let script = calls.iter().map(|c| c.last().unwrap().clone()).find(|ps| ps.contains("Unregister-ScheduledTask")).expect("the task is unregistered");
    assert!(!script.contains("Stop-ScheduledTask"), "{script}");
    assert!(proc.signals.borrow().is_empty(), "the app itself is left running");
}

// an update by 0.20.2 ended by writing its own version into Settings → Apps, after the new
// version had listed itself: the app puts the installed version back, and from 0.20.4 the entry
// names the installed program's version, not the updater's
#[test]
#[cfg_attr(windows, ignore)]
fn windows_settings_apps_names_the_installed_version_and_is_put_right_when_an_older_updater_wrote_its_own() {
    let _g = lock();
    let home = setup(&["powershell.exe"]);
    let proc = FakeProc::new(&home);
    let sys = sys(&proc, Platform::Windows, home.dir.join("x").as_path());
    let bin = home.dir.join(".inyon").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut program = fake_exe(3);
    program.extend_from_slice(inyon_cli::engine::COPY_MARK.as_bytes());
    program.extend_from_slice(b"..inyon-build:0.20.9;..");
    std::fs::write(bin.join("inyon"), &program).unwrap();
    let entry = |calls: &[Vec<String>]| calls.iter().map(|c| c.last().unwrap().clone()).find(|ps| ps.contains("DisplayVersion ="));
    // the entry is this version's: nothing is written
    assert!(!inyon_cli::service::repair_app_entry(&sys));
    let calls = home.calls();
    let check = calls[0].last().unwrap();
    let app = bin.join("inyon-app").to_string_lossy().into_owned();
    assert!(check.contains("$e.DisplayVersion -eq '0.20.9'") && check.contains(&format!("$e.UninstallString -eq '\"{app}\" uninstall --purge'")), "{check}");
    assert!(entry(&calls).is_none());
    // an older updater's entry: listed again, with the installed program's version
    home.clear_calls();
    std::env::set_var("FAKE_PS_NO", "$e.DisplayVersion -eq");
    assert!(inyon_cli::service::repair_app_entry(&sys));
    std::env::remove_var("FAKE_PS_NO");
    let script = entry(&home.calls()).expect("the entry is written");
    assert!(script.contains("DisplayVersion = '0.20.9'") && script.contains("inyon-app\" uninstall --purge"), "{script}");
}
