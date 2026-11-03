//! The engine's state files in ~/.inyon/node, read directly so `inyon`, `phones` and `doctor`
//! answer without starting it. The engine is the only writer; link.json is the one exception,
//! which setup writes before the link's first start.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::node_dir;

/// A paired phone, as phones.json keeps it under the phone's key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Phone {
    pub name: String,
    #[serde(rename = "pairedAt", default)]
    pub paired_at: f64,
    #[serde(rename = "lastSeen", default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<f64>,
}

/// What the running link writes: its addresses, pid and version.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LinkState {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub relays: Vec<String>,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: f64,
    #[serde(default)]
    pub pid: Option<u32>,
    /// missing from links before `tether-node update` existed
    #[serde(default)]
    pub version: Option<String>,
}

/// The link's settings (link.json); what's missing takes the engine's default.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LinkConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relays: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// whatever else the file holds stays in it when setup writes it back
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Identity {
    #[serde(default)]
    pub id: Option<String>,
}

/// What a started copy writes with its pid and version: state.json (the link), node.json, hub.json.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Running {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub version: Option<String>,
}

fn file(name: &str) -> PathBuf {
    node_dir().join(name)
}

fn read_json<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    let text = fs::read_to_string(file(name)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn identity() -> Option<Identity> {
    read_json("identity.json")
}

pub fn link_config() -> LinkConfig {
    read_json("link.json").unwrap_or_default()
}

pub fn link_state() -> Option<LinkState> {
    read_json("state.json")
}

/// The paired phones, each under a name a person reads.
pub fn phones() -> BTreeMap<String, Phone> {
    let mut all: BTreeMap<String, Phone> = read_json("phones.json").unwrap_or_default();
    for p in all.values_mut() {
        p.name = phone_name(&p.name);
    }
    all
}

/// The phone's model without the build number Android's browser adds ("A142 Build/BP2A.250605"
/// is "A142"), which some copies of the app sent as the name.
pub fn phone_name(raw: &str) -> String {
    let name = raw.split_once(" Build/").map(|(model, _)| model).unwrap_or(raw).trim();
    if name.is_empty() {
        "Phone".to_string()
    } else {
        name.to_string()
    }
}

/// Whether a pairing code is still open (pairing.json, which the engine removes once used).
pub fn pairing_open() -> bool {
    #[derive(Deserialize)]
    struct Pairing {
        #[serde(default)]
        expires: f64,
    }
    read_json::<Pairing>("pairing.json").is_some_and(|p| p.expires > now_ms())
}

/// Written whole or not at all, readable only by this user.
pub fn write_json<T: Serialize>(name: &str, value: &T) -> std::io::Result<()> {
    let dir = crate::paths::ensure_node_dir()?;
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, dir.join(name))
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// A moment in milliseconds since 1970 as this machine's local date and time.
pub fn format_time(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    format_secs(secs + local_offset(secs))
}

/// The same in UTC, for `doctor`'s clock line.
pub fn format_utc(ms: f64) -> String {
    format_secs((ms / 1000.0).floor() as i64)
}

/// The UTC moment as the logs write it, like the engine's 2026-10-14T11:01:23Z.
pub fn format_iso(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    format!("{}:{:02}Z", format_secs(secs).replace(' ', "T"), secs.rem_euclid(60))
}

pub fn format_date(ms: f64) -> String {
    format_time(ms)[..10].to_string()
}

fn format_secs(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

/// A log line that starts with a UTC time (2026-10-14T11:01:23.456Z, as the engine writes
/// them) with that time in local time instead; any other line comes back as it was.
pub fn local_log_line(line: &str) -> String {
    let Some((stamp, rest)) = line.split_once(' ') else { return line.to_string() };
    match parse_iso(stamp) {
        Some(secs) => {
            let local = secs + local_offset(secs);
            format!("{}:{:02} {rest}", format_secs(local), local.rem_euclid(60))
        }
        None => line.to_string(),
    }
}

/// Seconds since 1970 from YYYY-MM-DDTHH:MM:SS, with optional fractions and a Z at the end.
fn parse_iso(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let s = s.split_once('.').map(|(a, _)| a).unwrap_or(s);
    let (date, time) = s.split_once('T')?;
    let num = |t: &str, len: usize| if t.len() == len && t.bytes().all(|b| b.is_ascii_digit()) { t.parse::<i64>().ok() } else { None };
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let (y, m, day) = (num(d[0], 4)?, num(d[1], 2)?, num(d[2], 2)?);
    let (h, min, sec) = (num(t[0], 2)?, num(t[1], 2)?, num(t[2], 2)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || min > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, m as u32, day as u32) * 86_400 + h * 3600 + min * 60 + sec)
}

/// How far local time is ahead of UTC at that moment, in seconds (daylight saving included).
#[cfg(unix)]
fn local_offset(secs: i64) -> i64 {
    extern "C" {
        fn tzset();
    }
    let t = secs as libc::time_t;
    // SAFETY: an all-zero tm is a valid value of this plain C struct
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: tzset only reads TZ and the zone files; localtime_r fills our tm and keeps no pointer
    let ok = unsafe {
        tzset();
        !libc::localtime_r(&t, &mut tm).is_null()
    };
    if !ok {
        return 0;
    }
    let local = days_from_civil(tm.tm_year as i64 + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32) * 86_400 + tm.tm_hour as i64 * 3600 + tm.tm_min as i64 * 60 + tm.tm_sec as i64;
    local - secs
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SystemTimeToTzSpecificLocalTime(zone: *const std::ffi::c_void, utc: *const SystemTime, local: *mut SystemTime) -> i32;
}

/// How far local time is ahead of UTC at that moment, in seconds (daylight saving included).
#[cfg(windows)]
fn local_offset(secs: i64) -> i64 {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    if !(1601..=30_000).contains(&y) {
        return 0;
    }
    let utc =
        SystemTime { year: y as u16, month: m as u16, day: d as u16, hour: (rem / 3600) as u16, minute: (rem % 3600 / 60) as u16, second: (rem % 60) as u16, ..Default::default() };
    let mut local = SystemTime::default();
    // SAFETY: a null zone means the machine's own; both structs are ours and live through the call
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0 {
        return 0;
    }
    let back = days_from_civil(local.year as i64, local.month as u32, local.day as u32) * 86_400 + local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64;
    back - secs
}

#[cfg(not(any(unix, windows)))]
fn local_offset(_secs: i64) -> i64 {
    0
}

/// Howard Hinnant's days-from-civil, the other way.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's days-to-civil, so no date crate is needed.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_milliseconds() {
        assert_eq!(format_utc(0.0), "1970-01-01 00:00");
        assert_eq!(format_utc(1_760_313_600_000.0), "2025-10-13 00:00");
        assert_eq!(format_utc(951_782_400_000.0), "2000-02-29 00:00");
        assert_eq!(format_iso(1_760_439_683_000.0), "2025-10-14T11:01:23Z");
        for days in [-1, 0, 11_016, 20_374, 60_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn local_time_is_utc_moved_by_the_zone() {
        // whatever zone this runs in, it's a whole number of quarter hours from UTC
        let secs = 1_760_439_683;
        let off = local_offset(secs);
        assert!(off % 900 == 0 && off.abs() <= 14 * 3600, "{off}");
        assert_eq!(format_time(secs as f64 * 1000.0), format_secs(secs + off));
        assert_eq!(format_date(secs as f64 * 1000.0), format_time(secs as f64 * 1000.0)[..10]);
    }

    #[test]
    fn log_lines_get_local_times() {
        let secs = parse_iso("2025-10-14T11:01:23.456Z").unwrap();
        assert_eq!(secs, 1_760_439_683);
        assert_eq!(parse_iso("2025-10-14T11:01:23Z"), Some(secs));
        let local = secs + local_offset(secs);
        assert_eq!(local_log_line("2025-10-14T11:01:23.456Z tunnel: up"), format!("{}:23 tunnel: up", format_secs(local)));
        for plain in ["tunnel: up", "2025-10-14 11:01 running", "2025-13-14T11:01:23Z odd", ""] {
            assert_eq!(local_log_line(plain), plain);
        }
    }

    #[test]
    fn phones_parse_the_engines_shape() {
        let text = r#"{"abc123":{"name":"Pixel","pairedAt":1700000000000,"lastSeen":1700000500000},"def":{"name":"iPhone","pairedAt":1700000000000}}"#;
        let all: BTreeMap<String, Phone> = serde_json::from_str(text).unwrap();
        assert_eq!(all["abc123"].name, "Pixel");
        assert_eq!(all["def"].last_seen, None);
    }

    #[test]
    fn phone_names_drop_the_android_build() {
        assert_eq!(phone_name("A142 Build/BP2A.250605.031.A3"), "A142");
        assert_eq!(phone_name("Pixel 9 Pro Build/AP4A.250105.002; wv"), "Pixel 9 Pro");
        assert_eq!(phone_name("Work phone"), "Work phone");
        assert_eq!(phone_name(" Build/BP2A"), "Phone");
    }
}
