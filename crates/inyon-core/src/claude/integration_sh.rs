//! Port of claude/integration-sh.ts: shell shared by the integration scripts and the probe. The
//! text is byte for byte the TS's (fixtures/expected/scripts.json checks it).

/// Bump when the scripts change; the probe reports older copies as outdated.
pub const MARKER_VERSION: u32 = 9;
pub const MARKER: &str = r"inyon-integration 9";

/// Shell test: the script in file $1 is this version of the integration or a newer one. A phone
/// still on an older app then leaves scripts a newer one wrote alone, instead of the two putting
/// their own version back in turn.
pub const CURRENT_SH: &str = r#"current() { v=$(sed -n 's/^# inyon-integration \([0-9][0-9]*\)$/\1/p' "$1" 2>/dev/null | head -n 1); [ -n "$v" ] && [ "$v" -ge 9 ]; }"#;

/// Sets $pane to the pane the script runs in, as the hub names it. psmux (Windows) numbers panes per
/// session, so every session has a %1: the hub calls them %<session>.<pane> (win-prelude.sh), and the
/// scripts ask psmux for the session id so two sessions don't share one file.
pub const PANE_SH: &str = r#"pane=${TMUX_PANE:-}
if [ -n "$pane" ] && [ -n "${PSMUX_SESSION:-}" ]; then
  for _ in 1 2; do s=$(tmux display -p -t "$PSMUX_SESSION" '#{session_id}' 2>/dev/null | tr -d '$\r'); [ -n "$s" ] && break; done
  case $s in ''|*[!0-9]*) ;; *) pane="%$s.${pane#%}" ;; esac
fi"#;

/// Inyon was called Tether: moves ~/.tether to ~/.inyon once and leaves a link at the old path, so
/// hooks and status lines still pointing there keep working until they're rewritten.
pub const MOVE_OLD_FOLDER_SH: &str = r#"if [ -d "$HOME/.tether" ] && [ ! -L "$HOME/.tether" ] && [ ! -e "$HOME/.inyon" ]; then mv "$HOME/.tether" "$HOME/.inyon" && { [ -e "$HOME/.tether" ] || ln -s "$HOME/.inyon" "$HOME/.tether"; }; fi"#;

/// Shell that prints the account a Claude config file is signed in to (oauthAccount.accountUuid),
/// or "-". Append the file as an argument; the status line wrapper and the probe both use it.
pub const ACCOUNT_SH: &str = r#"a() { v=$(tr -d '\n\r' < "$1" 2>/dev/null | grep -o '"oauthAccount": *{[^}]*' | grep -o '"accountUuid": *"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'); echo "${v:--}"; }; a"#;
