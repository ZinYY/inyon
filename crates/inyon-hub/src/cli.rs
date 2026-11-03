//! `inyon-engine hub`, as the Bun engine's hub.ts answered it, run by the inyon command. Its settings (PORT, BIND, TLS_CERT and TLS_KEY, INYON_ALLOW_HTTP…) can go
//! in ~/.inyon/hub.env, one NAME=value a line, so the background service picks them up too.
//!
//!   hub                 run it here
//!   hub code            show the setup code a new hub asks for
//!   hub companion-code  a one-time code for a companion app (Legato) to pair with
//!   hub reset-password  choose a new password when the old one is forgotten

use std::io::{BufRead, Write};
use std::path::PathBuf;

use inyon_engine::output::{emit, fail};
use serde_json::{Map, Value};

fn data_dir() -> PathBuf {
    std::env::var("INYON_DATA_DIR").ok().filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| inyon_engine::paths::home().join(".inyon"))
}

pub fn hub_env_file() -> PathBuf {
    data_dir().join("hub.env")
}

/// Puts what hub.env holds into this process's environment where nothing is set yet (`??=`), and
/// each TETHER_* variable under its INYON_* name. Called before any other thread starts.
pub fn load_env() {
    for (k, v) in inyon_engine::paths::read_env_file(&hub_env_file()) {
        if std::env::var_os(&k).is_none() {
            std::env::set_var(k, v);
        }
    }
    let old: Vec<(String, String)> = std::env::vars().filter_map(|(k, v)| k.strip_prefix("TETHER_").map(|r| (format!("INYON_{r}"), v))).collect();
    for (k, v) in old {
        if std::env::var_os(&k).is_none() {
            std::env::set_var(k, v);
        }
    }
}

/// `inyon-engine hub [code|reset-password]`
pub fn run(args: &[String]) -> i32 {
    load_env();
    match args.first().map(String::as_str) {
        Some("code") => show_setup_code(),
        Some("companion-code") => show_companion_code(),
        Some("reset-password") => reset_password(args.get(1).cloned()),
        Some(_) => {
            eprintln!("usage: inyon-engine hub [code|companion-code|reset-password]");
            2
        }
        None => {
            // for `inyon update`: it waits for this to name the new version
            let _ = std::fs::create_dir_all(inyon_engine::paths::node_dir());
            let running = serde_json::json!({ "pid": std::process::id(), "version": inyon_protocol::VERSION });
            let _ = std::fs::write(inyon_engine::paths::running_file(inyon_engine::paths::Role::Hub), running.to_string());
            crate::start::serve()
        }
    }
}

/// The hub's own address on this machine, for asking it questions.
fn local_url() -> String {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let tls = get("TLS_CERT").is_some() && get("TLS_KEY").is_some();
    let bind = get("BIND").unwrap_or_else(|| "127.0.0.1".into());
    let host = if ["0.0.0.0", "::", "127.0.0.1"].contains(&bind.as_str()) { "127.0.0.1".to_owned() } else { bind };
    format!("{}://{host}:{}", if tls { "https" } else { "http" }, get("PORT").unwrap_or_else(|| "4777".into()))
}

/// What `inyon hub code` says: the code while the hub still waits for its first password, and
/// otherwise what to do instead.
pub fn setup_code_message(code: Option<&str>, session: Option<&Value>, url: &str) -> String {
    let setup = |s: &Value| crate::http::truthy(s.get("setup"));
    match (code, session) {
        (_, Some(s)) if !setup(s) => "This hub has a password already, so there is no setup code. Forgotten it? Run `inyon hub reset-password`.".into(),
        (Some(c), Some(_)) => format!("Setup code: {c}\nOpen {url} in a browser and enter it to choose a password."),
        (Some(c), None) => {
            format!("Setup code: {c}\nThe hub isn't answering right now; start it (`inyon hub` or `inyon hub start`), then open it in a browser and enter the code.")
        }
        (None, _) => "No setup code here: the hub has not started yet. Run `inyon hub` or `inyon hub start`; it makes a code as it starts.".into(),
    }
}

fn show_setup_code() -> i32 {
    let url = local_url();
    let code = std::fs::read_to_string(data_dir().join("setup-code")).ok().map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
    let session = inyon_engine::http::get_local(&format!("{url}/api/session"), std::time::Duration::from_secs(5)).ok().and_then(|r| serde_json::from_slice::<Value>(&r.body).ok());
    let message = setup_code_message(code.as_deref(), session.as_ref(), &url);
    let has_password = session.as_ref().map(|s| !crate::http::truthy(s.get("setup")));
    let mut out = Map::new();
    if has_password != Some(true) {
        if let Some(c) = &code {
            out.insert("code".into(), Value::String(c.clone()));
        }
    }
    out.insert("url".into(), Value::String(url.clone()));
    out.insert("running".into(), Value::Bool(session.is_some()));
    if let Some(h) = has_password {
        out.insert("hasPassword".into(), Value::Bool(h));
    }
    out.insert("message".into(), Value::String(message.clone()));
    emit(Value::Object(out), Some(&message));
    0
}

/// "K7M2QX" as it is shown: K7M-2QX.
pub fn show_code(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() {
        format!("{}-{}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

/// What `inyon hub companion-code` says once the hub made a code. A hub on loopback sits behind a
/// proxy or `tailscale serve`, whose address this program doesn't know, so it names none then.
pub fn companion_code_message(code: &str, url: &str) -> String {
    let host = url.split("://").nth(1).unwrap_or(url);
    let address = if host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[::1]") {
        "the address you open this hub at".to_owned()
    } else {
        format!("this hub's address, {url}")
    };
    format!("Companion code: {}\nType it into the companion app (Legato: Settings > Connections > Inyon) with {address}. It works once, for 5 minutes.", show_code(code))
}

/// `inyon hub companion-code`: asks the running hub for a companion code over its own address,
/// with the token it wrote for the inyon command on this machine.
fn show_companion_code() -> i32 {
    let url = local_url();
    let token = std::fs::read_to_string(data_dir().join("local-token")).map(|t| t.trim().to_owned()).unwrap_or_default();
    if token.is_empty() {
        fail("The hub hasn't started here yet. Run `inyon hub` or `inyon hub start` first.", 1);
    }
    let auth = format!("Bearer {token}");
    let Ok(res) = inyon_engine::http::post_local(&format!("{url}/api/local/companion-code"), &[("authorization", &auth)], std::time::Duration::from_secs(5)) else {
        fail(&format!("The hub isn't answering at {url}. Start it with `inyon hub start`."), 1);
    };
    if res.status == 404 {
        fail("This hub is too old for companion apps. Update it with `inyon update`.", 1);
    }
    let body: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
    let Some(code) = body.get("code").and_then(Value::as_str) else {
        let said = body.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("The hub answered {}.", res.status));
        fail(&said, 1);
    };
    let message = companion_code_message(code, &url);
    let mut out = Map::new();
    out.insert("code".into(), Value::String(code.to_owned()));
    if let Some(e) = body.get("expiresAt") {
        out.insert("expiresAt".into(), e.clone());
    }
    // contract 1.1: the certificate a companion can pin, when the hub serves https itself
    if let Some(f) = body.get("tlsFingerprint") {
        out.insert("tlsFingerprint".into(), f.clone());
    }
    out.insert("url".into(), Value::String(url));
    out.insert("message".into(), Value::String(message.clone()));
    emit(Value::Object(out), Some(&message));
    0
}

/// A new password for a forgotten one, written straight to the hub's records; a running hub has
/// to restart to pick it up. It comes as an argument, in INYON_NEW_PASSWORD, or from a prompt.
fn reset_password(given: Option<String>) -> i32 {
    let env = inyon_backends::env::HubEnv::current();
    let pw = given.or_else(|| std::env::var("INYON_NEW_PASSWORD").ok()).unwrap_or_else(|| {
        print!("New Inyon password: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        line.trim_end_matches(['\r', '\n']).to_owned()
    });
    if let Err(e) = reset_password_in(&env.data_dir(), &pw) {
        fail(&e, 1);
    }
    let message = "Password updated; everyone was signed out. Restart the hub to use it (`inyon hub restart`).";
    emit(serde_json::json!({ "message": message }), Some(message));
    0
}

/// Sets the password in the hub's records under `dir`.
fn reset_password_in(dir: &std::path::Path, pw: &str) -> Result<(), String> {
    if let Some(problem) = crate::password::password_problem(pw) {
        return Err(problem.to_owned());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't open {}: {e}", dir.display()))?;
    let store = inyon_backends::fs_store::FsStore::new(dir).map_err(|e| format!("Can't open {}: {e}", dir.display()))?;
    // the records belong to the core's thread, which this one becomes for the change
    inyon_core::platform::rt::install(std::rc::Rc::new(inyon_backends::runtime::TokioRuntime));
    inyon_core::platform::store::use_store(std::rc::Rc::new(store));
    crate::password::init();
    crate::password::set_password(pw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_password_is_set_from_the_command_line() {
        // 0.20's reset-password stopped in a debug build: the records were used off the core's thread
        let dir = std::env::temp_dir().join(format!("inyon-hub-reset-{}", std::process::id()));
        assert!(reset_password_in(&dir, "x").is_err(), "too short");
        reset_password_in(&dir, "a long enough password").unwrap();
        // read back from the records on disk
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("hub.json")).unwrap()).unwrap();
        let salt = saved["salt"].as_str().unwrap();
        assert_eq!(saved["passwordHash"].as_str(), Some(crate::password::hash_password("a long enough password", salt).as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_companion_code_is_shown_in_two_halves() {
        assert_eq!(show_code("K7M2QX"), "K7M-2QX");
        assert_eq!(show_code("short"), "short");
        assert_eq!(
            companion_code_message("K7M2QX", "http://100.97.0.1:8787"),
            "Companion code: K7M-2QX\nType it into the companion app (Legato: Settings > Connections > Inyon) with this hub's address, http://100.97.0.1:8787. It works once, for 5 minutes."
        );
        // behind a proxy the loopback address would reach nothing from the app's computer
        assert!(companion_code_message("K7M2QX", "http://127.0.0.1:4777").contains("with the address you open this hub at."));
    }

    #[test]
    fn the_setup_code_message_says_what_to_do() {
        let url = "http://127.0.0.1:4777";
        assert!(setup_code_message(Some("abcd1234"), Some(&serde_json::json!({ "setup": false })), url).starts_with("This hub has a password already"));
        assert_eq!(
            setup_code_message(Some("abcd1234"), Some(&serde_json::json!({ "setup": true })), url),
            format!("Setup code: abcd1234\nOpen {url} in a browser and enter it to choose a password.")
        );
        assert!(setup_code_message(Some("abcd1234"), None, url).contains("The hub isn't answering right now"));
        assert!(setup_code_message(None, None, url).starts_with("No setup code here"));
    }
}
