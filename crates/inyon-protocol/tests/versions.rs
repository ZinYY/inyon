//! versions.ts in Rust: the same constants, and release numbers compared the same way.

use inyon_protocol::*;

fn repo(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)).unwrap()
}

#[test]
fn the_version_is_the_root_package_json_one() {
    let pkg: serde_json::Value = serde_json::from_str(&repo("package.json")).unwrap();
    assert_eq!(pkg["version"], VERSION);
    assert!(repo("packages/protocol/src/index.ts").contains(&format!("export const VERSION = '{VERSION}'")));
}

#[test]
fn constants_match_versions_ts() {
    let ts = repo("packages/protocol/src/versions.ts");
    for (name, value) in [
        ("PRODUCT", PRODUCT),
        ("SLUG", SLUG),
        ("OLD_SLUG", OLD_SLUG),
        ("SOURCE_REPO", SOURCE_REPO),
        ("WEB_URL", WEB_URL),
        ("INSTALL_URL", INSTALL_URL),
        ("APP_ID", APP_ID),
        ("ENGINE", ENGINE),
        ("RELEASE_KEY", RELEASE_KEY),
    ] {
        assert!(ts.contains(&format!("export const {name} = '{value}'")), "{name} differs from versions.ts");
    }
    assert!(ts.contains("export const RELEASES_REPO = SOURCE_REPO"));
    assert_eq!(RELEASES_REPO, SOURCE_REPO);
    assert!(ts.contains("export const DOWNLOAD_URL = `${INSTALL_URL}/download`"));
    assert_eq!(DOWNLOAD_URL, format!("{INSTALL_URL}/download"));
    assert!(ts.contains("export const LATEST_URL = `${DOWNLOAD_URL}/latest.json`"));
    assert_eq!(LATEST_URL, format!("{DOWNLOAD_URL}/latest.json"));
    // the inyon command checks its own updates with the same key
    assert!(include_str!("../../../apps/cli/src/signing.rs").contains(&format!("pub const RELEASE_KEY: &str = \"{RELEASE_KEY}\";")));
}

#[test]
fn release_files_are_named_per_system() {
    assert_eq!(release_files("linux-x64"), ReleaseFiles { cli: "inyon-linux-x64".into(), engine: "inyon-engine-linux-x64".into() });
    assert_eq!(release_files("windows-arm64"), ReleaseFiles { cli: "inyon-windows-arm64.exe".into(), engine: "inyon-engine-windows-arm64.exe".into() });
    assert_eq!(compressed("inyon-linux-x64"), "inyon-linux-x64.gz");
}

#[test]
fn release_numbers() {
    assert_eq!(parse_release("0.6.10"), Some([0, 6, 10]));
    assert_eq!(parse_release(" v0.6.10\n"), Some([0, 6, 10]));
    for bad in ["0.7.0-rc.1", "0.15", "latest", "", "1.2.3.4", "v", "1..2", "+1.2.3"] {
        assert_eq!(parse_release(bad), None, "{bad}");
    }
    assert!(newer_release("0.6.4", "0.6.3"));
    assert!(newer_release("v0.10.0", "0.9.9"));
    assert!(!newer_release("0.6.3", "0.6.3"));
    assert!(!newer_release("0.6.2", "0.6.3"));
    // a build between releases counts as the release it's built on
    assert!(newer_release("0.6.4", "0.6.3-build.41"));
    assert!(!newer_release("0.6.3", "0.6.3-build.41"));
    assert!(!newer_release("0.7.0-rc.1", "0.6.3"));
    assert!(!newer_release("0.7.0", "dev"));
    // a test build comes before its release, and after the one before
    assert!(newer_release("0.19.0", "0.19.0-rc.1"));
    assert!(newer_release("0.19.0", " v0.19.0-rc.2 "));
    assert!(!newer_release("0.18.0", "0.19.0-rc.1"));
    assert!(test_build("v0.19.0-rc.12") && !test_build("0.19.0-rc.") && !test_build("0.19-rc.1") && !test_build("0.19.0-rc.1x"));
    assert_eq!(newest_release(["0.6.4", "v0.7.1", "0.7.0", "0.8.0-rc.1"], "0.6.3").as_deref(), Some("0.7.1"));
    assert_eq!(newest_release(["0.6.2"], "0.6.3"), None);
}

#[test]
fn prompt_keys_ignore_white_space() {
    let opt = |n, label: &str| PromptOption { n, label: label.into(), detail: None, selected: false, checked: None, current: None };
    let a = prompt_key(&["Do you want to proceed?".into()], &[opt(1, "Yes"), opt(2, "No, and tell Claude")]);
    let b = prompt_key(&["Do you want", "to proceed?"].map(String::from), &[opt(1, "Yes"), opt(2, "No,  and tell\u{a0}Claude")]);
    assert_eq!(a, "Doyouwanttoproceed?1.Yes2.No,andtellClaude");
    assert_eq!(a, b);
    assert_eq!(chat_key("local", "%12"), "local:%12");
    assert!(is_busy(AgentState::Working, None));
    assert!(is_busy(AgentState::Idle, Some("1 shell")));
    assert!(!is_busy(AgentState::Idle, Some("")));
    assert!(!is_busy(AgentState::Waiting, Some("1 shell")));
}
