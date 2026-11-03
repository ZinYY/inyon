//! The machine side of Inyon, which the Bun engine ran until 0.19 (it was removed in 0.23): the
//! phone link, the connection to a hub, the commands either of them asks this machine to run, and
//! the notifications it sends to paired browsers. The `inyon` command runs all of it in-process.
//!
//! Each module is the port of one file of the Bun engine (`apps/node` until 0.23), keeping its
//! structure: `ops` (ops.ts), `connect` (inyon.ts's connection to a hub), `link` (link.ts),
//! `push` (push.ts), `tidy`, `paths` and `output`. Everything a phone, a hub or an older `inyon`
//! command sees stays as the Bun engine had it: the `/node` frames, the link's bytes, the `--json`
//! answers and the files under `~/.inyon`.

pub mod cli;
pub mod connect;
pub mod http;
pub mod hub;
pub mod hub_tls;
pub mod link;
pub mod networks;
pub mod ops;
pub mod output;
pub mod paths;
pub mod push;
pub mod qr;
pub mod screen;
pub mod tidy;
pub mod webpush;
#[cfg(windows)]
pub mod winjob;
mod ws;

/// Inyon's version, the same for every part of a release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The web app pairing QR codes open (INYON_WEB_URL points them at a copy you host).
pub const WEB_URL: &str = "https://inyon.tech/";
/// The short address the one-line installers are typed with.
pub const INSTALL_URL: &str = "https://inyon.tech";

/// What the engine's commands are, as `inyon-engine --help` lists them.
pub const USAGE: &str = "inyon-engine: the program behind the inyon command, which runs it for these.

  inyon-engine pair [options]       make a pairing code (starts the link here if none runs)
  inyon-engine link [options]       run the phone link
    --lan / --no-lan                also answer on this machine's LAN addresses
    --vpn / --no-vpn                also answer on Tailscale, NetBird, ZeroTier, WireGuard (on)
    --tunnel / --no-tunnel          open a free Cloudflare tunnel so any network works (on)
    --url <wss://…> / --no-url      publish an address of your own (repeatable)
    --relay <url> / --no-relay      keep a connection to an Inyon relay (repeatable)
    --port <n>                      local port (4780)
  inyon-engine phones               list paired phones
  inyon-engine forget <name|key>    unpair a phone
  inyon-engine push …               the browsers this machine sends notifications to
  inyon-engine join <hub-url>       save a hub's address, with the token in INYON_TOKEN
  inyon-engine connect              run the connection to that hub
  inyon-engine hub                  run the hub
  inyon-engine hub code             show the setup code a new hub asks for
  inyon-engine hub companion-code   a one-time code for a companion app to pair with
  inyon-engine hub reset-password   choose a new password for a forgotten one
  inyon-engine version              print the version

Services and updates are the inyon command's: inyon setup, inyon service, inyon hub start, inyon update.";

/// The usage with this version in its first line.
pub fn usage() -> String {
    USAGE.replacen("inyon-engine:", &format!("inyon-engine {VERSION}:"), 1)
}

/// Windows, where the shell is busybox-w32 and tmux is psmux.
pub const WIN: bool = cfg!(windows);

/// `os.platform()` as Node names it: linux, darwin or win32.
pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}

/// `os.arch()` as Node names it: x64 or arm64.
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "arm" => "arm",
        other => other,
    }
}

/// `os.hostname()`.
pub fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length, and gethostname writes at most that much
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
        std::env::var("HOSTNAME").unwrap_or_default()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{ComputerNamePhysicalDnsHostname, GetComputerNameExW};
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: len holds the buffer's size, which the call never writes past
        if unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, buf.as_mut_ptr(), &mut len) } != 0 {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
}

/// `Date.now()`: milliseconds since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// `new Date().toISOString()`, as the Bun engine stamped its log lines.
pub fn iso_now() -> String {
    iso(now_ms())
}

/// A moment as `2026-10-14T11:01:23.456Z`.
pub fn iso(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    // civil from days, after Howard Hinnant's algorithm
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{:03}Z", ms % 1000)
}

/// Random bytes from the system.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    getrandom::fill(&mut out).expect("the system has no randomness to give");
    out
}

/// How long to wait before trying again: capped exponential backoff with full jitter, a random
/// time between 0 and min(cap, base·2^attempt). After a hub restarts, or the pkarr relays come
/// back, every machine would otherwise try again at the same moments.
pub fn backoff(attempt: u32, base: std::time::Duration, cap: std::time::Duration) -> std::time::Duration {
    let r = u64::from_le_bytes(random_bytes(8).try_into().unwrap_or([0; 8]));
    backoff_at(attempt, base, cap, r as f64 / (u64::MAX as f64 + 1.0))
}

/// `backoff` with the random draw given (0 ≤ r < 1).
pub fn backoff_at(attempt: u32, base: std::time::Duration, cap: std::time::Duration, r: f64) -> std::time::Duration {
    cap.min(base.saturating_mul(1u32 << attempt.min(20))).mul_f64(r.clamp(0.0, 1.0))
}

/// How long a connection has to stay up before its backoff starts from the first step again; one
/// that opens and drops at once (a crash loop, a proxy that cuts upgrades) keeps backing off.
pub const STABLE: std::time::Duration = std::time::Duration::from_secs(30);

/// The attempt number after a try that stayed up for `up` (None: it never opened).
pub fn next_attempt(attempt: u32, up: Option<std::time::Duration>) -> u32 {
    if up.is_some_and(|u| u >= STABLE) {
        0
    } else {
        attempt.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_times_read_like_javascripts() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_760_440_883_456), "2025-10-14T11:21:23.456Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn retries_back_off_exponentially_up_to_the_cap_with_full_jitter() {
        use std::time::Duration;
        let (s1, s30) = (Duration::from_secs(1), Duration::from_secs(30));
        let top: Vec<u128> = [0, 1, 2, 3, 4, 5, 40].iter().map(|a| backoff_at(*a, s1, s30, 1.0).as_millis()).collect();
        assert_eq!(top, [1000, 2000, 4000, 8000, 16000, 30000, 30000]);
        assert_eq!(backoff_at(3, s1, s30, 0.0), Duration::ZERO);
        assert_eq!(backoff_at(3, s1, s30, 0.5), Duration::from_secs(4));
        for _ in 0..1000 {
            assert!(backoff(4, s1, s30) < Duration::from_secs(16));
        }
    }

    #[test]
    fn the_backoff_starts_over_only_after_a_connection_stayed_up_a_while() {
        use std::time::Duration;
        assert_eq!(next_attempt(3, None), 4);
        // a hub that accepts and drops at once keeps being backed off from
        assert_eq!(next_attempt(3, Some(Duration::from_millis(200))), 4);
        assert_eq!(next_attempt(3, Some(STABLE)), 0);
    }

    #[test]
    fn platform_names_are_nodes() {
        assert!(["linux", "darwin", "win32"].contains(&platform()));
        assert!(["x64", "arm64"].contains(&arch()));
        assert!(!hostname().is_empty());
    }
}
