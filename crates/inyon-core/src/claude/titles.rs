//! Read a conversation's persisted name even when nobody has opened its Chat.
use crate::hosts::host::shq;

#[derive(Clone)]
pub struct CachedTitle {
    pub path: String,
    pub session: String,
    pub title: Option<String>,
    pub at: u64,
}

pub fn command(path: &str) -> String {
    // Claude repeats title metadata as it saves. Bound the read and transfer for large chats.
    let p = shq(path);
    format!(r#"{{ head -c 262144 {p} 2>/dev/null; printf '\n'; tail -c 262144 {p} 2>/dev/null; }} | grep -E '^\{{"type":"(custom-title|ai-title)"' | tail -n 12 | cut -c1-2400"#)
}

pub fn parse(text: &str, session: &str) -> Option<String> {
    let (mut custom, mut generated) = (None, None);
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if e.get("sessionId").and_then(|x| x.as_str()).is_some_and(|id| id != session) { continue; }
        let (slot, key) = match e.get("type").and_then(|x| x.as_str()) {
            Some("custom-title") => (&mut custom, "customTitle"),
            Some("ai-title") => (&mut generated, "aiTitle"),
            _ => continue,
        };
        *slot = e.get(key).and_then(|x| x.as_str()).map(crate::features::conversations::one_line).filter(|t| !t.is_empty());
    }
    custom.or(generated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_current_conversation_metadata_names_the_session() {
        let text = concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Explain training data\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"My notes\",\"sessionId\":\"current\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"New generated title\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"Other pane\",\"sessionId\":\"other\"}\n",
            "{\"type\":\"assistant\",\"customTitle\":\"Quoted title\"}\n");
        assert_eq!(parse(text, "current").as_deref(), Some("My notes"));
        assert_eq!(parse(&(text.to_owned()+"{\"type\":\"custom-title\",\"customTitle\":\"\"}\n"), "current").as_deref(), Some("New generated title"));
        assert_eq!(parse("", "current"), None);
    }
}
