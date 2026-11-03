//! hosts/via-backend.test.ts

use super::*;
use crate::test_support::rt::run;
use serde_json::json;

fn config(v: serde_json::Value) -> HostConfig {
    serde_json::from_value(v).unwrap()
}

#[test]
fn a_command_reaches_the_server_through_the_computer_exactly_as_written() {
    let dir = std::env::temp_dir().join(format!("inyon-via-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.display();
    // an ssh that records how it was called and runs the remote command the way a login shell would
    std::fs::write(dir.join("ssh"), format!("#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"{d}/args\"; done\nfor last in \"$@\"; do :; done\nexec sh -c \"$last\"\n"))
        .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let cmd = r#"printf '%s|' "it's" "a \"quoted\" \$HOME" | tr a-z A-Z"#;
    let line = via_command(&config(json!({ "id": "s", "name": "s", "kind": "ssh", "target": "work-db", "port": 2222, "via": "jump" })), cmd, false);
    let out = std::process::Command::new("sh").arg("-c").arg(line).env("PATH", format!("{d}:{}", std::env::var("PATH").unwrap())).env("HOME", &dir).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), r#"IT'S|A "QUOTED" $HOME|"#);
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    let argv: Vec<&str> = args.trim().split('\n').collect();
    // no prompts, one shared connection under the computer's home, and the alias left to its config
    assert!(argv.contains(&"BatchMode=yes"));
    assert!(argv.contains(&format!("ControlPath={d}/.cache/inyon-ssh/%C").as_str()));
    let p = argv.iter().position(|a| *a == "-p").unwrap();
    assert_eq!(&argv[p..p + 2], ["-p", "2222"]);
    let dd = argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(argv[dd + 1], "work-db");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn when_the_computer_isnt_connected_the_servers_error_names_it_not_inyon() {
    run(|| async {
        let jump = config(json!({ "id": "jump-offline", "name": "Jump box", "kind": "link" }));
        let j = jump.clone();
        let via = via_backend(move |id| (id == j.id).then(|| j.clone()));
        let r = via.exec(&config(json!({ "id": "db", "name": "DB", "kind": "ssh", "target": "work-db", "via": jump.id })), "true", ExecOptions::default()).await.unwrap();
        assert_eq!(r.code, 255);
        assert_eq!(r.stderr, "Jump box isn't connected right now, and this server is reached through it.");
        // and a computer that was unpaired since
        let r = via.exec(&config(json!({ "id": "db", "name": "DB", "kind": "ssh", "target": "work-db", "via": "gone" })), "true", ExecOptions::default()).await.unwrap();
        assert!(r.stderr.contains("no longer paired"));
    });
}
