//! Port of features/models.ts: the models the app's picker offers for the agent in a pane, asked
//! of the agent on its machine where it can say, else the catalog's own list.

use inyon_protocol::{AgentCatalog, AgentKind, CatalogSource};

use crate::agents::antigravity_models::agy_models;
use crate::agents::codex_models::codex_models;
use crate::agents::copilot_models::copilot_models;
use crate::agents::opencode_api::opencode_catalog;
use crate::agents::opencode_models::opencode_cli_models;
use crate::hosts::http_error::{http, HubResult};
use crate::hosts::registry::registry;

pub async fn list_models(host_id: &str, pane_id: &str) -> HubResult<AgentCatalog> {
    let found = registry().find_pane(host_id, pane_id);
    let Some((agent, cwd)) = found.and_then(|f| f.pane.agent.map(|a| (a, f.pane.cwd))) else {
        return Err(http(404, "No agent is running in this pane"));
    };
    match agent.kind {
        AgentKind::Antigravity => Ok(agy_models(&registry().host(host_id)?).await),
        AgentKind::Opencode => {
            let host = registry().host(host_id)?;
            match agent.transcript_path.filter(|t| t.starts_with("opencode://")) {
                // started without --port: no API, so ask the CLI
                None => Ok(opencode_cli_models(&host).wait().await),
                Some(uri) => opencode_catalog(&host, &uri, Some(&cwd)).await,
            }
        }
        AgentKind::Copilot => Ok(copilot_models(&registry().host(host_id)?).await),
        AgentKind::Codex => Ok(codex_models(&registry().host(host_id)?).await),
        kind => Ok(AgentCatalog { models: kind.def().models.clone(), source: CatalogSource::Fallback, updated_at: 0, error: None, modes: None }),
    }
}
