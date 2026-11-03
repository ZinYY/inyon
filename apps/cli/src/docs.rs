//! `inyon docs`: the docs at inyon.tech, inside the program, so an agent (or you, offline) can read
//! how to set things up without a browser. Each page is the Markdown the site is built from.

use serde_json::{json, Value};

use crate::commands::Outcome;

/// Where the docs are online.
pub const DOCS_URL: &str = "https://inyon.tech/docs/";

/// Every page in site/docs, by its name there. A test checks none is missing.
pub const PAGES: [(&str, &str); 26] = [
    ("agents", include_str!("../../../site/docs/agents.md")),
    ("android", include_str!("../../../site/docs/android.md")),
    ("browser-copy", include_str!("../../../site/docs/browser-copy.md")),
    ("cli", include_str!("../../../site/docs/cli.md")),
    ("companion-agent", include_str!("../../../site/docs/companion-agent.md")),
    ("companions", include_str!("../../../site/docs/companions.md")),
    ("computers", include_str!("../../../site/docs/computers.md")),
    ("configuration", include_str!("../../../site/docs/configuration.md")),
    ("controls", include_str!("../../../site/docs/controls.md")),
    ("hub", include_str!("../../../site/docs/hub.md")),
    ("index", include_str!("../../../site/docs/index.md")),
    ("install", include_str!("../../../site/docs/install.md")),
    ("machines", include_str!("../../../site/docs/machines.md")),
    ("notifications", include_str!("../../../site/docs/notifications.md")),
    ("overview", include_str!("../../../site/docs/overview.md")),
    ("pages", include_str!("../../../site/docs/pages.md")),
    ("quick-start", include_str!("../../../site/docs/quick-start.md")),
    ("security", include_str!("../../../site/docs/security.md")),
    ("sessions", include_str!("../../../site/docs/sessions.md")),
    ("settings", include_str!("../../../site/docs/settings.md")),
    ("setups", include_str!("../../../site/docs/setups.md")),
    ("skills", include_str!("../../../site/docs/skills.md")),
    ("temp-files", include_str!("../../../site/docs/temp-files.md")),
    ("terminal", include_str!("../../../site/docs/terminal.md")),
    ("troubleshooting", include_str!("../../../site/docs/troubleshooting.md")),
    ("windows-app", include_str!("../../../site/docs/windows-app.md")),
];

/// A page's title, description and body, from its front matter.
pub fn parse(text: &str) -> (String, String, &str) {
    let field = |head: &str, k: &str| head.lines().find_map(|l| l.strip_prefix(&format!("{k}: "))).unwrap_or("").trim().to_string();
    match text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")) {
        Some((head, body)) => (field(head, "title"), field(head, "description"), body.trim_start_matches('\n')),
        None => (String::new(), String::new(), text),
    }
}

/// The page a topic names: its name, then a name starting with it, then every word in its title or
/// description ("hub", "add machine", "pages").
pub fn find(topic: &str) -> Option<&'static (&'static str, &'static str)> {
    let t = topic.trim().to_lowercase();
    let t = t.trim_end_matches(".md");
    PAGES.iter().find(|(n, _)| *n == t).or_else(|| PAGES.iter().find(|(n, _)| n.starts_with(t))).or_else(|| {
        let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).collect();
        PAGES.iter().find(|(_, text)| {
            let (title, desc, _) = parse(text);
            let hay = format!("{title} {desc}").to_lowercase();
            !words.is_empty() && words.iter().all(|w| hay.contains(w.trim_end_matches('s')))
        })
    })
}

fn url(name: &str) -> String {
    if name == "index" {
        DOCS_URL.to_string()
    } else {
        format!("{DOCS_URL}{name}/")
    }
}

/// `inyon docs`: every page with what it covers. `inyon docs <topic>`: that page.
pub fn docs_command(topic: Option<&str>) -> Outcome {
    let Some(topic) = topic.filter(|t| !t.trim().is_empty()) else {
        let list: Vec<Value> = PAGES
            .iter()
            .map(|(n, text)| {
                let (title, desc, _) = parse(text);
                json!({ "topic": n, "title": title, "description": desc, "url": url(n) })
            })
            .collect();
        let width = PAGES.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let mut lines = vec![format!("The docs, also at {DOCS_URL}. Read one with: inyon docs <topic>"), String::new()];
        lines.extend(PAGES.iter().map(|(n, text)| {
            let (title, desc, _) = parse(text);
            format!("  {n:<width$}  {title}: {desc}")
        }));
        return Ok(json!({ "pages": list, "message": lines.join("\n") }));
    };
    let Some((name, text)) = find(topic) else {
        return Err(format!("No docs page about \"{topic}\". inyon docs lists them all."));
    };
    let (title, desc, body) = parse(text);
    Ok(
        json!({ "topic": name, "title": title, "description": desc, "url": url(name), "text": body, "message": format!("# {title}\n\n{desc}\n{}\n\n{}", url(name), body.trim_end()) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_docs_page_is_inside_the_program() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/docs");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".md").map(str::to_owned)).collect();
        on_disk.sort();
        assert_eq!(PAGES.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>(), on_disk, "add the new page to PAGES in docs.rs");
        for (n, text) in PAGES {
            let (title, desc, body) = parse(text);
            assert!(!title.is_empty() && !desc.is_empty() && !body.starts_with("---"), "{n}");
        }
    }

    #[test]
    fn a_topic_finds_its_page_by_name_start_or_words() {
        let name = |t: &str| find(t).map(|(n, _)| *n);
        assert_eq!(name("hub"), Some("hub"));
        assert_eq!(name("Pages.md"), Some("pages"));
        assert_eq!(name("trouble"), Some("troubleshooting"));
        assert_eq!(name("add machines"), Some("machines"));
        assert_eq!(name("temp files"), Some("temp-files"));
        assert_eq!(name("zzz"), None);
        let page = docs_command(Some("hub")).unwrap();
        assert_eq!((page["title"].as_str(), page["url"].as_str()), (Some("Running a hub"), Some("https://inyon.tech/docs/hub/")));
        assert!(docs_command(None).unwrap()["message"].as_str().unwrap().contains("  pages "));
    }
}
