//! Port of agents/copilot-models.ts: the models the signed-in Copilot CLI offers, asked through its
//! official SDK transport on the machine (read-only: it starts no conversation and submits no
//! inference, and the CLI's credentials stay there).

use std::cell::RefCell;
use std::collections::HashMap;

use inyon_protocol::{AgentCatalog, AgentModel, CatalogSource};
use serde_json::Value;

use super::codex_models::NOT_JSON;
use super::copilot_rpc_sh::COPILOT_CATALOG_SCRIPT;
use crate::clock::now_ms;
use crate::features::setup::PATHS;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::js::json::{self, Js};
use crate::js::JsStr;
use crate::platform::rt::{spawn_task, Deferred};

pub fn parse_copilot_models(payload: &Value) -> Result<Vec<AgentModel>, String> {
    let rows = match payload {
        Value::Array(a) => a,
        p => Some(p).at("models").arr().ok_or("Invalid Copilot model catalog")?,
    };
    let mut models = vec![AgentModel {
        id: "auto".into(),
        label: "Auto".into(),
        note: Some("Copilot chooses an available model for each task".into()),
        context: None,
        efforts: Some(vec![]),
        default_effort: None,
        variants: None,
    }];
    for m in rows.iter().map(Some) {
        let Some(id) = m.at("id").str() else { continue };
        if !m.truthy() || id == "auto" || m.at("policy").at("state").str() == Some("disabled") || m.at("model_picker_enabled") == Some(&Value::Bool(false)) {
            continue;
        }
        let context = m.at("capabilities").at("limits").at("max_context_window_tokens").filter(|v| Some(*v).truthy()).and_then(Value::as_f64);
        models.push(AgentModel {
            id: id.into(),
            label: m.at("name").nonempty().unwrap_or(id).into(),
            note: None,
            context: context.map(Into::into),
            efforts: Some(m.at("supportedReasoningEfforts").arr().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_owned)).collect()).unwrap_or_default()),
            default_effort: None,
            variants: None,
        });
    }
    Ok(models)
}

thread_local! {
    static CACHE: RefCell<HashMap<String, (u64, Deferred<AgentCatalog>)>> = RefCell::new(HashMap::new());
}

/// `copilotModels(host)`: kept for a minute.
pub async fn copilot_models(host: &Host) -> AgentCatalog {
    let id = host.id();
    if let Some((_, v)) = CACHE.with(|c| c.borrow().get(&id).filter(|(at, _)| now_ms().saturating_sub(*at) < 60_000).cloned()) {
        return v.wait().await;
    }
    let h = host.clone();
    let value = spawn_task(async move {
        let run = async {
            // node and copilot are often only on PATH in a login shell (nvm, ~/.local/bin)
            let r = h.exec(&format!("{PATHS}; node -e {}", shq(COPILOT_CATALOG_SCRIPT)), ExecOptions::timeout(22_000)).await.map_err(|e| e.message)?;
            if r.code != 0 {
                let e = r.stderr.jtrim();
                return Err(if e.is_empty() { "Copilot catalog unavailable".to_owned() } else { e.to_owned() });
            }
            let payload = json::parse(&r.stdout).ok_or(NOT_JSON)?;
            parse_copilot_models(&payload)
        };
        match run.await {
            Ok(models) => AgentCatalog { models, source: CatalogSource::Cli, updated_at: now_ms(), error: None, modes: None },
            Err(error) => AgentCatalog {
                models: parse_copilot_models(&Value::Array(vec![])).unwrap_or_default(),
                source: CatalogSource::Fallback,
                updated_at: 0,
                error: Some(error),
                modes: None,
            },
        }
    });
    CACHE.with(|c| c.borrow_mut().insert(id, (now_ms(), value.clone())));
    value.wait().await
}
