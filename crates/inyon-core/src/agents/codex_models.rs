//! Port of agents/codex-models.ts: the models Codex offers, from its own cache.

use std::cell::RefCell;
use std::collections::HashMap;

use inyon_protocol::{agents, AgentCatalog, AgentKind, AgentModel, CatalogSource};

use crate::clock::now_ms;
use crate::hosts::host::Host;
use crate::js::json::{self, value_string, Js};
use crate::js::JsStr;

/// What `JSON.parse` throws for text that isn't JSON; the TS shows the engine's own message.
pub const NOT_JSON: &str = "JSON Parse error: Unable to parse JSON string";

/// Codex caches the account's model list in ~/.codex/models_cache.json (what /model shows): the
/// "list" models, each with its reasoning levels and context window. Hidden ones stay hidden.
pub fn parse_codex_models(text: &str) -> Result<Vec<AgentModel>, String> {
    let data = json::parse(text).ok_or(NOT_JSON)?;
    let rows = Some(&data).at("models").arr().map(Vec::as_slice).unwrap_or_default();
    Ok(rows
        .iter()
        .map(Some)
        .filter(|m| m.at("slug").truthy() && m.at("visibility").filter(|v| !v.is_null()).is_none_or(|v| v.as_str() == Some("list")))
        .map(|m| AgentModel {
            id: value_string(m.at("slug").unwrap()),
            label: value_string(m.at("display_name").filter(|v| !v.is_null()).or(m.at("slug")).unwrap()),
            note: m.at("description").str().map(|d| d.strip_suffix('.').unwrap_or(d).to_owned()),
            context: m.at("context_window").num().map(Into::into),
            efforts: m.at("supported_reasoning_levels").arr().map(|ls| {
                ls.iter()
                    .map(|l| {
                        let e = Some(l).at("effort").filter(|v| !v.is_null()).unwrap_or(l);
                        value_string(e)
                    })
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
            default_effort: m.at("default_reasoning_level").str().map(str::to_owned),
            variants: None,
        })
        .collect())
}

thread_local! {
    static CACHE: RefCell<HashMap<String, (u64, AgentCatalog)>> = RefCell::new(HashMap::new());
}

fn fallback(error: Option<String>) -> AgentCatalog {
    AgentCatalog { models: agents()[&AgentKind::Codex].models.clone(), source: CatalogSource::Fallback, updated_at: 0, error: error.filter(|e| !e.is_empty()), modes: None }
}

/// `codexModels(host)`: kept for a minute.
pub async fn codex_models(host: &Host) -> AgentCatalog {
    let id = host.id();
    if let Some((_, v)) = CACHE.with(|c| c.borrow().get(&id).filter(|(at, _)| now_ms().saturating_sub(*at) < 60_000).cloned()) {
        return v;
    }
    let value = match host.run(r#"cat "${CODEX_HOME:-$HOME/.codex}/models_cache.json" 2>/dev/null"#).await {
        Err(e) => fallback(Some(e.message)),
        Ok(r) => match if r.stdout.jtrim().is_empty() { Ok(vec![]) } else { parse_codex_models(&r.stdout) } {
            Err(e) => fallback(Some(e)),
            Ok(models) if models.is_empty() => fallback(Some("Codex has not cached its model list yet".into())),
            Ok(models) => AgentCatalog { models, source: CatalogSource::Cli, updated_at: now_ms(), error: None, modes: None },
        },
    };
    CACHE.with(|c| c.borrow_mut().insert(id, (now_ms(), value.clone())));
    value
}
