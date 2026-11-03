//! Takes back what the installers did to put `inyon` on the PATH: the line get.sh marked
//! "added by Inyon" in a shell's startup file, and on Windows the folder get.ps1 put on the
//! user's Path. Lines you wrote yourself stay, even ones that mention ~/.inyon/bin.

use std::fs;
use std::path::{Path, PathBuf};

pub const MARK: &str = "# added by Inyon";

/// Where get.sh may have written its line (fish's file is Inyon's own).
fn startup_files(home: &Path, zdotdir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files = vec![home.join(".zshrc"), home.join(".bashrc"), home.join(".bash_profile"), home.join(".profile")];
    if let Some(z) = zdotdir.filter(|z| z.as_path() != home) {
        files.insert(0, z.join(".zshrc"));
    }
    files
}

/// `text` without the marked lines; None when there were none.
fn without_mark(text: &str) -> Option<String> {
    if !text.lines().any(|l| l.trim_end().ends_with(MARK)) {
        return None;
    }
    let mut out: String = text.lines().filter(|l| !l.trim_end().ends_with(MARK)).map(|l| format!("{l}\n")).collect();
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

/// Removes the marked lines; says which files changed.
pub fn remove_unix(home: &Path, zdotdir: Option<PathBuf>) -> Vec<String> {
    let mut said = Vec::new();
    for f in startup_files(home, zdotdir) {
        let Ok(text) = fs::read_to_string(&f) else { continue };
        if let Some(rest) = without_mark(&text) {
            // written in place, so the file keeps its permissions and a link stays a link
            if fs::write(&f, rest).is_ok() {
                said.push(format!("took inyon off the PATH in {}", f.display()));
            }
        }
    }
    let fish = home.join(".config/fish/conf.d/inyon.fish");
    if let Ok(text) = fs::read_to_string(&fish) {
        match without_mark(&text) {
            Some(rest) if rest.trim().is_empty() => {
                if fs::remove_file(&fish).is_ok() {
                    said.push(format!("removed {}", fish.display()));
                }
            }
            Some(rest) => {
                if fs::write(&fish, rest).is_ok() {
                    said.push(format!("took inyon off the PATH in {}", fish.display()));
                }
            }
            None => {}
        }
    }
    said
}

/// The PowerShell that takes `%USERPROFILE%\.inyon\bin` off the user's Path, leaving every other
/// entry exactly as it was written (unexpanded), and tells open programs the Path changed.
pub const WINDOWS_SCRIPT: &str = r#"$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$p = [string]$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
$d = Join-Path $env:USERPROFILE '.inyon\bin'
$all = @($p -split ';' | Where-Object { $_ })
$keep = @($all | Where-Object { $_.TrimEnd('\') -ne $d -and $_.TrimEnd('\') -ne '%USERPROFILE%\.inyon\bin' })
if ($keep.Count -ne $all.Count) {
  $k.SetValue('Path', ($keep -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
  [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', '1', 'User')
  [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', $null, 'User')
  'removed'
}
$k.Close()"#;

/// The PowerShell that puts `%USERPROFILE%\.inyon\bin` on the user's Path when it isn't there,
/// the way get.ps1 does, so `inyon setup` run from a copy that got there some other way (or
/// after an uninstall took the entry off) also leaves `inyon` findable by name.
pub const WINDOWS_ADD_SCRIPT: &str = r#"$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$p = [string]$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
$d = Join-Path $env:USERPROFILE '.inyon\bin'
$all = @($p -split ';' | Where-Object { $_ })
if (-not ($all | Where-Object { $_.TrimEnd('\') -eq $d -or $_.TrimEnd('\') -eq '%USERPROFILE%\.inyon\bin' })) {
  $k.SetValue('Path', ((@($d) + $all) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
  [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', '1', 'User')
  [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', $null, 'User')
  'added'
}
$k.Close()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_scripts_add_and_remove_the_same_entry() {
        for s in [WINDOWS_SCRIPT, WINDOWS_ADD_SCRIPT] {
            assert!(s.contains(r"Join-Path $env:USERPROFILE '.inyon\bin'"));
            assert!(s.contains("DoNotExpandEnvironmentNames"), "other entries stay as written");
            assert!(s.contains("INYON_PATH_CHANGED"), "open programs hear of the change");
        }
        assert!(WINDOWS_ADD_SCRIPT.contains("'added'") && WINDOWS_SCRIPT.contains("'removed'"));
    }

    /// Numbered, not timed: two tests at once can read the same clock on Windows.
    fn dir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("inyon-shellpath-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn only_the_marked_lines_go() {
        let h = dir();
        let line = format!("export PATH=\"$HOME/.inyon/bin:$PATH\" {MARK}");
        fs::write(h.join(".zshrc"), format!("alias ll='ls -l'\n{line}\nexport PATH=\"$HOME/.inyon/bin:$PATH\"\n")).unwrap();
        fs::write(h.join(".bashrc"), "# nothing of ours\n").unwrap();
        let said = remove_unix(&h, None);
        assert_eq!(said.len(), 1, "{said:?}");
        // a line you wrote yourself stays, even the same export
        assert_eq!(fs::read_to_string(h.join(".zshrc")).unwrap(), "alias ll='ls -l'\nexport PATH=\"$HOME/.inyon/bin:$PATH\"\n");
        assert_eq!(fs::read_to_string(h.join(".bashrc")).unwrap(), "# nothing of ours\n");
        // nothing left to take out the second time
        assert!(remove_unix(&h, None).is_empty());
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn fish_file_goes_and_zdotdir_is_read() {
        let h = dir();
        let z = h.join(".config/zsh");
        fs::create_dir_all(h.join(".config/fish/conf.d")).unwrap();
        fs::create_dir_all(&z).unwrap();
        fs::write(h.join(".config/fish/conf.d/inyon.fish"), format!("contains $HOME/.inyon/bin $PATH; or set -gx PATH $HOME/.inyon/bin $PATH {MARK}\n")).unwrap();
        fs::write(z.join(".zshrc"), format!("setopt autocd\nexport PATH=\"$HOME/.inyon/bin:$PATH\" {MARK}")).unwrap();
        let said = remove_unix(&h, Some(z.clone()));
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(!h.join(".config/fish/conf.d/inyon.fish").exists());
        assert_eq!(fs::read_to_string(z.join(".zshrc")).unwrap(), "setopt autocd");
        let _ = fs::remove_dir_all(&h);
    }
}
