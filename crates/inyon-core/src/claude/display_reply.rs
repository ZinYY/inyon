//! Exact Windows Claude text from the official MessageDisplay hook. The native Windows
//! renderer may buffer its whole answer even while this hook delivers line batches.
use inyon_protocol::{AssistantBlock, ChatItem, LiveItem, LiveKind};
use inyon_protocol::claude_stream::{DisplaySnapshot, MAX_TEXT};
use crate::js::date;

pub(super) fn replies_in_turn(items: &[ChatItem]) -> Vec<&str> {
    items.iter().rev().take_while(|i| !matches!(i, ChatItem::User { .. })).filter_map(|i| match i {
        ChatItem::Assistant { block: AssistantBlock::Text { text }, .. } => Some(text.as_str()),
        _ => None,
    }).collect()
}

pub(super) fn pending_text(text: &str, items: &[ChatItem]) -> String {
    // Unlike a terminal capture, hook text is exact Markdown. Do not apply the screen
    // reader's approximate paragraph matching to different messages with a common opening.
    if replies_in_turn(items).iter().any(|r| r.trim_end().starts_with(text.trim_end())) { String::new() }
    else { text.to_owned() }
}

pub(super) fn preview(raw: &str, session: &str, started: u64, items: &[ChatItem], now: u64) -> Option<LiveItem> {
    if raw.len() > MAX_TEXT * 2 { return None; }
    let s: DisplaySnapshot = serde_json::from_str(raw).ok()?;
    let prompt_at = items.iter().rev().find_map(|i| match i { ChatItem::User { ts, .. } => Some(date::parse(ts)), _ => None })?;
    if !s.valid || s.session_id != session || s.text.len() > MAX_TEXT || !prompt_at.is_finite()
        || s.started_at < started || (s.started_at as f64) < prompt_at
        || now.saturating_sub(s.updated_at) > 15_000 || s.updated_at > now.saturating_add(5_000) { return None; }
    let text = pending_text(&s.text, items);
    (!text.is_empty()).then(|| LiveItem { id: format!("display:{}", s.message_id), kind: LiveKind::Text, text, turn_id: Some(s.turn_id) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_protocol::claude_stream::{DisplayEvent, DisplaySnapshot};
    const ID: &str = "11111111-1111-4111-8111-111111111111";
    fn user(ts: &str) -> ChatItem { ChatItem::User { seq:1, uuid:"u".into(), ts:ts.into(), text:"你好".into(), images:0, queued:None, shell:None } }
    #[test]
    fn exact_id_current_turn_and_freshness_are_required_and_final_record_deduplicates() {
        let now = 1_793_700_000_000;
        let s = DisplaySnapshot::apply(None, DisplayEvent { hook_event_name:"MessageDisplay".into(), session_id:ID.into(), turn_id:ID.into(), message_id:ID.into(), index:0, delta:"你好 🎸".into(), finished:false }, now).unwrap();
        let raw=serde_json::to_string(&s).unwrap();
        let mut items=vec![user(&date::to_iso((now-1000) as f64).unwrap())];
        assert_eq!(preview(&raw,ID,now-2000,&items,now).unwrap().text,"你好 🎸");
        assert!(preview(&raw,"22222222-2222-4222-8222-222222222222",now-2000,&items,now).is_none());
        assert!(preview(&raw,ID,now+1,&items,now).is_none());
        assert!(preview(&raw,ID,now-2000,&items,now+16_000).is_none());
        assert!(preview(&raw,ID,now-2000,&[user(&date::to_iso((now+1) as f64).unwrap())],now).is_none());
        items.push(ChatItem::Assistant { seq:2, uuid:"a".into(), ts:date::to_iso(now as f64).unwrap(), msg_id:"api-id".into(), model:None, block:AssistantBlock::Text{text:"你好 🎸".into()} });
        assert!(preview(&raw,ID,now-2000,&items,now).is_none());
        items.push(user(&date::to_iso((now-500) as f64).unwrap()));
        assert!(preview(&raw,ID,now-2000,&items,now).is_some(),"an identical reply to another prompt is not a duplicate");
    }
}
