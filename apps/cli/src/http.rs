//! The few HTTP requests the program makes: the release's SHA256SUMS and files, the newest
//! version (latest.json, or on GitHub a redirect it doesn't follow), and the hub next door.

use std::time::Duration;

use inyon_engine::http::Limits;

pub struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Every request has the engine's limits: 10 s to connect (a dead address gives way to the next),
/// 30 s without a byte, and `total` in all. A request to the hub joined takes its pinned
/// certificate (inyon_engine::hub_tls) when join kept one.
fn agent(url: &str, follow: bool, insecure: bool) -> ureq::Agent {
    let mut c = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(if follow { 10 } else { 0 })
        .max_redirects_will_error(false)
        .user_agent(format!("inyon/{}", env!("CARGO_PKG_VERSION")));
    c = c.tls_config(if insecure {
        // the hub on this machine may use a certificate it issued itself
        ureq::tls::TlsConfig::builder().disable_verification(true).build()
    } else {
        // Mozilla's roots, and the ones NODE_EXTRA_CA_CERTS adds
        ureq::tls::TlsConfig::builder().root_certs(inyon_engine::http::ureq_roots()).build()
    });
    let limits = Limits::new(Duration::from_secs(600));
    if insecure {
        inyon_engine::http::timed_agent(c, limits)
    } else {
        inyon_engine::http::timed_agent_for(url, c, limits)
    }
}

/// More than any release file (the engine is about 120 MB) and less than would fill memory.
const MAX_BODY: u64 = 1024 * 1024 * 1024;
/// What a Content-Length may reserve up front; a bigger body grows into it as it arrives.
const MAX_RESERVE: u64 = 64 * 1024 * 1024;

/// Private forks use the already authenticated gh CLI. Only URLs within the explicitly
/// configured repository are handled here; credentials never go to release asset hosts.
fn github_path<'a>(url: &'a str, repo: &str) -> Option<&'a str> {
    if repo.split('/').count() != 2 || repo.split('/').any(|p| p.is_empty() || p == "." || p == ".." || !p.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))) { return None; }
    url.strip_prefix(&format!("https://github.com/{repo}/releases/"))
}

fn github_release(url: &str) -> Option<Result<Response, String>> {
    let repo = crate::paths::env_var("INYON_GITHUB_REPO")?;
    let path = github_path(url, &repo)?;
    Some((|| {
        let (endpoint, asset) = if path == "latest" {
            (format!("repos/{repo}/releases/latest"), None)
        } else if let Some(name) = path.strip_prefix("latest/download/") {
            (format!("repos/{repo}/releases/latest"), Some(name))
        } else if let Some((tag, name)) = path.strip_prefix("download/").and_then(|s| s.split_once('/')) {
            (format!("repos/{repo}/releases/tags/{tag}"), Some(name))
        } else { return Err("Unsupported private release URL".into()); };
        let run = |args: &[&str]| -> Result<Vec<u8>, String> {
            let out = std::process::Command::new("gh").args(args).output().map_err(|e| format!("Private releases require gh: {e}"))?;
            if !out.status.success() { return Err("Could not download the private release; check gh auth status and the proxy.".into()); }
            Ok(out.stdout)
        };
        let release: serde_json::Value = serde_json::from_slice(&run(&["api", &endpoint])?).map_err(|e| e.to_string())?;
        let tag = release["tag_name"].as_str().ok_or("Release has no tag")?;
        let Some(name) = asset else {
            return Ok(Response { status: 302, location: Some(format!("https://github.com/{repo}/releases/tag/{tag}")), body: vec![] });
        };
        let id = release["assets"].as_array().and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(name))).and_then(|a| a["id"].as_u64());
        let Some(id) = id else { return Ok(Response { status: 404, location: None, body: vec![] }); };
        let body = run(&["api", &format!("repos/{repo}/releases/assets/{id}"), "-H", "Accept: application/octet-stream"])?;
        Ok(Response { status: 200, location: None, body })
    })())
}

fn fetch(url: &str, follow: bool, insecure: bool, progress: Option<&dyn Fn(u64, Option<u64>)>) -> Result<Response, String> {
    if let Some(result) = github_release(url) { return result; }
    let mut res = agent(url, follow, insecure).get(url).call().map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let location = res.headers().get("location").and_then(|v| v.to_str().ok()).map(String::from);
    let total = res.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let body = match progress {
        None => res.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| e.to_string())?,
        Some(report) => {
            use std::io::Read;
            let mut reader = res.body_mut().with_config().limit(MAX_BODY).reader();
            let mut body = Vec::with_capacity(total.unwrap_or(0).min(MAX_RESERVE) as usize);
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n]);
                report(body.len() as u64, total);
            }
            body
        }
    };
    Ok(Response { status, location, body })
}

/// GET, following redirects.
pub fn get(url: &str) -> Result<Response, String> {
    fetch(url, true, false, None)
}

/// GET with a progress callback (bytes so far, total if known).
pub fn get_with_progress(url: &str, progress: &dyn Fn(u64, Option<u64>)) -> Result<Response, String> {
    fetch(url, true, false, Some(progress))
}

/// GET without following the redirect, so a `…/releases/latest` page says where it points.
pub fn get_no_redirect(url: &str) -> Result<Response, String> {
    fetch(url, false, false, None)
}

/// GET with no certificate check, for what something else vouches for (a hub's certificate, by the
/// sha256 its command names).
pub fn get_unverified(url: &str) -> Result<Response, String> {
    fetch(url, true, true, None)
}

/// GET from the hub on this machine, whose certificate may be its own.
pub fn get_local(url: &str, headers: &[(String, String)]) -> Result<Response, String> {
    let c = ureq::Agent::config_builder().http_status_as_error(false).tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build());
    let mut req = inyon_engine::http::timed_agent(c, Limits::new(Duration::from_secs(3))).get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let mut res = req.call().map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let body = res.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    Ok(Response { status, location: None, body })
}

/// A request to a hub's API with a token: the hub on this machine (`local`, whose certificate may be
/// its own) or the hub this machine joined (its pinned certificate, when join kept one).
pub fn hub_call(method: &str, url: &str, token: &str, body: Option<&serde_json::Value>, local: bool) -> Result<Response, String> {
    let agent = if local {
        let c = ureq::Agent::config_builder().http_status_as_error(false).tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build());
        inyon_engine::http::timed_agent(c, Limits::new(Duration::from_secs(15)))
    } else {
        let c = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(format!("inyon/{}", env!("CARGO_PKG_VERSION")))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(inyon_engine::http::ureq_roots()).build());
        inyon_engine::http::timed_agent_for(url, c, Limits::new(Duration::from_secs(15)))
    };
    let auth = format!("Bearer {token}");
    let mut res = match (method, body) {
        ("POST", Some(b)) => agent.post(url).header("authorization", &auth).header("content-type", "application/json").send(b.to_string()),
        ("POST", None) => agent.post(url).header("authorization", &auth).send_empty(),
        ("DELETE", _) => agent.delete(url).header("authorization", &auth).call(),
        ("GET", _) => agent.get(url).header("authorization", &auth).call(),
        _ => return Err(format!("no {method} requests to the hub")),
    }
    .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let body = res.body_mut().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())?;
    Ok(Response { status, location: None, body })
}

/// The host part of a URL, for messages.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest).to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// The request line the hub would get for each method: a DELETE once went out as a GET.
    #[test]
    fn each_method_goes_out_as_itself() {
        for (method, body) in [("GET", None), ("POST", Some(serde_json::json!({ "a": 1 }))), ("POST", None), ("DELETE", None)] {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/api/companion", l.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                // the whole request before answering: closing on a client still sending fails on macOS
                let mut length = 0;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                std::io::Read::read_exact(&mut r, &mut vec![0; length]).unwrap();
                s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}").unwrap();
                line
            });
            let res = super::hub_call(method, &url, "t", body.as_ref(), true).unwrap();
            assert_eq!(res.status, 200);
            assert_eq!(server.join().unwrap().trim_end(), format!("{method} /api/companion HTTP/1.1"));
        }
        assert!(super::hub_call("PUT", "http://127.0.0.1:9/", "t", None, true).is_err());
    }
}

#[cfg(test)]
mod private_release_tests {
    use super::*;
    #[test]
    fn private_credentials_are_scoped_to_the_exact_configured_repository() {
        assert_eq!(github_path("https://github.com/ZinYY/inyon/releases/latest", "ZinYY/inyon"), Some("latest"));
        for url in ["http://github.com/ZinYY/inyon/releases/latest", "https://github.com/other/inyon/releases/latest", "https://github.com.evil/ZinYY/inyon/releases/latest"] {
            assert!(github_path(url, "ZinYY/inyon").is_none());
        }
        assert!(github_path("https://github.com/../inyon/releases/latest", "../inyon").is_none());
    }
}
