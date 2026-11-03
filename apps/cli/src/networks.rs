//! The networks this machine is on, for the setup screen and `doctor`: private networks
//! (Tailscale, NetBird, ZeroTier, WireGuard), whose addresses only reach devices the network
//! lets in, and the LAN. The Tailscale and NetBird programs are asked first, since they know the
//! machine's name on the network; anything else is recognised by its interface name.

use crate::sys::{run, which, Platform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tailscale,
    Headscale,
    Netbird,
    Zerotier,
    Wireguard,
    Lan,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Tailscale => "Tailscale",
            Kind::Headscale => "Headscale",
            Kind::Netbird => "NetBird",
            Kind::Zerotier => "ZeroTier",
            Kind::Wireguard => "WireGuard",
            Kind::Lan => "LAN",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    pub kind: Kind,
    /// this machine's IPv4 address on the network
    pub ip: String,
    /// its name there, like laptop.tail1234.ts.net
    pub name: Option<String>,
}

fn kind_of(iface: &str) -> Option<Kind> {
    let l = iface.to_lowercase();
    if l.starts_with("tailscale") {
        Some(Kind::Tailscale)
    } else if (l.starts_with("wt") && l[2..].chars().all(|c| c.is_ascii_digit()) && l.len() > 2) || l.starts_with("netbird") {
        Some(Kind::Netbird)
    } else if l.starts_with("zt") || l.starts_with("feth") || l.starts_with("zerotier") {
        Some(Kind::Zerotier)
    } else if l == "wg" || (l.starts_with("wg") && l[2..].chars().all(|c| c.is_ascii_digit() || c == '-')) || l.contains("wireguard") {
        Some(Kind::Wireguard)
    } else {
        None
    }
}

fn is_lan(ip: &str) -> bool {
    let p: Vec<u8> = ip.split('.').filter_map(|o| o.parse().ok()).collect();
    if p.len() != 4 {
        return false;
    }
    p[0] == 10 || (p[0] == 192 && p[1] == 168) || (p[0] == 172 && (16..=31).contains(&p[1]))
}

/// `tailscale status --json`, when it's logged in and running.
pub fn from_tailscale(status: &serde_json::Value) -> Option<Network> {
    if status.get("BackendState")?.as_str()? != "Running" {
        return None;
    }
    let ips = status.pointer("/Self/TailscaleIPs").or_else(|| status.get("TailscaleIPs"))?.as_array()?;
    let ip = ips.iter().filter_map(|v| v.as_str()).find(|s| s.split('.').count() == 4)?.to_string();
    let name = status.pointer("/Self/DNSName").and_then(|v| v.as_str()).map(|s| s.trim_end_matches('.').to_lowercase()).filter(|s| !s.is_empty());
    let suffix = status
        .pointer("/CurrentTailnet/MagicDNSSuffix")
        .or_else(|| status.get("MagicDNSSuffix"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('.').to_lowercase())
        .unwrap_or_default();
    // Tailscale's own servers hand out names under ts.net; any other suffix is a Headscale server
    let kind = if !suffix.is_empty() && suffix != "ts.net" && !suffix.ends_with(".ts.net") { Kind::Headscale } else { Kind::Tailscale };
    Some(Network { kind, ip, name })
}

/// `netbird status --json`, when it's connected.
pub fn from_netbird(status: &serde_json::Value) -> Option<Network> {
    if status.pointer("/management/connected").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let ip = status.get("netbirdIp")?.as_str()?.split('/').next()?.to_string();
    if ip.split('.').count() != 4 {
        return None;
    }
    let name = status.get("fqdn").and_then(|v| v.as_str()).map(|s| s.trim_end_matches('.').to_lowercase()).filter(|s| !s.is_empty());
    Some(Network { kind: Kind::Netbird, ip, name })
}

/// Interface names and IPv4 addresses as `ip -o -4 addr` (Linux), `ifconfig` (macOS) or
/// `ipconfig` (Windows) print them.
pub fn parse_interfaces(platform: Platform, text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match platform {
        Platform::Linux => {
            for line in text.lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() >= 4 && f[2] == "inet" {
                    out.push((f[1].to_string(), f[3].split('/').next().unwrap_or("").to_string()));
                }
            }
        }
        Platform::Windows => {
            let mut iface = String::new();
            for line in text.lines() {
                if !line.starts_with(' ') && line.trim_end().ends_with(':') {
                    iface = line.trim().trim_end_matches(':').to_string();
                    iface = iface.split_once("adapter ").map(|(_, n)| n.to_string()).unwrap_or(iface);
                } else if let Some((k, v)) = line.split_once(':') {
                    if k.trim_start().starts_with("IPv4 Address") {
                        out.push((iface.clone(), v.trim().trim_end_matches("(Preferred)").trim().to_string()));
                    }
                }
            }
        }
        _ => {
            let mut iface = String::new();
            for line in text.lines() {
                if !line.starts_with([' ', '\t']) {
                    iface = line.split(':').next().unwrap_or("").to_string();
                } else if let Some(rest) = line.trim().strip_prefix("inet ") {
                    out.push((iface.clone(), rest.split_whitespace().next().unwrap_or("").to_string()));
                }
            }
        }
    }
    out.retain(|(_, ip)| !ip.starts_with("127.") && !ip.is_empty());
    out
}

/// The networks the interface list shows: private networks by name, and the LAN by address.
pub fn from_interfaces(ifaces: &[(String, String)]) -> Vec<Network> {
    let mut found = Vec::new();
    for (iface, ip) in ifaces {
        if let Some(kind) = kind_of(iface) {
            found.push(Network { kind, ip: ip.clone(), name: None });
        } else if is_lan(ip) && !found.iter().any(|n: &Network| n.kind == Kind::Lan) {
            found.push(Network { kind: Kind::Lan, ip: ip.clone(), name: None });
        }
    }
    found
}

fn json_of(cmd: &[&str]) -> Option<serde_json::Value> {
    which(cmd[0])?;
    let r = run(cmd);
    r.ok.then(|| serde_json::from_str(&r.out).ok()).flatten()
}

/// Every network this machine is on, one entry per address.
pub fn detect() -> Vec<Network> {
    let platform = Platform::current();
    let mut nets = Vec::new();
    let ts = json_of(&["tailscale", "status", "--json"]).or_else(|| {
        // the macOS app keeps its command line inside the bundle
        (platform == Platform::Darwin).then(|| json_of(&["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "status", "--json"])).flatten()
    });
    if let Some(n) = ts.as_ref().and_then(from_tailscale) {
        nets.push(n);
    }
    if let Some(n) = json_of(&["netbird", "status", "--json"]).as_ref().and_then(from_netbird) {
        nets.push(n);
    }
    let text = match platform {
        Platform::Linux => run(&["ip", "-o", "-4", "addr"]).out,
        Platform::Windows => run(&["ipconfig"]).out,
        _ => run(&["ifconfig"]).out,
    };
    for n in from_interfaces(&parse_interfaces(platform, &text)) {
        if !nets.iter().any(|m| m.ip == n.ip) {
            nets.push(n);
        }
    }
    nets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_are_read_from_each_systems_listing() {
        let linux = "1: lo    inet 127.0.0.1/8 scope host lo\n2: eth0    inet 192.168.1.20/24 brd 192.168.1.255 scope global eth0\n3: tailscale0    inet 100.64.0.9/32 scope global tailscale0\n";
        let parsed = parse_interfaces(Platform::Linux, linux);
        assert_eq!(parsed, vec![("eth0".to_string(), "192.168.1.20".to_string()), ("tailscale0".to_string(), "100.64.0.9".to_string())]);
        let nets = from_interfaces(&parsed);
        assert_eq!(nets[0], Network { kind: Kind::Lan, ip: "192.168.1.20".into(), name: None });
        assert_eq!(nets[1].kind, Kind::Tailscale);

        let mac = "lo0: flags=8049<UP,LOOPBACK>\n\tinet 127.0.0.1 netmask 0xff000000\nen0: flags=8863<UP>\n\tinet 10.0.0.5 netmask 0xffffff00 broadcast 10.0.0.255\nutun3: flags=8051<UP>\n\tinet 100.100.1.2 --> 100.100.1.2 netmask 0xffffffff\n";
        let parsed = parse_interfaces(Platform::Darwin, mac);
        assert_eq!(parsed, vec![("en0".to_string(), "10.0.0.5".to_string()), ("utun3".to_string(), "100.100.1.2".to_string())]);

        let win = "Windows IP Configuration\n\nEthernet adapter Ethernet:\n\n   IPv4 Address. . . . . . . . . . . : 192.168.0.7(Preferred)\n\nUnknown adapter Tailscale:\n\n   IPv4 Address. . . . . . . . . . . : 100.80.1.1\n";
        let parsed = parse_interfaces(Platform::Windows, win);
        assert_eq!(parsed, vec![("Ethernet".to_string(), "192.168.0.7".to_string()), ("Tailscale".to_string(), "100.80.1.1".to_string())]);
        assert_eq!(from_interfaces(&parsed)[1].kind, Kind::Tailscale);
    }

    #[test]
    fn tailscale_and_netbird_answers() {
        let ts: serde_json::Value = serde_json::from_str(r#"{"BackendState":"Running","Self":{"TailscaleIPs":["100.64.0.9","fd7a::1"],"DNSName":"laptop.tail1234.ts.net."},"CurrentTailnet":{"MagicDNSSuffix":"tail1234.ts.net"}}"#).unwrap();
        let n = from_tailscale(&ts).unwrap();
        assert_eq!(n, Network { kind: Kind::Tailscale, ip: "100.64.0.9".into(), name: Some("laptop.tail1234.ts.net".into()) });
        let hs: serde_json::Value =
            serde_json::from_str(r#"{"BackendState":"Running","Self":{"TailscaleIPs":["100.64.0.9"]},"CurrentTailnet":{"MagicDNSSuffix":"example.org"}}"#).unwrap();
        assert_eq!(from_tailscale(&hs).unwrap().kind, Kind::Headscale);
        // a Headscale suffix that merely ends in the letters ts.net is still Headscale
        let lookalike: serde_json::Value =
            serde_json::from_str(r#"{"BackendState":"Running","Self":{"TailscaleIPs":["100.64.0.9"]},"CurrentTailnet":{"MagicDNSSuffix":"notts.net"}}"#).unwrap();
        assert_eq!(from_tailscale(&lookalike).unwrap().kind, Kind::Headscale);
        let stopped: serde_json::Value = serde_json::from_str(r#"{"BackendState":"Stopped"}"#).unwrap();
        assert!(from_tailscale(&stopped).is_none());
        let nb: serde_json::Value = serde_json::from_str(r#"{"management":{"connected":true},"netbirdIp":"100.90.0.3/16","fqdn":"box.netbird.cloud."}"#).unwrap();
        assert_eq!(from_netbird(&nb).unwrap().name.unwrap(), "box.netbird.cloud");
        assert_eq!(from_netbird(&nb).unwrap().ip, "100.90.0.3");
    }
}
