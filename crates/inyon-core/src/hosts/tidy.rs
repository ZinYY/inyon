//! Port of hosts/tidy.ts: what Inyon takes back off a machine just before it's removed.

use futures_util::future::select;
use inyon_protocol::{HostState, HostStatus, IntegrationState};

use super::host::{shq, ExecOptions, Host};
use crate::claude::integration::{uninstall_agy_integration, uninstall_integration};
use crate::platform::rt::{sleep, spawn_task};

fn hooked(s: Option<IntegrationState>) -> bool {
    matches!(s, Some(IntegrationState::Installed | IntegrationState::Outdated | IntegrationState::Removed))
}

/// Removes one exact line from ~/.ssh/authorized_keys. The file is only rewritten when the copy
/// without the line has exactly the lines it should, so a full disk can never empty it. Lines are
/// counted with `grep -c ''`, which unlike `wc -l` counts a last line with no newline after it.
pub fn drop_authorized_key(line: &str) -> String {
    let l = shq(line);
    format!(
        r#"f="$HOME/.ssh/authorized_keys"; t="$f.inyon-tmp"; if [ -f "$f" ] && grep -qxF {l} "$f"; then grep -vxF {l} "$f" > "$t"; [ "$(grep -c '' "$t")" -eq "$(( $(grep -c '' "$f") - $(grep -cxF {l} "$f") ))" ] && cat "$t" > "$f"; rm -f "$t"; fi;"#
    )
}

/// The parts of a machine's state tidying looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TidyState {
    pub status: HostStatus,
    pub integration: Option<IntegrationState>,
    pub agy_integration: Option<IntegrationState>,
}

impl From<&HostState> for TidyState {
    fn from(s: &HostState) -> Self {
        TidyState { status: s.status, integration: Some(s.integration), agy_integration: s.agy_integration }
    }
}

/// Takes back what Inyon put on a machine, while it can still be reached, just before it's removed:
/// the Claude and Antigravity integration (hooks, status line, prompt log), the uploads, and any SSH
/// connection kept open to it. `also` is more shell to run there (the phone's key coming out of
/// authorized_keys). Gives up after 20 seconds; removing never waits on a machine that won't answer
/// (the work itself goes on, as the TS promise does).
pub async fn tidy_machine(host: &Host, state: Option<TidyState>, also: &str) {
    let (host, also) = (host.clone(), also.to_owned());
    let work = spawn_task(async move {
        if let Some(state) = state.filter(|s| s.status == HostStatus::Online) {
            if hooked(state.integration) {
                let _ = uninstall_integration(&host).await;
            } else if hooked(state.agy_integration) {
                let _ = uninstall_agy_integration(&host).await;
            }
            let _ = host
                .exec(
                    &format!(r#"rm -rf "$HOME/.inyon/uploads" "$HOME/.inyon/upload-policy" "$HOME/.inyon/events.jsonl" "$HOME/.inyon/events.old.jsonl"; {also} true"#),
                    ExecOptions::timeout(10000),
                )
                .await;
        }
        let _ = host.close().await;
    });
    let done = Box::pin(work.wait());
    let _ = select(done, sleep(20000)).await;
}

#[cfg(test)]
#[path = "tidy_tests.rs"]
mod tests;
