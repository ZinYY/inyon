//! A page's notes: the Markdown files in its site's `research/` folder, which the viewer's Notes
//! drawer opens at `/v/<token>/research/<name>.md?as=note`. The file is found and checked exactly
//! as any other file of the page (the token, dot names, symlinks out of the site, other machines);
//! only the answer differs: the Markdown as a page to read, in the app's light or dark, under the
//! same sandbox. Nothing in a note runs: HTML written in it is shown as text, headings take no
//! attributes, and a link or image may only lead to http(s), mailto, an anchor or a path in the site.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

/// The query that asks for a note as a page.
pub const AS_NOTE: &str = "as=note";
/// The biggest note shown as a page; a bigger one is sent as it is.
pub const NOTE_MAX: usize = 1024 * 1024;

/// Whether a `/v/` request asks for the Markdown file `path` as a note.
pub fn wants_note(query: &str, path: &std::path::Path) -> bool {
    query.split('&').any(|p| p == AS_NOTE) && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Whether a link or image may keep its address: one in the site (a path or an anchor), or
/// http(s) or mailto. Anything else (`javascript:`, `data:`, `vbscript:`…) is dropped. Browsers
/// ignore tabs, newlines and leading spaces in an address, so those don't count when judging it.
fn safe_dest(dest: &str) -> bool {
    let d: String = dest.chars().filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control()).collect::<String>().to_ascii_lowercase();
    let scheme_end = d.find([':', '/', '?', '#']);
    match scheme_end {
        Some(i) if d.as_bytes()[i] == b':' => ["http:", "https:", "mailto:"].iter().any(|s| d.starts_with(s)),
        _ => true,
    }
}

/// A relative link to another Markdown file stays a note when followed.
fn note_link(dest: CowStr<'_>) -> CowStr<'_> {
    if !safe_dest(&dest) {
        return CowStr::Borrowed("");
    }
    let relative = !dest.contains(':') && !dest.starts_with('/') && !dest.starts_with('#');
    let (path, frag) = dest.split_once('#').map_or((&*dest, None), |(p, f)| (p, Some(f)));
    if !relative || path.contains('?') || !path.to_ascii_lowercase().ends_with(".md") {
        return dest;
    }
    let mut out = format!("{path}?{AS_NOTE}");
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out.into()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The note's Markdown as HTML: tables, task lists and strikethrough, raw HTML as text.
pub fn render(md: &str) -> String {
    // no ENABLE_HEADING_ATTRIBUTES: `# x {onclick=…}` would put a handler on the heading
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let events = Parser::new_ext(md, opts).map(|e| match e {
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => Event::Start(Tag::Link { link_type, dest_url: note_link(dest_url), title, id }),
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            Event::Start(Tag::Image { link_type, dest_url: if safe_dest(&dest_url) { dest_url } else { CowStr::Borrowed("") }, title, id })
        }
        e => e,
    });
    let mut out = String::with_capacity(md.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

/// The note as a whole page. The theme comes as `?theme=` in the address and as a message from the
/// app when it changes, as for any page; without either it follows the system.
pub fn page(name: &str, md: &str) -> String {
    let title = escape(name.trim_end_matches(".md").trim_end_matches(".MD"));
    let body = render(md);
    format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>{STYLE}</style><script>{SCRIPT}</script><main>{body}</main></html>\n")
}

const STYLE: &str = ":root{color-scheme:light;--bg:#f4f3f0;--fg:#1d1c1a;--fg2:#4a4844;--fg3:#5f5d58;--line:#d6d5cf;--raised:#ecebe7}\
:root[data-theme=dark]{color-scheme:dark;--bg:#1b1a18;--fg:#f1f0ec;--fg2:#b4b2ab;--fg3:#959389;--line:#33312e;--raised:#252422}\
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){color-scheme:dark;--bg:#1b1a18;--fg:#f1f0ec;--fg2:#b4b2ab;--fg3:#959389;--line:#33312e;--raised:#252422}}\
*{box-sizing:border-box}html{background:var(--bg);color:var(--fg);-webkit-text-size-adjust:100%}\
body{margin:0;font:16px/1.65 system-ui,-apple-system,'Segoe UI',sans-serif}\
main{max-width:46rem;margin:0 auto;padding:28px 16px 64px;overflow-wrap:anywhere}\
h1,h2,h3,h4{line-height:1.2;letter-spacing:-.02em;margin:1.8em 0 .5em}h1{font-size:1.75rem;letter-spacing:-.03em;margin-top:0}h2{font-size:1.3rem}h3{font-size:1.08rem}h4{font-size:1rem}\
p,ul,ol,table,pre,blockquote{margin:0 0 1em}ul,ol{padding-left:1.4em}li{margin:.2em 0}li>p{margin:0}\
a{color:inherit;text-decoration:underline;text-decoration-color:var(--fg3);text-underline-offset:3px}\
code{font:.88em/1.5 ui-monospace,'SF Mono',Menlo,Consolas,monospace;background:var(--raised);border:1px solid var(--line);border-radius:6px;padding:.1em .35em}\
pre{background:var(--raised);border:1px solid var(--line);border-radius:12px;padding:12px 14px;overflow-x:auto}pre code{background:none;border:0;padding:0;font-size:13px}\
blockquote{margin-left:0;padding-left:14px;border-left:2px solid var(--line);color:var(--fg2)}\
hr{border:0;border-top:1px solid var(--line);margin:2em 0}img{max-width:100%}\
table{display:block;overflow-x:auto;border-collapse:collapse;font-size:14px}th,td{border-bottom:1px solid var(--line);padding:6px 12px 6px 0;text-align:left;vertical-align:top}th{color:var(--fg2);font-weight:600}\
li:has(>input[type=checkbox]){list-style:none;margin-left:-1.4em}input[type=checkbox]{margin:0 .5em 0 0;vertical-align:-1px}";

const SCRIPT: &str = "(function(){var d=document.documentElement;function set(t){if(t==='light'||t==='dark')d.dataset.theme=t}\
set(new URLSearchParams(location.search).get('theme'));\
addEventListener('message',function(e){if(e.source===parent&&e.data&&e.data.type==='theme')set(e.data.value)});\
if(parent!==window)parent.postMessage({type:'theme?'},'*')})()";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn only_markdown_asked_for_as_a_note_is_one() {
        assert!(wants_note("as=note&theme=dark", Path::new("/s/research/plan.md")));
        assert!(wants_note("theme=light&as=note", Path::new("/s/research/Plan.MD")));
        assert!(!wants_note("theme=dark", Path::new("/s/research/plan.md")));
        assert!(!wants_note("as=note", Path::new("/s/index.html")));
        assert!(!wants_note("as=notes", Path::new("/s/research/plan.md")));
    }

    #[test]
    fn html_in_a_note_is_text_and_links_to_notes_stay_notes() {
        let h = render("# Plan\n\n<script>alert(1)</script>\n\nSee [the internals](internals.md#seams), [a site](https://x.dev/a.md), [top](#plan) and [a page](../index.html).\n\n- [x] done\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(!h.contains("<script>"), "{h}");
        assert!(h.contains("&lt;script&gt;"), "{h}");
        assert!(h.contains("href=\"internals.md?as=note#seams\""), "{h}");
        assert!(h.contains("href=\"https://x.dev/a.md\""), "{h}");
        assert!(h.contains("href=\"#plan\""), "{h}");
        assert!(h.contains("href=\"../index.html\""), "{h}");
        assert!(h.contains("<table>") && h.contains("checkbox"), "{h}");
    }

    // a note could put script on the page through a heading's attributes or a link's address
    #[test]
    fn nothing_in_a_note_can_run() {
        let h = render("# Read this {onmouseover=alert(1)}\n\n[a](javascript:alert(1)) [b](JavaScript:alert(1)) [c](<java\tscript:alert(1)>) <javascript:alert(1)> [d](vbscript:x) [e](data:text/html,x) ![i](javascript:alert(1)) [ok](https://x.dev) [mail](mailto:a@b.c) [rel](plan.md) [path](a/b:c.html)\n");
        // the attribute stays text in the heading
        assert!(h.contains("<h1>Read this {onmouseover=alert(1)}</h1>"), "{h}");
        for bad in ["javascript:", "JavaScript:", "vbscript:", "data:text"] {
            assert!(!h.contains(&format!("href=\"{bad}")) && !h.contains(&format!("src=\"{bad}")), "{bad} in {h}");
        }
        assert!(!h.contains("java\tscript") && !h.contains("java%09script"), "{h}");
        for good in ["href=\"https://x.dev\"", "href=\"mailto:a@b.c\"", "href=\"plan.md?as=note\"", "href=\"a/b:c.html\""] {
            assert!(h.contains(good), "{good} missing in {h}");
        }
    }

    #[test]
    fn the_page_is_named_after_the_file_and_escapes_it() {
        let p = page("a<b>.md", "text");
        assert!(p.contains("<title>a&lt;b&gt;</title>"), "{p}");
        assert!(p.contains("<main><p>text</p>\n</main>"), "{p}");
    }
}
