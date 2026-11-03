//! The hub starting, as index.ts's top level does: refuse plain http on a network address, the
//! first-run setup code, the core and its timers, the local token, the listener, the signals that
//! save everything before it stops.

use std::path::Path;
use std::rc::Rc;

use inyon_backends::env::{install_hub, HubEnv};
use inyon_core::platform::console;
use inyon_core::platform::fetch::{FetchRequest, FetchResponse, Fetcher};

use crate::server::{install_ctx, CtxInit};

/// The core's `fetch` (npm's registry, OpenCode's catalogue) over inyon-engine's HTTP client.
struct NetFetcher;

impl Fetcher for NetFetcher {
    fn fetch(&self, req: FetchRequest) -> futures_util::future::LocalBoxFuture<'static, Result<FetchResponse, String>> {
        Box::pin(async move {
            let method: &'static str = match req.method.as_str() {
                "POST" => "POST",
                "PUT" => "PUT",
                "PATCH" => "PATCH",
                "DELETE" => "DELETE",
                "HEAD" => "HEAD",
                _ => "GET",
            };
            let r = inyon_engine::http::fetch(method, req.url, req.headers, req.body, std::time::Duration::from_millis(req.timeout_ms)).await?;
            Ok(FetchResponse { status: r.status, body: r.body })
        })
    }
}

fn write_private(p: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(p);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p)?;
    f.write_all(text.as_bytes())
}

fn truthy_flag(v: Option<String>) -> bool {
    v.is_some_and(|v| ["1", "true", "yes"].contains(&v.to_ascii_lowercase().as_str()))
}

/// Runs the hub until it's stopped; the process's exit code.
pub fn serve() -> i32 {
    let env = HubEnv::current();
    let (bind, port) = (env.bind(), env.port());
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let tls_files = get("TLS_CERT").zip(get("TLS_KEY"));
    let exposed = !(bind.starts_with("127.") || bind == "::1" || bind == "localhost");
    // on a network address, plain http sends the password and session in the clear: only on
    // purpose (say on a VPN address, which encrypts on its own), with INYON_ALLOW_HTTP=1
    if exposed && tls_files.is_none() && !truthy_flag(get("INYON_ALLOW_HTTP")) {
        eprintln!("Refusing to serve plain http on {bind}: set TLS_CERT and TLS_KEY, keep BIND on 127.0.0.1 behind an HTTPS proxy, or set INYON_ALLOW_HTTP=1 if the network encrypts it already (Tailscale, NetBird, WireGuard).");
        return 1;
    }
    let tls = match &tls_files {
        Some((cert, key)) => match crate::tls::config(cert, key) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("Can't serve HTTPS: {e}");
                return 1;
            }
        },
        None => None,
    };
    #[cfg(unix)]
    inyon_backends::limits::raise_open_files();
    inyon_backends::runtime::run_core(|| async move {
        let hostname = inyon_engine::hostname();
        let data_dir = match install_hub(&env, &hostname) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Can't open the hub's records: {e}");
                return 1;
            }
        };
        inyon_core::platform::fetch::use_fetcher(Some(Rc::new(NetFetcher)));
        crate::password::init();
        crate::auth::init();
        crate::phones::init();
        crate::companions::init();
        crate::pages::init(inyon_engine::paths::home());
        // messages held before a restart go once their agents are free
        inyon_core::claude::held::start();

        // A new hub has no password: the first visit creates one, with a one-time code from the
        // hub's log. The code is asked for even on localhost, since a page in your browser can
        // reach localhost too.
        let setup_file = data_dir.join("setup-code");
        let setup_code = (!crate::password::has_password()).then(|| crate::http::random_hex(4));
        if let Some(code) = &setup_code {
            if let Err(e) = write_private(&setup_file, &format!("{code}\n")) {
                eprintln!("Can't write {}: {e}", setup_file.display());
                return 1;
            }
        }

        // the inyon command on this machine reads the hub's machines with this token, readable
        // only by this user; whoever can read it can read the hub's records anyway
        let local_token = crate::http::random_base64url(24);
        if let Err(e) = write_private(&data_dir.join("local-token"), &format!("{local_token}\n")) {
            eprintln!("Can't write the local token: {e}");
            return 1;
        }

        let repo = crate::hub_update::checkout();
        let commit = crate::hub_update::head_commit(repo.as_deref());
        let dist = crate::node_dist::Dist::new(repo.clone(), &data_dir);
        dist.install();
        let updater = crate::hub_update::HubUpdater::new(crate::hub_update::Options::new(repo, data_dir.clone(), commit.clone()));
        let core = inyon_backends::core_here();
        install_ctx(
            CtxInit {
                data_dir: data_dir.clone(),
                bind: bind.clone(),
                port,
                tls: tls.is_some(),
                allowed_hosts: get("INYON_ALLOWED_HOSTS"),
                allowed_origins: get("INYON_ALLOWED_ORIGINS"),
                hostname,
                setup_code: setup_code.clone(),
                local_token,
                hub_commit: commit,
                core: core.clone(),
            },
            crate::index::routes(updater),
        );
        crate::core_routes::start_core();
        crate::clients::poll_less_when_unwatched();
        crate::push::start_hub_push();
        crate::boards::start();
        crate::pages_stream::start();
        crate::finding::start();

        // this machine's names on its private networks count as the hub's own
        let names = crate::server::ctx().network_names.clone();
        inyon_core::platform::rt::spawn(async move {
            for n in inyon_engine::networks::detect_networks().await {
                if let Some(name) = n.name {
                    names.borrow_mut().insert(name);
                }
            }
        });

        let listener = match tokio::net::TcpListener::bind((bind.as_str(), port)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Can't listen on {bind}:{port}: {e}");
                return 1;
            }
        };
        let scheme = if tls.is_some() { "https" } else { "http" };
        crate::server::serve(listener, tls, core);
        console::log(&format!("Inyon hub listening on {scheme}://{bind}:{port}"));
        if !crate::static_files::built() {
            console::log("This hub was built without the web app (apps/web/dist); its pages say so until it's rebuilt.");
        }
        dist.prebuild();
        if !crate::password::has_password() {
            let host = if exposed { bind.as_str() } else { "localhost" };
            console::log(&format!("\n  First run: open {scheme}://{host}:{port} and create your password."));
            if let Some(code) = &setup_code {
                use std::io::IsTerminal;
                console::log(&setup_code_note(code, &setup_file, std::io::stdout().is_terminal()));
            }
        }

        // a restart (systemctl, Ctrl+C) keeps what was about to be saved: open chats, drafts, machines
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            std::future::pending::<()>().await;
            return 0;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        crate::core_routes::save_everything();
        0
    })
}

/// What the first start says about the setup code. The code itself only to a terminal: a log
/// (launchd's, nohup's, the journal) may be readable by other users of this machine, who could set
/// the password before you do.
fn setup_code_note(code: &str, file: &std::path::Path, terminal: bool) -> String {
    if terminal {
        format!("  It will ask for this setup code: {code} (also in {})\n", file.display())
    } else {
        format!("  It will ask for the setup code in {} (`inyon hub code` shows it)\n", file.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_code_goes_to_a_terminal_and_only_its_file_to_a_log() {
        let file = std::path::Path::new("/home/me/.inyon/setup-code");
        assert!(setup_code_note("1a2b3c4d", file, true).contains("1a2b3c4d"));
        let logged = setup_code_note("1a2b3c4d", file, false);
        assert!(!logged.contains("1a2b3c4d") && logged.contains("/home/me/.inyon/setup-code") && logged.contains("inyon hub code"), "{logged}");
    }
}
