//! `inyon hub --help` prints help and nothing else: on a joined Windows laptop it fetched the
//! engine to run a hub, and failed with "Access is denied".

use std::process::Command;

#[test]
fn asking_the_hub_command_for_help_prints_it_and_fetches_nothing() {
    let home = std::env::temp_dir().join(format!("inyon-hub-help-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    for ask in [&["hub", "--help"][..], &["hub", "-h"], &["hub", "help"]] {
        let o = Command::new(env!("CARGO_BIN_EXE_inyon"))
            .args(ask)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            // nowhere to fetch from, should it try
            .env("INYON_DOWNLOAD", "http://127.0.0.1:9/")
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(0), "{ask:?}: {out}{err}");
        assert!(out.contains("reset-password") && out.contains("start"), "{ask:?}: {out}");
        assert!(!err.contains("engine") && !home.join(".inyon").join("bin").exists(), "{ask:?}: {err}");
    }
    let _ = std::fs::remove_dir_all(&home);
}
