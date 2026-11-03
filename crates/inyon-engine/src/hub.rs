//! The part of hub.ts a machine needs without the hub: which hub addresses may take its token
//! over plain http. The hub itself (`inyon hub`) still runs from the Bun engine.

use std::net::IpAddr;

use url::Url;

/// An address plain http may carry the token to: this machine, or a VPN that encrypts on its own
/// (Tailscale and NetBird's 100.64.0.0/10, Tailscale's fd7a:115c:a1e0::/48). A LAN address isn't:
/// anyone on the Wi-Fi can read the token there, or answer in the hub's place.
pub fn plain_http_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || (a == 100 && (64..=127).contains(&b))
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => plain_http_ip(IpAddr::V4(v4)),
            None => v6.is_loopback() || v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        },
    }
}

/// What a hub address on plain http is: fine as it is (an address above, or localhost), a name
/// whose addresses have to be looked up and checked before every connection, or refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plain {
    Ok,
    LookUp,
    Refused,
}

pub fn plain_host(url: &Url) -> Plain {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ok_if(plain_http_ip(ip.into())),
        Some(url::Host::Ipv6(ip)) => ok_if(plain_http_ip(ip.into())),
        Some(url::Host::Domain(d)) if d.eq_ignore_ascii_case("localhost") => Plain::Ok,
        Some(url::Host::Domain(_)) => Plain::LookUp,
        None => Plain::Refused,
    }
}

fn ok_if(ok: bool) -> Plain {
    if ok {
        Plain::Ok
    } else {
        Plain::Refused
    }
}

/// Whether Inyon before 0.20 sent the token over plain http to this address: a private one (10/8,
/// 172.16/12, 192.168/16, 100.64/10, loopback, fc00::/7, fe80::/10), localhost, a single-label
/// name, or one ending in .ts.net, .netbird.cloud, .netbird.selfhosted, .local, .lan or .internal.
/// A machine added then keeps connecting to such a hub, with a warning; a public address it never
/// went to stays refused.
pub fn plain_before_0_20(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            let [a, b, ..] = ip.octets();
            a == 10 || a == 127 || (a == 192 && b == 168) || (a == 172 && (16..=31).contains(&b)) || (a == 100 && (64..=127).contains(&b))
        }
        Some(url::Host::Ipv6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
        Some(url::Host::Domain(d)) => {
            let h = d.to_ascii_lowercase();
            h == "localhost" || [".ts.net", ".netbird.cloud", ".netbird.selfhosted", ".local", ".lan", ".internal"].iter().any(|s| h.ends_with(s)) || !h.contains('.')
        }
        None => false,
    }
}

/// Why a hub address can't take the token over plain http, and what to do instead.
pub fn plain_refusal(host: &str) -> String {
    format!(
        "Refusing to send the token over plain http to {host}: anyone on that network could read it, or answer in the hub's place. Use the hub's https address, or its Tailscale or NetBird address (100.x.y.z), or set INYON_ALLOW_INSECURE=1 to send it anyway."
    )
}

/// Looks up a hub's name for a connection made right after, to the addresses it returns and no
/// others. Only the ones plain http may go to are kept, so a name that also has an address on the
/// LAN or NetBird's own IPv6 still works: the connection never goes there. Err(None) when the name
/// couldn't be looked up, which is worth trying again; Err(Some(why)) when no address is allowed.
pub async fn check_name(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, Option<String>> {
    keep_allowed(host, tokio::net::lookup_host((host, port)).await.map_err(|_| None)?.collect())
}

fn keep_allowed(host: &str, addrs: Vec<std::net::SocketAddr>) -> Result<Vec<std::net::SocketAddr>, Option<String>> {
    let first = addrs.first().ok_or(None)?.ip();
    let kept: Vec<std::net::SocketAddr> = addrs.into_iter().filter(|a| plain_http_ip(a.ip())).collect();
    if kept.is_empty() {
        return Err(Some(refused_name(host, first)));
    }
    Ok(kept)
}

/// For a program that looks the name up again itself (`inyon join`), with no async threads.
pub fn check_name_now(host: &str, port: u16) -> Result<(), Option<String>> {
    use std::net::ToSocketAddrs;
    judge(host, &(host, port).to_socket_addrs().map_err(|_| None)?.map(|a| a.ip()).collect::<Vec<_>>())
}

/// Whether plain http may go to a name that something else looks up again (the HTTP client here,
/// the WebView in the app), so any of its addresses may be the one used. Every address must be
/// allowed, with one exception: NetBird gives each peer an IPv6 address too, in a unique-local
/// /64 picked at random per network (fd91:8fa2:…), so no fixed range names it. A unique-local
/// IPv6 (fc00::/7) passes when the name also has an allowed IPv4 (100.64/10 or loopback): that is
/// the VPN's own overlay next to its 100.x address. A LAN or public address anywhere still
/// refuses, and so does a unique-local address alone. The app's rule (apps/web/src/lib/hubs.ts)
/// is the same.
fn judge(host: &str, addrs: &[IpAddr]) -> Result<(), Option<String>> {
    if addrs.is_empty() {
        return Err(None);
    }
    let vpn_v4 = addrs.iter().any(|ip| ip.is_ipv4() && plain_http_ip(*ip));
    let overlay = |ip: &IpAddr| matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xfe00 == 0xfc00);
    match addrs.iter().find(|ip| !plain_http_ip(**ip) && !(vpn_v4 && overlay(ip))) {
        Some(bad) => Err(Some(refused_name(host, *bad))),
        None => Ok(()),
    }
}

fn refused_name(host: &str, bad: IpAddr) -> String {
    format!("{} ({host} is {bad})", plain_refusal(host).trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(hub: &str) -> Plain {
        plain_host(&Url::parse(hub).unwrap())
    }

    #[test]
    fn plain_http_is_for_this_machine_and_the_vpn_ranges() {
        for h in ["localhost", "127.0.0.1", "127.8.0.1", "100.64.0.1", "100.127.255.255", "[::1]", "[fd7a:115c:a1e0::1]", "[fd7a:115c:a1e0:ab12::5]", "[::ffff:100.100.1.1]"] {
            assert_eq!(plain(&format!("http://{h}:4777")), Plain::Ok, "{h}");
        }
    }

    #[test]
    fn a_lan_address_or_a_public_one_is_refused_and_a_name_is_looked_up() {
        // a LAN can be listened to, and answered by anyone on it
        for h in [
            "10.0.0.1",
            "192.168.1.20",
            "172.16.0.1",
            "8.8.8.8",
            "100.128.0.1",
            "100.63.255.255",
            "[fd00::1]",
            "[fe80::1]",
            "[fd7a:115c:a1e1::1]",
            "[2001:db8::1]",
            "[::ffff:192.168.1.2]",
        ] {
            assert_eq!(plain(&format!("http://{h}:4777")), Plain::Refused, "{h}");
        }
        // mDNS, LLMNR and the Wi-Fi's DNS answer names: what they point to is checked each time
        for h in ["hub.local", "hub.lan", "hub", "hub.tail1234.ts.net", "10.0.0.1.attacker.com"] {
            assert_eq!(plain(&format!("http://{h}:4777")), Plain::LookUp, "{h}");
        }
    }

    #[tokio::test]
    async fn a_name_is_connected_to_only_at_its_allowed_addresses() {
        // the addresses it was judged by, to connect to and nothing else
        assert!(check_name("localhost", 4777).await.unwrap().iter().all(|a| a.ip().is_loopback() && a.port() == 4777));
        // 192.168.1.2 written as a name
        let lan = check_name("192.168.1.2", 4777).await.unwrap_err().unwrap();
        assert!(lan.starts_with("Refusing to send the token over plain http to 192.168.1.2") && lan.contains("is 192.168.1.2"), "{lan}");
        assert_eq!(check_name("100.100.1.1", 4777).await, Ok(vec!["100.100.1.1:4777".parse().unwrap()]));
        assert_eq!(check_name("nothing.invalid", 4777).await, Err(None));
        assert_eq!(check_name_now("localhost", 4777), Ok(()));
        assert!(check_name_now("192.168.1.2", 4777).unwrap_err().is_some());
    }

    #[test]
    fn a_netbird_name_with_its_ipv6_passes_and_a_lan_address_beside_it_does_not() {
        let ips = |l: &[&str]| l.iter().map(|s| s.parse().unwrap()).collect::<Vec<IpAddr>>();
        let ok = |l: &[&str]| judge("hub", &ips(l)).is_ok();
        // NetBird with IPv6 on: its 100.x address and a random unique-local one
        assert!(ok(&["100.97.188.112", "fd91:8fa2:3e51:2fcb:2587:6cbd:82:4402"]));
        assert!(ok(&["fd7a:115c:a1e0::5"]), "Tailscale's IPv6 alone");
        assert!(ok(&["100.97.188.112"]) && ok(&["127.0.0.1", "::1"]));
        assert!(!ok(&["100.97.188.112", "192.168.1.5"]), "a LAN address beside it");
        assert!(!ok(&["100.97.188.112", "2001:db8::1"]), "a public IPv6 beside it");
        assert!(!ok(&["fd91:8fa2:3e51:2fcb::1"]), "a unique-local address alone is any LAN's");
        assert!(!ok(&["fd7a:115c:a1e0::5", "fd91:8fa2:3e51:2fcb::1"]), "the exception needs a 100.x address");
        let why = judge("hub", &ips(&["100.97.188.112", "192.168.1.5"])).unwrap_err().unwrap();
        assert!(why.contains("hub is 192.168.1.5"), "{why}");
        assert_eq!(judge("hub", &[]), Err(None));
        // a connection made to the looked-up addresses goes only to the allowed ones
        let at = |l: &[&str]| l.iter().map(|s| std::net::SocketAddr::new(s.parse().unwrap(), 4777)).collect::<Vec<_>>();
        assert_eq!(keep_allowed("hub", at(&["192.168.1.5", "100.97.188.112", "fd91::1"])), Ok(at(&["100.97.188.112"])));
        let none = keep_allowed("hub", at(&["192.168.1.5", "fd91::1"])).unwrap_err().unwrap();
        assert!(none.contains("hub is 192.168.1.5"), "{none}");
        assert_eq!(keep_allowed("hub", vec![]), Err(None));
    }

    #[test]
    fn machines_added_before_0_20_keep_only_the_addresses_that_version_sent_the_token_to() {
        let before = |h: &str| plain_before_0_20(&Url::parse(&format!("http://{h}:4777")).unwrap());
        for h in [
            "localhost",
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.20",
            "172.16.0.1",
            "172.31.255.255",
            "100.64.0.1",
            "[::1]",
            "[fd7a:115c:a1e0::1]",
            "[fe80::1]",
            "hub.ts.net",
            "hub.local",
            "hub.lan",
            "hub.internal",
            "hub",
        ] {
            assert!(before(h), "{h}");
        }
        for h in ["example.com", "8.8.8.8", "203.0.113.7", "172.32.0.1", "100.128.0.1", "10.0.0.1.attacker.com", "192.168.1.20.example.net", "127.0.0.1.nip.io", "[2001:db8::1]"] {
            assert!(!before(h), "{h}");
        }
    }
}
