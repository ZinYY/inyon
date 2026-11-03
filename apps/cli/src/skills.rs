//! `inyon skill`: agent skills that ship with each release as `inyon-skill-<name>.tar.gz`, listed
//! in the release's signed SHA256SUMS like the programs. `inyon skill add <name>` downloads one,
//! checks it, and unpacks it into the skills folder of each coding agent on this machine.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::sys::Platform;
use crate::update;

/// What a skill folder Inyon put there carries, so `add` may replace it and `remove` may delete it.
pub const MARKER: &str = ".inyon-skill";
/// The most a skill may unpack to; the largest is well under 1 MB.
const MAX_BYTES: u64 = 32 << 20;

pub fn asset(name: &str) -> String {
    format!("inyon-skill-{name}.tar.gz")
}

/// The skills a release offers, from the names in its SHA256SUMS.
pub fn available(sums: &str) -> Vec<String> {
    let mut out: Vec<String> = sums
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|f| f.trim_start_matches('*'))
        .filter_map(|f| f.strip_prefix("inyon-skill-")?.strip_suffix(".tar.gz"))
        .filter(|n| valid_name(n))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !n.starts_with('-')
}

/// The skills folders to install into: one per agent whose folder is in `home`. Claude Code reads
/// ~/.claude/skills; Codex reads ~/.agents/skills (also the shared folder other agents use);
/// Gemini CLI and Antigravity read ~/.gemini/config/skills. With none of them, ~/.agents/skills.
pub fn targets(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if home.join(".claude").is_dir() {
        out.push(home.join(".claude/skills"));
    }
    if home.join(".codex").is_dir() || home.join(".agents").is_dir() {
        out.push(home.join(".agents/skills"));
    }
    if home.join(".gemini").is_dir() {
        out.push(home.join(".gemini/config/skills"));
    }
    if out.is_empty() {
        out.push(home.join(".agents/skills"));
    }
    out
}

/// Where a skill is installed, among `dirs`.
pub fn installed(dirs: &[PathBuf], name: &str) -> Vec<PathBuf> {
    dirs.iter().map(|d| d.join(name)).filter(|p| p.join("SKILL.md").is_file()).collect()
}

/// The files in a skill's tarball, checked: every path sits under `<name>/`, only plain files and
/// folders, no `..`, a SKILL.md at the top, and no more than MAX_BYTES in all.
pub fn read_tarball(gz: &[u8], name: &str) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let bad = |why: &str| format!("{} isn't a usable skill ({why}), so nothing was installed.", asset(name));
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    let mut files = Vec::new();
    let mut total = 0u64;
    for e in ar.entries().map_err(|e| bad(&e.to_string()))? {
        let mut e = e.map_err(|e| bad(&e.to_string()))?;
        let path = e.path().map_err(|e| bad(&e.to_string()))?.into_owned();
        let mut parts = path.components().filter(|c| !matches!(c, Component::CurDir));
        match parts.next() {
            Some(Component::Normal(top)) if top == name => {}
            _ => return Err(bad(&format!("{} is outside {name}/", path.display()))),
        }
        let rest: PathBuf = parts.clone().collect();
        if parts.any(|c| !matches!(c, Component::Normal(_))) {
            return Err(bad(&format!("{} leaves the folder", path.display())));
        }
        let kind = e.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(bad(&format!("{} isn't a plain file", path.display())));
        }
        total += e.size();
        if total > MAX_BYTES {
            return Err(bad("it is too big"));
        }
        let mut data = Vec::new();
        e.read_to_end(&mut data).map_err(|e| bad(&e.to_string()))?;
        files.push((rest, data));
    }
    if !files.iter().any(|(p, _)| p == Path::new("SKILL.md")) {
        return Err(bad("no SKILL.md"));
    }
    Ok(files)
}

/// Writes `files` to `dir/<name>`. A folder already there is replaced only when Inyon put it there
/// (it has MARKER) or `force` is set; the new copy is written beside it and swapped in, so a failed
/// write leaves the old one.
pub fn install(dir: &Path, name: &str, files: &[(PathBuf, Vec<u8>)], version: &str, force: bool) -> Result<PathBuf, String> {
    let dest = dir.join(name);
    if dest.exists() && !dest.join(MARKER).is_file() && !force {
        return Err(format!("{} already has a skill called {name} that Inyon didn't install. Add --force to replace it.", dir.display()));
    }
    let fail = |e: io::Error| format!("Couldn't write to {}: {e}", dir.display());
    fs::create_dir_all(dir).map_err(fail)?;
    let tmp = dir.join(format!(".{name}.inyon-new"));
    let _ = fs::remove_dir_all(&tmp);
    let write = || -> io::Result<()> {
        for (rel, data) in files {
            let f = tmp.join(rel);
            if let Some(p) = f.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(f, data)?;
        }
        fs::write(tmp.join(MARKER), format!("{version}\n"))
    };
    if let Err(e) = write() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(fail(e));
    }
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(fail)?;
    }
    fs::rename(&tmp, &dest).map_err(fail)?;
    Ok(dest)
}

/// Deletes `dir/<name>` if Inyon installed it there. Returns whether it did.
pub fn remove(dir: &Path, name: &str) -> Result<bool, String> {
    let dest = dir.join(name);
    if !dest.join(MARKER).is_file() {
        return Ok(false);
    }
    fs::remove_dir_all(&dest).map_err(|e| format!("Couldn't remove {}: {e}", dest.display()))?;
    Ok(true)
}

/// The release's checked SHA256SUMS and where its files are, as `inyon update` gets them.
pub fn release(platform: Platform) -> Result<(String, String), String> {
    let base = crate::paths::env_var("INYON_DOWNLOAD").unwrap_or_else(|| update::RELEASES.to_string()).trim_end_matches('/').to_string();
    let key = crate::signing::signing_key(&base);
    let sums = update::release_sums(&base, &key, platform).map_err(|e| e.to_string())?;
    Ok((base, sums))
}

/// Downloads `name`'s tarball from the release, checked against `sums`.
pub fn download(base: &str, sums: &str, name: &str, log: &dyn Fn(&str)) -> Result<Vec<u8>, String> {
    if !available(sums).iter().any(|n| n == name) {
        let have = available(sums);
        return Err(if have.is_empty() {
            format!("This release has no skills to install (it needs 0.17.7 or newer), so {name} wasn't installed.")
        } else {
            format!("There's no skill called {name}. This release has: {}.", have.join(", "))
        });
    }
    let tmp = std::env::temp_dir().join(format!("inyon-skill-{name}-{}.tar.gz", std::process::id()));
    update::fetch_release(base, &asset(name), sums, &tmp, log).map_err(|e| e.to_string())?;
    let data = fs::read(&tmp).map_err(|e| e.to_string());
    let _ = fs::remove_file(&tmp);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            // set the raw name so a test can write paths the builder itself would refuse
            let name = &mut h.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inyon-skills-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_the_skills_in_a_release() {
        let sums = "aa  inyon-linux-x64\nbb  inyon-skill-blueprint.tar.gz\ncc *inyon-skill-other.tar.gz\ndd  inyon-skill-../x.tar.gz\nee  inyon-web.tar.gz\n";
        assert_eq!(available(sums), vec!["blueprint", "other"]);
        assert!(available("aa  inyon-linux-x64\n").is_empty());
    }

    #[test]
    fn names_are_plain() {
        assert!(valid_name("blueprint"));
        assert!(valid_name("a-1"));
        for n in ["", "-x", "Blue", "a/b", "..", "a b", &"x".repeat(65)] {
            assert!(!valid_name(n), "{n}");
        }
    }

    #[test]
    fn installs_for_each_agent_on_the_machine() {
        let h = scratch("targets");
        assert_eq!(targets(&h), vec![h.join(".agents/skills")]);
        fs::create_dir_all(h.join(".claude")).unwrap();
        fs::create_dir_all(h.join(".codex")).unwrap();
        fs::create_dir_all(h.join(".gemini")).unwrap();
        assert_eq!(targets(&h), vec![h.join(".claude/skills"), h.join(".agents/skills"), h.join(".gemini/config/skills")]);
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn reads_a_good_tarball() {
        let gz = tarball(&[("blueprint/SKILL.md", b"---\nname: blueprint\n---\n"), ("blueprint/scripts/a.mjs", b"x"), ("./blueprint/b.txt", b"y")]);
        let files = read_tarball(&gz, "blueprint").unwrap();
        let names: Vec<_> = files.iter().map(|(p, _)| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, vec!["SKILL.md", "scripts/a.mjs", "b.txt"]);
    }

    #[test]
    fn refuses_paths_that_leave_the_folder() {
        for bad in ["blueprint/../evil", "other/SKILL.md", "/etc/passwd", "../blueprint/SKILL.md"] {
            let gz = tarball(&[("blueprint/SKILL.md", b"x"), (bad, b"x")]);
            assert!(read_tarball(&gz, "blueprint").is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_links_and_missing_skill_md() {
        assert!(read_tarball(&tarball(&[("blueprint/README.md", b"x")]), "blueprint").unwrap_err().contains("no SKILL.md"));
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "blueprint/SKILL.md", "/etc/passwd").unwrap();
        let gz = b.into_inner().unwrap().finish().unwrap();
        assert!(read_tarball(&gz, "blueprint").unwrap_err().contains("plain file"));
        assert!(read_tarball(b"not a tarball", "blueprint").is_err());
    }

    #[test]
    fn replaces_only_its_own_copy() {
        let d = scratch("install");
        let files = vec![(PathBuf::from("SKILL.md"), b"one".to_vec()), (PathBuf::from("a/b.txt"), b"b".to_vec())];
        let dest = install(&d, "blueprint", &files, "0.17.7", false).unwrap();
        assert_eq!(fs::read_to_string(dest.join("a/b.txt")).unwrap(), "b");
        assert_eq!(fs::read_to_string(dest.join(MARKER)).unwrap(), "0.17.7\n");
        // an update replaces it, dropping files the new copy doesn't have
        let newer = vec![(PathBuf::from("SKILL.md"), b"two".to_vec())];
        install(&d, "blueprint", &newer, "0.17.8", false).unwrap();
        assert_eq!(fs::read_to_string(dest.join("SKILL.md")).unwrap(), "two");
        assert!(!dest.join("a").exists());
        // someone's own skill of the same name stays unless forced
        fs::create_dir_all(d.join("mine")).unwrap();
        fs::write(d.join("mine/SKILL.md"), "mine").unwrap();
        assert!(install(&d, "mine", &newer, "0.17.8", false).unwrap_err().contains("--force"));
        assert_eq!(fs::read_to_string(d.join("mine/SKILL.md")).unwrap(), "mine");
        assert!(!remove(&d, "mine").unwrap());
        install(&d, "mine", &newer, "0.17.8", true).unwrap();
        assert!(remove(&d, "blueprint").unwrap());
        assert!(!dest.exists());
        assert_eq!(installed(std::slice::from_ref(&d), "mine"), vec![d.join("mine")]);
        fs::remove_dir_all(&d).unwrap();
    }
}
