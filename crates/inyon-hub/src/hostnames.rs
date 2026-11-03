//! Port of hostnames.ts: which names the hub answers to. A web page on a domain its author
//! controls can repoint that domain at your machine (DNS rebinding) and then talk to the hub as its
//! own site; the browser still sends the attacker's name in Host. So browser requests are only
//! answered under names a stranger can't publish in public DNS:
//!
//!  - IP addresses (127.0.0.1, [::1], your Tailscale or NetBird address) and localhost
//!  - single-label names such as `my-server`, which only your own network resolves
//!  - private DNS suffixes: Tailscale (.ts.net), NetBird (.netbird.cloud, .netbird.selfhosted),
//!    mDNS and home networks (.local, .lan, .internal, .home.arpa, .localhost)
//!  - this machine's hostname, the BIND address, and the hosts in INYON_ALLOWED_ORIGINS
//!  - this machine's own name on its private networks (a Headscale or NetBird server with its own
//!    domain), found as the hub starts (see networks)
//!  - anything listed in INYON_ALLOWED_HOSTS (comma-separated, like `inyon.example.com`).
//!    Behind a reverse proxy on a public domain, add that domain here. `*` turns the check off.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

const PRIVATE_SUFFIXES: [&str; 8] = [".ts.net", ".netbird.cloud", ".netbird.selfhosted", ".local", ".lan", ".internal", ".home.arpa", ".localhost"];

fn list(v: Option<&str>) -> Vec<String> {
    v.unwrap_or("").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
}

/// "Example.com:4777" or "[::1]:4777" → "example.com" / "[::1]"; None if it isn't a host.
pub fn hostname_of(host: &str) -> Option<String> {
    // some clients send an IPv6 address without its brackets
    let host = if !host.starts_with('[') && host.matches(':').count() >= 2 { format!("[{host}]") } else { host.to_owned() };
    let u = url::Url::parse(&format!("http://{host}")).ok()?;
    // the URL's host with its port, as `u.host` reads it
    let shown = match u.port() {
        Some(p) => format!("{}:{p}", u.host_str()?),
        None => u.host_str()?.to_owned(),
    };
    // the URL must be nothing but that host: no path, user or other part was in it
    if u.path() != "/" || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let lower = host.to_lowercase();
    let lower_no80 = lower.strip_suffix(":80").unwrap_or(&lower);
    (shown == lower || shown == lower_no80).then(|| u.host_str().map(str::to_owned)).flatten()
}

/// The check: whether a Host header names this hub. `found` gets this machine's names on its
/// private networks once they're known.
pub fn allowed_hosts(allowed_hosts: Option<&str>, allowed_origins: Option<&str>, hostname: &str, bind: &str, found: Rc<RefCell<HashSet<String>>>) -> impl Fn(Option<&str>) -> bool {
    let mut names: HashSet<String> = list(allowed_hosts).into_iter().collect();
    for n in [hostname.to_lowercase(), bind.to_lowercase()] {
        if !n.is_empty() {
            names.insert(n);
        }
    }
    for origin in list(allowed_origins) {
        if let Some(h) = url::Url::parse(&origin).ok().and_then(|u| u.host_str().map(str::to_owned)) {
            names.insert(h);
        }
    }
    let any = names.contains("*");
    move |host: Option<&str>| {
        if any {
            return true;
        }
        let Some(name) = host.filter(|h| !h.is_empty()).and_then(hostname_of) else { return false };
        if ipv4_shape(&name) || name.starts_with('[') {
            return true;
        }
        if name == "localhost" || !name.contains('.') || PRIVATE_SUFFIXES.iter().any(|s| name.ends_with(s)) {
            return true;
        }
        names.contains(&name) || found.borrow().contains(&name)
    }
}

/// `/^\d{1,3}(\.\d{1,3}){3}$/`
fn ipv4_shape(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // hostnames.test.ts
    fn check(env_hosts: Option<&str>, origins: Option<&str>, bind: &str) -> impl Fn(Option<&str>) -> bool {
        allowed_hosts(env_hosts, origins, "my-box", bind, Rc::new(RefCell::new(HashSet::new())))
    }

    #[test]
    fn addresses_localhost_single_labels_and_private_suffixes_pass() {
        let ok = check(None, None, "127.0.0.1");
        for h in [
            "127.0.0.1:4777",
            "[::1]:4777",
            "100.64.1.2",
            "localhost:4777",
            "my-server",
            "laptop.tail1234.ts.net",
            "box.netbird.cloud",
            "nas.local",
            "pi.home.arpa",
            "MY-BOX:4777",
            "[fd7a:115c:a1e0::1]",
        ] {
            assert!(ok(Some(h)), "{h}");
        }
    }

    #[test]
    fn public_names_and_garbage_are_refused() {
        let ok = check(None, None, "127.0.0.1");
        for h in [
            "evil.example",
            "evil.example:4777",
            "127.0.0.1.evil.example",
            "",
            "a b",
            "user@evil.example",
            "evil.example/path",
            "localhost.evil.example",
            "ts.net.evil.example",
            "abc/def",
        ] {
            assert!(!ok(Some(h)), "{h}");
        }
        assert!(!ok(None));
    }

    #[test]
    fn listed_hosts_origins_and_found_names_pass_and_a_star_turns_it_off() {
        let found = Rc::new(RefCell::new(HashSet::new()));
        let ok = allowed_hosts(Some("Inyon.Example.com"), Some("https://app.example.org:8443/"), "my-box", "0.0.0.0", found.clone());
        assert!(ok(Some("inyon.example.com")));
        assert!(ok(Some("app.example.org:8443")));
        assert!(!ok(Some("box.headscale.example")));
        found.borrow_mut().insert("box.headscale.example".into());
        assert!(ok(Some("box.headscale.example")));
        assert!(check(Some("*"), None, "127.0.0.1")(Some("anything.example")));
    }

    #[test]
    fn a_comma_separated_list_upper_case_names_and_the_bind_address_pass() {
        let ok = check(Some("Inyon.Example.com, other.example"), Some("https://proxy.example:8443"), "0.0.0.0");
        assert!(ok(Some("inyon.example.com")));
        assert!(ok(Some("INYON.example.com:443")));
        assert!(ok(Some("other.example")));
        assert!(ok(Some("proxy.example:8443")));
        assert!(!ok(Some("evil.example")));
        assert!(check(None, None, "hub.example.org")(Some("hub.example.org:4777")));
    }

    #[test]
    fn a_found_name_passes_with_its_port_and_its_neighbours_do_not() {
        let found = Rc::new(RefCell::new(HashSet::new()));
        let ok = allowed_hosts(None, None, "my-box", "127.0.0.1", found.clone());
        assert!(!ok(Some("laptop.hs.example.com")));
        found.borrow_mut().insert("laptop.hs.example.com".into());
        assert!(ok(Some("laptop.hs.example.com:4777")));
        assert!(!ok(Some("other.hs.example.com")));
    }

    #[test]
    fn a_long_ipv6_address_passes_with_or_without_its_brackets() {
        let ok = check(None, None, "127.0.0.1");
        assert!(ok(Some("[fd91:8fa2:3e51:2fcb:2587:6cbd:82:4402]")));
        assert!(ok(Some("[fd91:8fa2:3e51:2fcb:2587:6cbd:82:4402]:4777")));
        assert!(ok(Some("fd91:8fa2:3e51:2fcb:2587:6cbd:82:4402")));
    }

    #[test]
    fn hostname_of_keeps_the_host_only() {
        assert_eq!(hostname_of("Example.com:4777").as_deref(), Some("example.com"));
        assert_eq!(hostname_of("[::1]:4777").as_deref(), Some("[::1]"));
        assert_eq!(hostname_of("::1").as_deref(), Some("[::1]"));
        assert_eq!(hostname_of("example.com:80").as_deref(), Some("example.com"));
        assert_eq!(hostname_of("0x7f.1"), None);
    }
}
