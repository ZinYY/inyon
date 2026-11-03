//! Port of agents/opencode-models.ts: the models and agents `opencode` lists, and matching the
//! TUI footer's "Model Name Provider Name" to them, and asking a machine for them (kept 10 minutes).

use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use indexmap::IndexMap;
use inyon_protocol::{AgentCatalog, AgentModel, CatalogSource, ModeOption};
use serde_json::Value;

use std::cell::RefCell;
use std::collections::HashMap;

use super::opencode_db::OC_PATH;
use crate::clock::now_ms;
use crate::hosts::host::{ExecOptions, Host};
use crate::js::json::{self, Js};
use crate::js::re::{group, replace_all, split};
use crate::js::text::{capitalize, lower};
use crate::js::JsStr;
use crate::platform::rt::{spawn_task, Deferred};
use crate::re;

/// Provider names as OpenCode's footer prints them after the model ("Big Pickle OpenCode"), for
/// the providers people connect most. A machine's own list (from `opencode auth login`'s catalogue
/// or a session's API) adds the rest as it's seen.
const PROVIDER_NAMES: &[(&str, &str)] = &[
    ("opencode", "OpenCode"),
    ("opencode-go", "OpenCode Go"),
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("github-copilot", "GitHub Copilot"),
    ("google", "Google"),
    ("google-vertex", "Vertex"),
    ("openrouter", "OpenRouter"),
    ("vercel", "Vercel AI Gateway"),
    ("xai", "xAI"),
    ("groq", "Groq"),
    ("deepseek", "DeepSeek"),
    ("mistral", "Mistral"),
    ("amazon-bedrock", "Amazon Bedrock"),
    ("azure", "Azure"),
    ("ollama-cloud", "Ollama Cloud"),
    ("lmstudio", "LMStudio"),
    ("zai", "Z.AI"),
    ("zai-coding-plan", "Z.AI Coding Plan"),
    ("moonshotai", "Moonshot AI"),
    ("kimi-code-plan-global", "Kimi For Coding (kimi.ai)"),
    ("minimax", "MiniMax (minimax.io)"),
    ("cerebras", "Cerebras"),
    ("togetherai", "Together AI"),
    ("fireworks-ai", "Fireworks AI"),
    ("huggingface", "Hugging Face"),
    ("llama", "Llama"),
    ("cohere", "Cohere"),
    ("perplexity", "Perplexity"),
    ("nvidia", "Nvidia"),
    ("poe", "Poe"),
    ("gitlab", "GitLab Duo"),
    ("digitalocean", "DigitalOcean"),
];

/// names older OpenCode versions printed, kept so their footers still read
const OLD_NAMES: &[&str] = &["OpenCode Zen"];

/// Provider names machines reported, for the whole process as the TS module's Map is: a hub
/// running readers on several threads learns a name once for all of them.
static LEARNED: LazyLock<Mutex<IndexMap<String, String>>> = LazyLock::new(|| Mutex::new(IndexMap::new()));

fn learned() -> MutexGuard<'static, IndexMap<String, String>> {
    // a panic while holding the lock leaves the map whole (inserts only), so keep using it
    LEARNED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Remembers provider names a machine reported, so footers naming them read right.
pub fn note_provider_names<'a>(names: impl IntoIterator<Item = (&'a str, &'a str)>) {
    let mut l = learned();
    for (id, name) in names {
        if !id.is_empty() && !name.is_empty() {
            l.insert(id.to_owned(), name.to_owned());
        }
    }
}

pub fn provider_name(id: &str) -> String {
    if let Some(n) = learned().get(id).cloned() {
        return n;
    }
    if let Some((_, n)) = PROVIDER_NAMES.iter().find(|(k, _)| *k == id) {
        return (*n).to_owned();
    }
    replace_all(id, re!(r"(^|-)(\w)", "g"), |c| format!("{}{}", if group(c, 1).unwrap_or("").is_empty() { "" } else { " " }, group(c, 2).unwrap_or("").to_uppercase()))
}

fn all_names() -> Vec<String> {
    let mut names: Vec<String> = learned().values().cloned().collect();
    for n in PROVIDER_NAMES.iter().map(|(_, n)| *n).chain(OLD_NAMES.iter().copied()) {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_owned());
        }
    }
    names.sort_by_key(|n| std::cmp::Reverse(n.len16()));
    names
}

/// The variant names a model offers, leaving out the disabled ones.
pub(crate) fn variant_names(variants: Option<&Value>) -> Vec<String> {
    variants.obj().map(|o| o.iter().filter(|(_, v)| !Some(*v).at("disabled").truthy()).map(|(k, _)| k.clone()).collect()).unwrap_or_default()
}

/// `opencode models --verbose` prints "provider/model" and then the model as pretty JSON, for every
/// provider that's connected. That JSON also carries request headers and provider options, which
/// can hold keys, so only the id, name, context size and variant names are kept.
pub fn parse_open_code_models(text: &str) -> Vec<AgentModel> {
    let mut out = vec![];
    let parts = split(text, re!(r"^([A-Za-z0-9._-]+\/\S+)[ \t]*$", "m"));
    let mut i = 1;
    while i < parts.len() {
        let id = parts[i].unwrap_or("");
        let body = parts.get(i + 1).copied().flatten().unwrap_or("");
        i += 2;
        let Some(m) = json::parse(body) else { continue };
        let m = Some(&m);
        if !m.truthy() || m.at("status").str() == Some("deprecated") {
            continue;
        }
        let provider = &id[..id.find('/').unwrap_or(0)];
        let name = m.at("name").nonempty().map_or_else(|| id[provider.len() + 1..].to_owned(), str::to_owned);
        out.push(AgentModel {
            id: id.to_owned(),
            label: format!("{} · {name}", provider_name(provider)),
            note: None,
            context: m.at("limit").at("context").num().map(inyon_protocol::Num),
            efforts: Some(variant_names(m.at("variants"))),
            default_effort: None,
            variants: None,
        });
    }
    out
}

/// agents OpenCode runs for itself, which its TUI never offers
const INTERNAL_AGENTS: &[&str] = &["compaction", "summary", "title"];

/// The agents `opencode agent list` prints ("build (primary)", then each one's permissions): the
/// ones a session can switch to.
pub fn parse_open_code_agents(text: &str) -> Vec<ModeOption> {
    let mut out: Vec<ModeOption> = vec![];
    for m in re!(r"^([\w.-]+) \((primary|all)\)\s*$", "gm").captures_iter(text) {
        let id = &m[1];
        if INTERNAL_AGENTS.contains(&id) || out.iter().any(|a| a.id == id) {
            continue;
        }
        out.push(ModeOption { id: id.to_owned(), label: capitalize(id) });
    }
    out
}

/// The command that lists the machine's models and agents (about 3 s to answer).
pub fn opencode_cli_models_command() -> String {
    format!("{OC_PATH}; opencode models --verbose 2>/dev/null; echo '@@AGENTS'; opencode agent list 2>/dev/null")
}

/// The catalog from what that command printed (`now`: unix ms).
pub fn opencode_cli_catalog(stdout: &str, now: u64) -> AgentCatalog {
    let (models_text, agents_text) = stdout.split_once("@@AGENTS\n").unwrap_or((stdout, ""));
    let models = parse_open_code_models(models_text);
    if models.is_empty() {
        return AgentCatalog { models, source: CatalogSource::Cli, updated_at: now, error: Some("OpenCode listed no models. Connect a provider first.".into()), modes: None };
    }
    let modes = parse_open_code_agents(agents_text);
    AgentCatalog { models, source: CatalogSource::Cli, updated_at: now, error: None, modes: (!modes.is_empty()).then_some(modes) }
}

/// The footer's "Model Name Provider Name" without a known provider's name.
pub fn without_provider(footer: &str) -> String {
    for name in all_names() {
        if let Some(rest) = footer.strip_suffix(name.as_str()).and_then(|r| r.strip_suffix(' ')) {
            return rest.to_owned();
        }
    }
    footer.to_owned()
}

fn label_name(m: &AgentModel) -> String {
    lower(&m.label.split(" · ").skip(1).collect::<Vec<_>>().join(" · "))
}

fn label_provider(m: &AgentModel) -> String {
    lower(m.label.split(" · ").next().unwrap_or(""))
}

/// Matches the footer's "Model Name Provider Name" to a catalog entry: the whole thing, then the
/// longest model name the footer starts with (two providers can offer the same model).
pub fn model_from_footer<'a>(models: &'a [AgentModel], footer: &str) -> Option<&'a AgentModel> {
    let text = lower(footer.jtrim());
    if let Some(exact) = models.iter().find(|m| text == format!("{} {}", label_name(m), label_provider(m))) {
        return Some(exact);
    }
    let mut by_name: Vec<&AgentModel> = models.iter().filter(|m| text == label_name(m) || text.starts_with(&format!("{} ", label_name(m)))).collect();
    by_name.sort_by_key(|m| std::cmp::Reverse(label_name(m).len16()));
    if by_name.len() <= 1 {
        return by_name.first().copied();
    }
    // several providers offer the model: the one whose name ends the footer, else the first
    let longest = label_name(by_name[0]);
    by_name.iter().copied().filter(|m| label_name(m) == longest).find(|m| text.ends_with(&format!(" {}", label_provider(m)))).or(Some(by_name[0]))
}

/// What to call the model in the footer: its catalog name when known, else the footer without the
/// provider.
pub fn footer_model_name(models: Option<&[AgentModel]>, footer: &str) -> String {
    match models.and_then(|m| model_from_footer(m, footer)) {
        Some(known) => known.label.split(" · ").skip(1).collect::<Vec<_>>().join(" · "),
        None => without_provider(footer),
    }
}

thread_local! {
    static CLI: RefCell<HashMap<String, (u64, Deferred<AgentCatalog>)>> = RefCell::new(HashMap::new());
    static SETTLED: RefCell<HashMap<String, AgentCatalog>> = RefCell::new(HashMap::new());
}

/// `opencodeCliModels(host)`: the models and agents OpenCode offers on this machine (about 3 s to
/// ask, so kept for 10 minutes). Starts asking at once; await the result or not.
pub fn opencode_cli_models(host: &Host) -> Deferred<AgentCatalog> {
    let id = host.id();
    if let Some((_, v)) = CLI.with(|c| c.borrow().get(&id).filter(|(at, _)| now_ms().saturating_sub(*at) < 600_000).cloned()) {
        return v;
    }
    let (h, hid) = (host.clone(), id.clone());
    let value = spawn_task(async move {
        let fail = |e: String| {
            CLI.with(|c| c.borrow_mut().remove(&hid));
            AgentCatalog { models: vec![], source: CatalogSource::Cli, updated_at: now_ms(), error: Some(e), modes: None }
        };
        match h.exec(&opencode_cli_models_command(), ExecOptions::timeout(45_000)).await {
            Err(e) => fail(e.message),
            Ok(r) => {
                let catalog = opencode_cli_catalog(&r.stdout, now_ms());
                if catalog.models.is_empty() {
                    CLI.with(|c| c.borrow_mut().remove(&hid));
                } else {
                    SETTLED.with(|s| s.borrow_mut().insert(hid.clone(), catalog.clone()));
                }
                catalog
            }
        }
    });
    // the task may already be over (an answer that didn't wait): only a pending or kept one is cached
    let keep = !value.is_settled() || value.value().is_some_and(|c| !c.models.is_empty());
    if keep {
        CLI.with(|c| c.borrow_mut().insert(id, (now_ms(), value.clone())));
    }
    value
}

/// `peekOpenCodeModels(hostId)`: the last catalog fetched for a machine, without waiting.
pub fn peek_open_code_models(host_id: &str) -> Option<AgentCatalog> {
    SETTLED.with(|s| s.borrow().get(host_id).cloned())
}

/// After a provider was connected or disconnected: the next ask reads the list again.
pub fn forget_open_code_models(host_id: &str) {
    CLI.with(|c| c.borrow_mut().remove(host_id));
    SETTLED.with(|s| s.borrow_mut().remove(host_id));
}

#[cfg(test)]
#[path = "opencode_models_tests.rs"]
mod tests;
