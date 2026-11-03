//! What the phone needs to pair with a machine, carried by the QR code (or a link you paste): the
//! machine's id (its public key), a one-time secret that proves you were at the machine, and the
//! addresses it can be reached at right now.

use serde::{Deserialize, Serialize};
use url::{form_urlencoded, Url};

use crate::identity::is_machine_id;
use crate::js;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairInfo {
    pub id: String,
    pub secret: String,
    pub name: String,
    /// wss:// or ws:// addresses of the machine's own listener (tunnel, LAN)
    pub urls: Vec<String>,
    /// relays the machine keeps a connection to
    pub relays: Vec<String>,
}

/// The link for the app. With `web` (the web app's address) it opens the web app instead, which is
/// what a phone's camera does with it; the code then rides after `#`, which browsers never send to
/// the server, so the host of the web app never sees it.
pub fn pair_link(p: &PairInfo, web: Option<&str>) -> String {
    let mut q = form_urlencoded::Serializer::new(String::new());
    q.append_pair("m", &p.id).append_pair("s", &p.secret).append_pair("n", &p.name);
    for u in &p.urls {
        q.append_pair("u", u);
    }
    for r in &p.relays {
        q.append_pair("r", r);
    }
    let q = q.finish();
    match web {
        Some(web) => format!("{web}#pair?{q}"),
        None => format!("inyon://pair?{q}"),
    }
}

/// `/^wss?:\/\/[^\s/]+/` or `/^https:\/\/[^\s/]+/`: a scheme we connect to, then a host.
fn reachable(u: &str) -> bool {
    ["ws://", "wss://", "https://"].iter().any(|scheme| u.strip_prefix(scheme).and_then(|rest| rest.chars().next()).is_some_and(|c| c != '/' && !js::is_space(c)))
}

/// Reads a pairing link; None if it isn't one.
pub fn parse_pair_link(text: &str) -> Option<PairInfo> {
    let url = Url::parse(js::trim(text)).ok()?;
    // tether:// is how codes looked before the rename to Inyon
    if !matches!(url.scheme(), "inyon" | "tether" | "https") {
        return None;
    }
    let pairs: Vec<(String, String)> = match url.fragment().and_then(|f| f.strip_prefix("pair?")) {
        // URLSearchParams drops one leading "?" from the string it's given
        Some(q) => form_urlencoded::parse(q.strip_prefix('?').unwrap_or(q).as_bytes()).into_owned().collect(),
        None => url.query_pairs().into_owned().collect(),
    };
    let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let all = |k: &str| pairs.iter().filter(|(key, v)| key == k && reachable(v)).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    let id = get("m").unwrap_or("");
    let secret = get("s").unwrap_or("");
    if !is_machine_id(id) || js::len(secret) < 16 {
        return None;
    }
    let name = js::slice(get("n").unwrap_or(""), 64);
    Some(PairInfo { id: id.to_string(), secret: secret.to_string(), name: if name.is_empty() { "Machine" } else { name }.to_string(), urls: all("u"), relays: all("r") })
}
