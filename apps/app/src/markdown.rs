//! An agent's reply as Markdown (GitHub's flavour, as the web app's react-markdown with remark-gfm
//! reads it), parsed into a small tree the chat draws. Raw HTML in a reply is shown as text, and
//! code keeps every character it was written with, leading spaces included (Ghostex #252).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

#[derive(Debug, Clone, PartialEq)]
pub enum Md {
    Text(String),
    /// inline `code`
    Code(String),
    /// a line break the writer forced
    Break,
    Rule,
    /// a fenced or indented block: its language, and its text with the final newline
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    /// an image: drawn only from this site or a data/blob URL; a web image is a link instead
    Image {
        src: String,
        alt: String,
    },
    Link {
        href: String,
        children: Vec<Md>,
    },
    El {
        tag: El,
        children: Vec<Md>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum El {
    P,
    H(u8),
    Quote,
    Ul,
    Ol(u64),
    Li,
    /// a task list item's box
    Check(bool),
    Em,
    Strong,
    Del,
    Table,
    Head,
    Body,
    Tr,
    Th,
    Td,
}

/// Parses a reply.
pub fn parse(text: &str) -> Vec<Md> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // open elements, each with what it has so far; the bottom one is the document
    let mut stack: Vec<(Option<Frame>, Vec<Md>)> = vec![(None, vec![])];
    let push = |stack: &mut Vec<(Option<Frame>, Vec<Md>)>, n: Md| stack.last_mut().expect("the document").1.push(n);
    for ev in Parser::new_ext(text, opts) {
        match ev {
            Event::Start(tag) => {
                let frame = match tag {
                    Tag::Paragraph => Frame::El(El::P),
                    Tag::Heading { level, .. } => Frame::El(El::H(level_of(level))),
                    Tag::BlockQuote(_) => Frame::El(El::Quote),
                    Tag::List(Some(start)) => Frame::El(El::Ol(start)),
                    Tag::List(None) => Frame::El(El::Ul),
                    Tag::Item => Frame::El(El::Li),
                    Tag::Emphasis => Frame::El(El::Em),
                    Tag::Strong => Frame::El(El::Strong),
                    Tag::Strikethrough => Frame::El(El::Del),
                    Tag::Table(_) => Frame::Table,
                    Tag::TableHead => Frame::El(El::Head),
                    Tag::TableRow => Frame::El(El::Tr),
                    Tag::TableCell => Frame::Cell,
                    Tag::CodeBlock(kind) => Frame::Code(match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().map(str::to_string).filter(|l| !l.is_empty()),
                        CodeBlockKind::Indented => None,
                    }),
                    Tag::Link { dest_url, .. } => Frame::Link(dest_url.to_string()),
                    Tag::Image { dest_url, .. } => Frame::Image(dest_url.to_string()),
                    Tag::HtmlBlock => Frame::Html,
                    // footnotes, definition lists, metadata: their text, unadorned
                    _ => Frame::Plain,
                };
                stack.push((Some(frame), vec![]));
            }
            Event::End(_) => {
                let Some((Some(frame), children)) = (stack.len() > 1).then(|| stack.pop()).flatten() else { continue };
                let node = match frame {
                    Frame::El(El::Head) => {
                        // the header's cells come without a row of their own
                        let tag = El::Head;
                        Some(Md::El { tag, children: vec![Md::El { tag: El::Tr, children }] })
                    }
                    Frame::El(tag) => Some(Md::El { tag, children }),
                    Frame::Cell => {
                        let in_head = stack.last().is_some_and(|(f, _)| matches!(f, Some(Frame::El(El::Head))));
                        Some(Md::El { tag: if in_head { El::Th } else { El::Td }, children })
                    }
                    Frame::Table => {
                        let (head, body): (Vec<Md>, Vec<Md>) = children.into_iter().partition(|c| matches!(c, Md::El { tag: El::Head, .. }));
                        let mut rows = head;
                        if !body.is_empty() {
                            rows.push(Md::El { tag: El::Body, children: body });
                        }
                        Some(Md::El { tag: El::Table, children: rows })
                    }
                    Frame::Code(lang) => Some(Md::CodeBlock { lang, code: text_of(&children) }),
                    Frame::Link(href) => Some(Md::Link { href, children }),
                    Frame::Image(src) => Some(Md::Image { src, alt: text_of(&children) }),
                    // a block of HTML reads as a paragraph of its source
                    Frame::Html => Some(Md::El { tag: El::P, children: vec![Md::Text(text_of(&children).trim_end().to_string())] }),
                    Frame::Plain => {
                        for c in children {
                            push(&mut stack, c);
                        }
                        None
                    }
                };
                if let Some(n) = node {
                    push(&mut stack, n);
                }
            }
            Event::Text(t) => push(&mut stack, Md::Text(t.into_string())),
            Event::Code(t) => push(&mut stack, Md::Code(t.into_string())),
            // HTML is shown as written, never run
            Event::Html(t) | Event::InlineHtml(t) => push(&mut stack, Md::Text(t.into_string())),
            // agents write one line per thought and the terminal shows each on its own line; the
            // paragraph keeps it (white-space: pre-line)
            Event::SoftBreak => push(&mut stack, Md::Text("\n".into())),
            Event::HardBreak => push(&mut stack, Md::Break),
            Event::Rule => push(&mut stack, Md::Rule),
            Event::TaskListMarker(done) => push(&mut stack, Md::El { tag: El::Check(done), children: vec![] }),
            _ => {}
        }
    }
    // anything left open (it can't be, from the parser) still shows
    while stack.len() > 1 {
        let (_, children) = stack.pop().expect("an open element");
        stack.last_mut().expect("the document").1.extend(children);
    }
    merge_text(stack.pop().map(|s| s.1).unwrap_or_default())
}

enum Frame {
    El(El),
    Table,
    Cell,
    Code(Option<String>),
    Link(String),
    Image(String),
    Html,
    Plain,
}

fn level_of(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Neighbouring text runs as one (the parser splits text at every escape and break).
fn merge_text(nodes: Vec<Md>) -> Vec<Md> {
    let mut out: Vec<Md> = Vec::with_capacity(nodes.len());
    for n in nodes {
        let n = match n {
            Md::El { tag, children } => Md::El { tag, children: merge_text(children) },
            Md::Link { href, children } => Md::Link { href, children: merge_text(children) },
            other => other,
        };
        match (out.last_mut(), n) {
            (Some(Md::Text(a)), Md::Text(b)) => a.push_str(&b),
            (_, n) => out.push(n),
        }
    }
    out
}

/// All the text under some nodes.
pub fn text_of(nodes: &[Md]) -> String {
    let mut s = String::new();
    for n in nodes {
        match n {
            Md::Text(t) | Md::Code(t) => s.push_str(t),
            Md::CodeBlock { code, .. } => s.push_str(code),
            Md::Break => s.push('\n'),
            Md::El { children, .. } | Md::Link { children, .. } => s.push_str(&text_of(children)),
            Md::Image { alt, .. } => s.push_str(alt),
            Md::Rule => {}
        }
    }
    s
}

/// What an image in a reply becomes. Agent output can't load outside images: a prompt-injected
/// agent could put data in the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picture {
    /// this site's own (a path, not `//host/…`), or a data or blob URL
    Show,
    /// a web address: a link to it
    Link,
    /// anything else: its alt text
    Alt,
}

pub fn picture(src: &str) -> Picture {
    let own = src.starts_with('/') && !src.starts_with("//") && !src.starts_with("/\\");
    if own || src.starts_with("data:image/") || src.starts_with("blob:") {
        Picture::Show
    } else if src.starts_with("http://") || src.starts_with("https://") {
        Picture::Link
    } else {
        Picture::Alt
    }
}

/// Where a link may go: web and mail addresses and this site's paths; a `javascript:` link
/// (or anything else) has no target.
pub fn safe_href(href: &str) -> Option<&str> {
    let lower = href.trim_start().to_ascii_lowercase();
    let ok = ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p)) || href.starts_with('/') || href.starts_with('#');
    ok.then_some(href)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_blocks(nodes: &[Md]) -> Vec<(Option<String>, String)> {
        let mut out = vec![];
        for n in nodes {
            match n {
                Md::CodeBlock { lang, code } => out.push((lang.clone(), code.clone())),
                Md::El { children, .. } | Md::Link { children, .. } => out.extend(code_blocks(children)),
                _ => {}
            }
        }
        out
    }

    // Ghostex #252: the chat stripped the leading spaces of code. Indented Python, YAML and a diff
    // come through byte for byte.
    #[test]
    fn code_keeps_every_leading_space() {
        let python = "def f(x):\n    if x:\n        return [\n            1,\n        ]\n    return None\n";
        let yaml = "jobs:\n  test:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: bun test\n";
        let diff = "@@ -1,3 +1,3 @@\n def f():\n-    return 1\n+    return 2\n \n";
        let reply = format!("Here:\n\n```python\n{python}```\n\nand the workflow\n\n```yaml\n{yaml}```\n\n```diff\n{diff}```\n");
        let blocks = code_blocks(&parse(&reply));
        assert_eq!(blocks, vec![(Some("python".into()), python.into()), (Some("yaml".into()), yaml.into()), (Some("diff".into()), diff.into())]);
        // an indented block (four spaces) keeps what's past them
        let indented = code_blocks(&parse("Run:\n\n        nested\n    flat\n"));
        assert_eq!(indented, vec![(None, "    nested\nflat\n".into())]);
        // inside a list item too
        let listed = code_blocks(&parse("1. Edit it:\n   ```js\n   if (a) {\n     b()\n   }\n   ```\n"));
        assert_eq!(listed, vec![(Some("js".into()), "if (a) {\n  b()\n}\n".into())]);
    }

    #[test]
    fn replies_keep_their_lines_and_markup() {
        let md = parse("**Done.** Ran `bun test`\nall green\n\n- one\n- [x] two");
        assert_eq!(
            md[0],
            Md::El {
                tag: El::P,
                children: vec![
                    Md::El { tag: El::Strong, children: vec![Md::Text("Done.".into())] },
                    Md::Text(" Ran ".into()),
                    Md::Code("bun test".into()),
                    Md::Text("\nall green".into())
                ]
            }
        );
        let Md::El { tag: El::Ul, children: items } = &md[1] else { panic!("{:?}", md[1]) };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], Md::El { children, .. } if children.contains(&Md::El { tag: El::Check(true), children: vec![] })));
    }

    #[test]
    fn tables_have_a_head_and_a_body() {
        let md = parse("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let Md::El { tag: El::Table, children } = &md[0] else { panic!("{md:?}") };
        assert!(
            matches!(&children[0], Md::El { tag: El::Head, children } if matches!(&children[0], Md::El { tag: El::Tr, children } if matches!(children[0], Md::El { tag: El::Th, .. })))
        );
        assert!(
            matches!(&children[1], Md::El { tag: El::Body, children } if matches!(&children[0], Md::El { tag: El::Tr, children } if matches!(children[1], Md::El { tag: El::Td, .. })))
        );
    }

    #[test]
    fn html_is_text_and_outside_images_never_load() {
        let md = parse("<img src=x onerror=alert(1)>\n\n```html\n<b>hi</b>\n```");
        assert_eq!(text_of(&md[..1]), "<img src=x onerror=alert(1)>");
        assert_eq!(code_blocks(&md), vec![(Some("html".into()), "<b>hi</b>\n".into())]);
        assert_eq!(picture("/next/assets/a.png"), Picture::Show);
        assert_eq!(picture("data:image/png;base64,AAA"), Picture::Show);
        // "//host/x" is another site: not drawn, and not a web link either
        assert_eq!(picture("//evil.example/x.png"), Picture::Alt);
        assert_eq!(picture("https://evil.example/x.png?d=secret"), Picture::Link);
        assert_eq!(picture("file:///etc/passwd"), Picture::Alt);
        assert_eq!(safe_href("javascript:alert(1)"), None);
        assert_eq!(safe_href(" JavaScript:alert(1)"), None);
        assert_eq!(safe_href("https://inyon.tech"), Some("https://inyon.tech"));
    }
}
