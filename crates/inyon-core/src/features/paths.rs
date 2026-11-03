//! The path checks from features/features.ts that the actions need (`isPlainPath`, `isTaskFile`).
//! The rest of features.ts (transcripts, subagents, task output) is hub routes, not ported here.

use crate::re;

/// An absolute path with nothing a shell or `..` could turn into another one.
pub fn is_plain_path(p: &str) -> bool {
    let absolute = p.starts_with('/') || re!(r"^[A-Za-z]:/").is_match(p);
    if !absolute || p.contains('\0') || p.contains('\n') {
        return false;
    }
    let segs: Vec<&str> = p.split('/').skip(1).collect();
    let n = segs.len();
    !segs.iter().enumerate().any(|(i, s)| *s == "." || *s == ".." || (s.is_empty() && i + 1 < n))
}

/// A background task's output file: in a temp folder (or agy's under home), named like one.
pub fn is_task_file(p: &str, home: Option<&str>) -> bool {
    let home_root = home.filter(|h| !h.is_empty()).map(|h| format!("{h}/"));
    let mut roots = vec!["/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/"];
    if let Some(h) = &home_root {
        roots.push(h);
    }
    is_plain_path(p)
        && roots.iter().any(|r| p.starts_with(r))
        && (re!(r"/tasks/[\w-]+\.output$").is_match(p) || re!(r"/\.gemini/antigravity-cli/brain/[\w-]+/\.system_generated/tasks/[\w-]+\.log$").is_match(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    // features/paths.test.ts
    #[test]
    fn task_output_only_from_temp_folders_or_agy_under_home() {
        let home = Some("/home/me");
        assert!(is_task_file("/tmp/claude-1000/-p/s1/tasks/b1.output", home));
        assert!(is_task_file("/private/tmp/claude-501/-p/s1/tasks/b1.output", home));
        assert!(is_task_file("/home/me/.gemini/antigravity-cli/brain/c1/.system_generated/tasks/t1.log", home));
        for bad in ["/tmp/../home/me/x/tasks/b1.output", "/tmp//tasks/b1.output", "/etc/tasks/b1.output", "/srv/x/.gemini/antigravity-cli/brain/c1/.system_generated/tasks/t1.log"]
        {
            assert!(!is_task_file(bad, home), "{bad}");
        }
    }
}
