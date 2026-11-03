//! Takes Inyon's hooks and status lines back out of the agents' settings, so removing ~/.inyon
//! never leaves Claude Code or Antigravity pointing at scripts that are gone. The status line you
//! had before Inyon (kept in ~/.inyon/*.orig) comes back.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// A command that runs one of Inyon's scripts (written as ~/.inyon/… or, before the rename, ~/.tether/…).
fn ours(command: &Value, file: &str) -> bool {
    command.as_str().is_some_and(|c| c.contains(&format!(".inyon/{file}")) || c.contains(&format!(".tether/{file}")))
}

fn read_orig(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Puts the status line back the way it was before Inyon, or removes it when there was none.
fn restore_status(settings: &mut Value, script: &str, orig: Option<String>) -> bool {
    let Some(obj) = settings.as_object_mut() else { return false };
    let mine = obj.get("statusLine").and_then(|s| s.get("command")).is_some_and(|c| ours(c, script));
    if !mine {
        return false;
    }
    match orig.filter(|o| !ours(&Value::String(o.clone()), script)) {
        Some(cmd) => {
            let sl = obj.get_mut("statusLine").and_then(|s| s.as_object_mut()).expect("checked above");
            sl.insert("command".into(), Value::String(cmd));
            sl.remove("stack_with_default");
        }
        None => {
            obj.remove("statusLine");
        }
    }
    true
}

/// Drops every hook that runs Inyon's hook.sh, and any event left with none.
fn drop_hooks(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return false };
    let mut changed = false;
    for groups in hooks.values_mut() {
        let Some(list) = groups.as_array_mut() else { continue };
        for g in list.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                let before = hs.len();
                hs.retain(|h| !h.get("command").is_some_and(|c| ours(c, "hook.sh")));
                changed |= hs.len() != before;
            }
        }
        list.retain(|g| g.get("hooks").and_then(|h| h.as_array()).is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        settings.as_object_mut().map(|o| o.remove("hooks"));
    }
    changed
}

/// Rewrites a settings file in place: through a temporary file, keeping the original's permissions.
fn write(path: &Path, settings: &Value) -> std::io::Result<()> {
    let tmp = path.with_extension("json.inyon-tmp");
    fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(settings).unwrap_or_default()))?;
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path)
}

/// Takes Inyon out of one settings file; says what it did, if anything.
fn take_out(file: &Path, script: &str, orig: &Path, hooks: bool) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let mut settings: Value = serde_json::from_str(&text).ok()?;
    let status = restore_status(&mut settings, script, read_orig(orig));
    let hooked = hooks && drop_hooks(&mut settings);
    if !(status || hooked) {
        return None;
    }
    write(file, &settings).ok()?;
    Some(format!("took Inyon's {} out of {}", if hooked { "hooks and status line" } else { "status line" }, file.display()))
}

/// Run before ~/.inyon goes: the agents stop calling scripts that are about to be deleted.
pub fn unhook(home: &Path) -> Vec<String> {
    let inyon = home.join(".inyon");
    [
        take_out(&home.join(".claude").join("settings.json"), "statusline.sh", &inyon.join("statusline.orig"), true),
        take_out(&home.join(".gemini").join("antigravity-cli").join("settings.json"), "agy-statusline.sh", &inyon.join("agy-statusline.orig"), false),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn home() -> tempfile_dir::Dir {
        tempfile_dir::Dir::new()
    }

    /// A throwaway folder, without pulling in a crate for it.
    mod tempfile_dir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!("inyon-unhook-{}-{}", std::process::id(), rand_suffix()));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        /// A counter, not the clock: Windows hands two tests running at once the same nanoseconds,
        /// and they then shared a folder and read each other's settings.
        fn rand_suffix() -> u64 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn put(dir: &Path, rel: &str, v: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, v).unwrap();
    }

    #[test]
    fn claude_gets_its_own_status_line_back_and_loses_only_inyons_hooks() {
        let h = home();
        put(&h.0, ".inyon/statusline.orig", "node ~/my-status.js\n");
        let settings = json!({
            "model": "opus",
            "statusLine": { "type": "command", "command": "~/.inyon/statusline.sh", "padding": 0 },
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "~/.inyon/hook.sh Stop" }, { "type": "command", "command": "say done" }] }],
                "SessionStart": [{ "hooks": [{ "type": "command", "command": "~/.inyon/hook.sh SessionStart" }] }]
            }
        });
        put(&h.0, ".claude/settings.json", &settings.to_string());
        let said = unhook(&h.0);
        assert_eq!(said.len(), 1);
        let after: Value = serde_json::from_str(&fs::read_to_string(h.0.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(
            after,
            json!({
                "model": "opus",
                "statusLine": { "type": "command", "command": "node ~/my-status.js", "padding": 0 },
                "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }] }
            })
        );
        // the keys stay in the order you wrote them
        let text = fs::read_to_string(h.0.join(".claude/settings.json")).unwrap();
        assert!(text.find("\"model\"").unwrap() < text.find("\"statusLine\"").unwrap());
    }

    #[test]
    fn with_no_status_line_before_inyon_it_goes_and_untouched_files_stay_byte_for_byte() {
        let h = home();
        put(&h.0, ".gemini/antigravity-cli/settings.json", r#"{"statusLine":{"type":"command","command":"~/.inyon/agy-statusline.sh","stack_with_default":true}}"#);
        let mine = r#"{ "statusLine": { "command": "my-own" } }"#;
        put(&h.0, ".claude/settings.json", mine);
        unhook(&h.0);
        assert_eq!(fs::read_to_string(h.0.join(".claude/settings.json")).unwrap(), mine);
        let agy: Value = serde_json::from_str(&fs::read_to_string(h.0.join(".gemini/antigravity-cli/settings.json")).unwrap()).unwrap();
        assert_eq!(agy, json!({}));
    }

    #[test]
    fn broken_or_missing_settings_are_left_alone() {
        let h = home();
        put(&h.0, ".claude/settings.json", "{ not json");
        assert!(unhook(&h.0).is_empty());
        assert_eq!(fs::read_to_string(h.0.join(".claude/settings.json")).unwrap(), "{ not json");
    }
}
