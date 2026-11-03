//! Port of agents/antigravity-models.ts: the models the signed-in Antigravity CLI offers.

use std::cell::RefCell;
use std::collections::HashMap;

use indexmap::IndexMap;
use inyon_protocol::{agents, AgentCatalog, AgentKind, AgentModel, CatalogSource};

use crate::clock::now_ms;
use crate::hosts::host::{ExecOptions, Host};
use crate::js::JsStr;
use crate::platform::rt::{spawn_task, Deferred};
use crate::re;

const ORDER: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

/// `agy models` queries the signed-in CLI's catalog and prints id<TAB>display name.
pub fn parse_agy_models(output: &str) -> Vec<AgentModel> {
    let mut families: IndexMap<String, AgentModel> = IndexMap::new();
    let plain = re!(r"\x1b\[[0-9;]*m", "g").replace_all(output, "");
    for line in plain.split('\n') {
        let Some(row) = re!(r"^([\w.-]+)\t(.+)$").captures(line) else { continue };
        let (variant, display) = (&row[1], &row[2]);
        let effort = re!(r"-(low|medium|high|xhigh|max|ultra)$").captures(variant).map(|m| m[1].to_owned());
        let gemini = variant.starts_with("gemini-");
        // A fixed-medium model is still selectable by its full id.
        let id = match &effort {
            Some(e) if gemini => variant[..variant.len() - e.len() - 1].to_owned(),
            _ => variant.to_owned(),
        };
        let model = families.entry(id.clone()).or_insert_with(|| {
            let known = agents()[&AgentKind::Antigravity].models.iter().find(|m| m.id == id);
            AgentModel {
                id: id.clone(),
                label: re!(r"\s*\((low|medium|high|xhigh|max|ultra)\)$", "i").replace(display, "").jtrim().to_owned(),
                note: None,
                context: known.and_then(|k| k.context),
                efforts: Some(vec![]),
                default_effort: None,
                variants: Some(IndexMap::new()),
            }
        });
        if let (Some(e), true) = (effort, gemini) {
            let efforts = model.efforts.get_or_insert_with(Vec::new);
            if !efforts.contains(&e) {
                efforts.push(e.clone());
            }
            model.variants.get_or_insert_with(IndexMap::new).insert(e, variant.to_owned());
        }
    }
    families
        .into_values()
        .map(|mut m| {
            if let Some(e) = m.efforts.as_mut() {
                e.sort_by_key(|x| ORDER.iter().position(|o| o == x));
            }
            m
        })
        .collect()
}

#[derive(Default)]
struct Entry {
    at: u64,
    value: Option<AgentCatalog>,
    pending: Option<Deferred<AgentCatalog>>,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
}

/// `agyModels(host)`: one request at a time per machine, kept for a minute; a failure keeps the
/// models it had and says why.
pub async fn agy_models(host: &Host) -> AgentCatalog {
    let id = host.id();
    let now = now_ms();
    let (pending, fresh, previous) = CACHE.with(|c| {
        let c = c.borrow();
        let e = c.get(&id);
        (e.and_then(|e| e.pending.clone()), e.and_then(|e| e.value.clone().filter(|_| now.saturating_sub(e.at) < 60_000)), e.and_then(|e| e.value.clone()))
    });
    if let Some(p) = pending {
        return p.wait().await;
    }
    if let Some(v) = fresh {
        return v;
    }
    let h = host.clone();
    let task = spawn_task(async move {
        let run = async {
            let r = h
                .exec(r#"if command -v agy >/dev/null 2>&1; then agy models; else "$HOME/.local/bin/agy" models; fi"#, ExecOptions::timeout(20_000))
                .await
                .map_err(|e| e.message)?;
            if r.code != 0 {
                let e = r.stderr.jtrim();
                return Err(if e.is_empty() { "Antigravity model discovery failed".to_owned() } else { e.to_owned() });
            }
            let models = parse_agy_models(&r.stdout);
            if models.is_empty() {
                return Err("Antigravity returned no model catalog".to_owned());
            }
            Ok(AgentCatalog { models, source: CatalogSource::Cli, updated_at: now_ms(), error: None, modes: None })
        };
        match run.await {
            Ok(c) => c,
            Err(error) => AgentCatalog {
                models: previous
                    .as_ref()
                    .map(|p| p.models.clone())
                    .unwrap_or_else(|| agents()[&AgentKind::Antigravity].models.iter().map(|m| AgentModel { efforts: Some(vec![]), ..m.clone() }).collect()),
                source: CatalogSource::Fallback,
                updated_at: previous.as_ref().map_or(0, |p| p.updated_at),
                error: Some(error),
                modes: None,
            },
        }
    });
    CACHE.with(|c| c.borrow_mut().insert(id.clone(), Entry { at: now, value: None, pending: Some(task.clone()) }));
    let value = task.wait().await;
    CACHE.with(|c| {
        if let Some(e) = c.borrow_mut().get_mut(&id) {
            if e.pending.as_ref().is_some_and(|p| p.same(&task)) {
                e.value = Some(value.clone());
                e.pending = None;
            }
        }
    });
    value
}

#[cfg(test)]
#[path = "antigravity_models_tests.rs"]
mod tests;
