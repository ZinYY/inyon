//! Spike r0-s5: a sketch, not built. On its first start on Android the Dioxus app picks up what the
//! last Capacitor build sealed into files/inyon-migrate.bin (see MigrationExport.kt). Java opens the
//! file with the Keystore key; Rust only ever sees the plaintext JSON.
//!
//! Crates: serde 1 (derive), serde_json 1, base64 0.22, ed25519-dalek 2, zeroize 1, and on Android
//! jni 0.21 and ndk-context 0.1.

use std::collections::{BTreeMap, HashSet};

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use zeroize::Zeroizing;

pub const PAYLOAD_VERSION: u32 = 1;

/// The export, field for field. Everything but `secrets` comes from the web app; Kotlin adds
/// `secrets` from inyon_secrets. Unknown fields are ignored so a later R5.x can add some.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub v: u32,
    /// ms since the epoch
    pub exported_at: u64,
    pub app_version: String,
    /// IndexedDB "inyon" / kv / "hosts": laptops paired over the link, and SSH servers
    #[serde(default)]
    pub hosts: Vec<HostConfig>,
    /// kv "names": session names keyed "<hostId> <sessionId>"
    #[serde(default)]
    pub names: Option<serde_json::Value>,
    /// kv "settings" (the core's HubSettings: uploads, keepSession, autoResume)
    #[serde(default)]
    pub core_settings: Option<serde_json::Value>,
    /// localStorage "inyon.hubs.v1"
    #[serde(default)]
    pub hubs: Vec<PairedHub>,
    /// localStorage "inyon.settings.v2" (theme, fonts, haptics...)
    #[serde(default)]
    pub ui_settings: Option<serde_json::Value>,
    /// inyon_secrets as it is: "phone-key" and "server:<hostId>", each a JSON string
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

/// packages/protocol/src/index.ts HostConfig, the fields a phone writes.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    pub id: String,
    pub name: String,
    pub kind: HostKind,
    /// ssh: "user@host", or with `via` an alias from that computer's ~/.ssh/config
    pub target: Option<String>,
    pub port: Option<u16>,
    /// ssh: "SHA256:<base64>", the server key fingerprint the user confirmed (the phone's known_hosts)
    pub host_key: Option<String>,
    pub auth: Option<Auth>,
    /// ssh: id of the paired laptop that opens the connection instead of the phone
    pub via: Option<String>,
    /// link: the laptop's id, z-base-32 of its ed25519 public key (52 chars)
    pub machine_id: Option<String>,
    /// link: ws:// or wss:// addresses that worked last
    #[serde(default)]
    pub urls: Vec<String>,
    /// link: relay origins, https://...
    #[serde(default)]
    pub relays: Vec<String>,
    pub keep_integration: Option<bool>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    Local,
    Ssh,
    Node,
    Link,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Auth {
    PhoneKey,
    Password,
    Key,
    None,
}

/// apps/web/src/lib/hubs.ts PairedHub. `token` is the bearer token the hub gave at pairing.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PairedHub {
    pub id: String,
    pub name: String,
    pub url: String,
    pub token: String,
    pub version: Option<String>,
    pub paired_at: u64,
}

/// "phone-key": `{"seed":"<base64 32 bytes>","pub":"<base64 32 bytes>"}`, standard base64.
#[derive(Deserialize)]
struct PhoneKey {
    seed: String,
    #[serde(rename = "pub")]
    public: String,
}

/// "server:<hostId>": a password, or a pasted OpenSSH/PEM private key and its passphrase.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerSecret {
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
}

/// What the importer commits to the new store.
pub struct Imported {
    /// The phone's one ed25519 seed: its SSH key, and (as X25519) its Noise static key.
    pub seed: Option<Zeroizing<[u8; 32]>>,
    pub hosts: Vec<HostConfig>,
    pub server_secrets: BTreeMap<String, ServerSecret>,
    pub hubs: Vec<PairedHub>,
    pub names: Option<serde_json::Value>,
    pub core_settings: Option<serde_json::Value>,
    pub ui_settings: Option<serde_json::Value>,
    /// hosts left out, with why, for the log and a one-line notice
    pub skipped: Vec<(String, &'static str)>,
}

#[derive(Debug)]
pub enum ImportError {
    Json(serde_json::Error),
    Version(u32),
    Bad(&'static str),
}

impl From<serde_json::Error> for ImportError {
    fn from(e: serde_json::Error) -> Self {
        ImportError::Json(e)
    }
}

pub fn parse(plaintext: &[u8]) -> Result<Imported, ImportError> {
    let export: Export = serde_json::from_slice(plaintext)?;
    validate(export)
}

fn validate(e: Export) -> Result<Imported, ImportError> {
    if e.v != PAYLOAD_VERSION {
        return Err(ImportError::Version(e.v));
    }

    // the identity: all or nothing. A seed that doesn't match its public key is a corrupt export,
    // and importing it would leave the phone unknown to every laptop and server.
    let seed = match e.secrets.get("phone-key") {
        None => None,
        Some(raw) => {
            let k: PhoneKey = serde_json::from_str(raw)?;
            let seed = Zeroizing::new(decode32(&k.seed).ok_or(ImportError::Bad("phone-key seed is not 32 bytes of base64"))?);
            let public = decode32(&k.public).ok_or(ImportError::Bad("phone-key pub is not 32 bytes of base64"))?;
            if SigningKey::from_bytes(&seed).verifying_key().to_bytes() != public {
                return Err(ImportError::Bad("phone-key pub does not belong to its seed"));
            }
            Some(seed)
        }
    };

    let mut ids = HashSet::new();
    let mut skipped = Vec::new();
    let mut server_secrets = BTreeMap::new();
    let link_ids: HashSet<&str> = e.hosts.iter().filter(|h| h.kind == HostKind::Link).map(|h| h.id.as_str()).collect();
    let mut hosts = Vec::new();

    for h in &e.hosts {
        if h.id.is_empty() || !ids.insert(h.id.clone()) {
            return Err(ImportError::Bad("a host id is empty or repeated"));
        }
        let why = match h.kind {
            // a hub's kinds; a phone never writes them
            HostKind::Local | HostKind::Node => Some("not a phone host"),
            HostKind::Link if seed.is_none() => Some("paired without a phone key"),
            HostKind::Link => match &h.machine_id {
                // R1's Rust port of packages/link/src/identity.ts isMachineId (canonical z-base-32)
                Some(m) if inyon_link::is_machine_id(m) => {
                    if h.urls.iter().all(|u| u.starts_with("ws://") || u.starts_with("wss://"))
                        && h.relays.iter().all(|r| r.starts_with("https://") || r.starts_with("http://"))
                    {
                        None
                    } else {
                        Some("bad address")
                    }
                }
                _ => Some("bad machine id"),
            },
            HostKind::Ssh => ssh_problem(h, &link_ids, seed.is_some(), &e.secrets, &mut server_secrets)?,
        };
        match why {
            Some(why) => skipped.push((h.id.clone(), why)),
            None => hosts.push(h.clone()),
        }
    }

    let mut hub_ids = HashSet::new();
    let hubs = e
        .hubs
        .into_iter()
        .filter(|h| {
            !h.id.is_empty()
                && !h.token.is_empty()
                && (h.url.starts_with("https://") || h.url.starts_with("http://"))
                && hub_ids.insert(h.id.clone())
        })
        .collect();

    Ok(Imported {
        seed,
        hosts,
        server_secrets,
        hubs,
        names: e.names,
        core_settings: e.core_settings,
        ui_settings: e.ui_settings,
        skipped,
    })
}

/// None when the server can be used as it is; Some(why) to leave it out.
fn ssh_problem(
    h: &HostConfig,
    link_ids: &HashSet<&str>,
    have_seed: bool,
    secrets: &BTreeMap<String, String>,
    out: &mut BTreeMap<String, ServerSecret>,
) -> Result<Option<&'static str>, ImportError> {
    if h.target.as_deref().unwrap_or("").is_empty() {
        return Ok(Some("no target"));
    }
    // through a laptop: that laptop's ssh signs in and checks the key, so nothing else is stored
    if let Some(via) = &h.via {
        return Ok(if link_ids.contains(via.as_str()) { None } else { Some("via an unknown computer") });
    }
    if !h.host_key.as_deref().unwrap_or("").starts_with("SHA256:") {
        return Ok(Some("no confirmed host key"));
    }
    Ok(match h.auth.unwrap_or(Auth::PhoneKey) {
        Auth::None => None,
        Auth::PhoneKey => (!have_seed).then_some("phone-key sign-in without a phone key"),
        auth @ (Auth::Password | Auth::Key) => {
            let Some(raw) = secrets.get(&format!("server:{}", h.id)) else {
                return Ok(Some("sign-in secret missing"));
            };
            let s: ServerSecret = serde_json::from_str(raw)?;
            let ok = match auth {
                Auth::Password => s.password.is_some(),
                _ => s.private_key.as_deref().is_some_and(|k| !k.is_empty()),
            };
            if ok {
                out.insert(h.id.clone(), s);
                None
            } else {
                Some("sign-in secret incomplete")
            }
        }
    })
}

fn decode32(b64: &str) -> Option<[u8; 32]> {
    STANDARD.decode(b64).ok()?.try_into().ok()
}

// ---- Android: the Kotlin side, over JNI ---------------------------------------------------------

#[cfg(target_os = "android")]
pub mod android {
    use jni::objects::{JByteArray, JClass, JObject, JValue};
    use jni::{JNIEnv, JavaVM};
    use zeroize::Zeroizing;

    const CLASS: &str = "tech.abhashchakraborty.inyon.migrate.MigrationExport";

    /// MigrationExport.read(context): the plaintext, or None when there is nothing to import.
    pub fn read() -> jni::errors::Result<Option<Zeroizing<Vec<u8>>>> {
        with_env(|env, context| {
            let class = app_class(env, context)?;
            let out = env
                .call_static_method(&class, "read", "(Landroid/content/Context;)[B", &[JValue::Object(context)])?
                .l()?;
            if out.is_null() {
                return Ok(None);
            }
            let array = JByteArray::from(out);
            Ok(Some(Zeroizing::new(env.convert_byte_array(&array)?)))
        })
    }

    /// MigrationExport.discard(context): deletes the file and the Keystore key.
    pub fn discard() -> jni::errors::Result<()> {
        with_env(|env, context| {
            let class = app_class(env, context)?;
            env.call_static_method(&class, "discard", "(Landroid/content/Context;)V", &[JValue::Object(context)])?;
            Ok(())
        })
    }

    fn with_env<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> jni::errors::Result<T> {
        let ctx = ndk_context::android_context();
        // SAFETY: ndk-context hands out the process's JavaVM and a global ref to the app's Context,
        // both alive for the life of the process; JObject::from_raw doesn't take ownership.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }?;
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let mut env = vm.attach_current_thread()?;
        let result = f(&mut env, &context);
        // a Java exception left pending poisons every later JNI call on this thread
        if env.exception_check()? {
            env.exception_describe()?;
            env.exception_clear()?;
        }
        result
    }

    /// FindClass on a thread Rust started only sees system classes, so ask the app's class loader.
    fn app_class<'local>(env: &mut JNIEnv<'local>, context: &JObject) -> jni::errors::Result<JClass<'local>> {
        let loader = env.call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string(CLASS)?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[(&name).into()])?
            .l()?;
        Ok(JClass::from(class))
    }
}

// ---- first start ----------------------------------------------------------------------------------

/// The new app's own store, as far as the import cares.
pub trait Store {
    fn has_identity(&self) -> bool;
    /// Writes everything in one transaction, secrets to the Keystore-backed side.
    fn commit(&mut self, imported: Imported) -> std::io::Result<()>;
}

pub enum Outcome {
    Imported { skipped: usize },
    /// already imported (a crash between commit and discard): the file only needed deleting
    AlreadyDone,
    /// no file, or one this phone can't open: fall back (see REPORT.md)
    Nothing,
    Failed(String),
}

#[cfg(target_os = "android")]
pub fn first_start(store: &mut impl Store) -> Outcome {
    if store.has_identity() {
        let _ = android::discard();
        return Outcome::AlreadyDone;
    }
    let plaintext = match android::read() {
        Ok(Some(p)) => p,
        Ok(None) => return Outcome::Nothing,
        Err(e) => return Outcome::Failed(format!("reading the export: {e}")),
    };
    let imported = match parse(&plaintext) {
        Ok(i) => i,
        // keep the file: a later build may read what this one can't
        Err(e) => return Outcome::Failed(format!("{e:?}")),
    };
    let skipped = imported.skipped.len();
    if let Err(e) = store.commit(imported) {
        return Outcome::Failed(format!("saving the import: {e}"));
    }
    // only now: until the commit holds, the file is the only copy outside WebView storage
    let _ = android::discard();
    Outcome::Imported { skipped }
}
