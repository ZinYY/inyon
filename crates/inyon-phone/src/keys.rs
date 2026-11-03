//! `phoneKeys` from the TypeScript phone core (removed in 0.23): the phone's own SSH key (made on first use,
//! one for every server, so you add one line to authorized_keys) and each server's password or
//! pasted key, all in the Android keystore through the plugin.

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use inyon_core::js::json::Js;
use inyon_core::platform::rt::{spawn, Deferred};
use inyon_link::identity::Identity;
use inyon_protocol::{HostAuth, HostConfig};
use serde_json::{json, Value};

use crate::ssh::{Credentials, SshPlugin, B64};

const KEY: &str = "phone-key";

pub struct PhoneKeys {
    plugin: Rc<dyn SshPlugin>,
    mine: RefCell<Option<Deferred<Result<Identity, String>>>>,
}

impl PhoneKeys {
    pub fn new(plugin: Rc<dyn SshPlugin>) -> Rc<Self> {
        Rc::new(PhoneKeys { plugin, mine: RefCell::default() })
    }

    async fn secret(&self, key: &str) -> Result<Option<String>, String> {
        let v = self.plugin.call("getSecret", json!({ "key": key })).await.map_err(|e| e.message)?;
        Ok(Some(&v).at("value").str().map(str::to_owned))
    }

    /// The phone's key, read from the keystore or made and saved there the first time; two callers
    /// at once get the same one.
    pub async fn own(self: &Rc<Self>) -> Result<Identity, String> {
        let pending = self.mine.borrow().clone();
        let p = match pending {
            Some(p) => p,
            None => {
                let p = Deferred::new();
                *self.mine.borrow_mut() = Some(p.clone());
                let (me, out) = (self.clone(), p.clone());
                spawn(async move {
                    let got = me.load().await;
                    // a failure is tried again next time
                    if got.is_err() && me.mine.borrow().as_ref().is_some_and(|m| m.same(&out)) {
                        *me.mine.borrow_mut() = None;
                    }
                    out.resolve(got);
                });
                p
            }
        };
        p.wait().await
    }

    async fn load(&self) -> Result<Identity, String> {
        if let Some(saved) = self.secret(KEY).await? {
            let v: Value = serde_json::from_str(&saved).map_err(|e| e.to_string())?;
            let seed = B64.decode(Some(&v).at("seed").str().unwrap_or_default()).map_err(|e| e.to_string())?;
            let seed: [u8; 32] = seed.try_into().map_err(|_| "The phone's saved key is damaged".to_owned())?;
            return Ok(Identity::from_seed(seed));
        }
        let id = Identity::generate().map_err(|e| e.to_string())?;
        let value = json!({ "seed": B64.encode(id.seed), "pub": B64.encode(id.public_key) }).to_string();
        self.plugin.call("setSecret", json!({ "key": KEY, "value": value })).await.map_err(|e| e.message)?;
        Ok(id)
    }

    /// The OpenSSH public key line for authorized_keys.
    pub async fn public_line(self: &Rc<Self>) -> Result<String, String> {
        Ok(public_line(&self.own().await?.public_key))
    }

    /// What a server's connection signs in with.
    pub async fn credentials(self: &Rc<Self>, config: &HostConfig) -> Result<Credentials, String> {
        match config.auth.unwrap_or(HostAuth::PhoneKey) {
            // nothing to sign in with: the server's "none" sign-in is tried first anyway, and an
            // empty password covers an account that has none
            HostAuth::None => Ok(Credentials { password: Some(String::new()), ..Default::default() }),
            HostAuth::PhoneKey => {
                let me = self.own().await?;
                Ok(Credentials { ed25519_seed: Some(B64.encode(me.seed)), ed25519_public: Some(B64.encode(me.public_key)), ..Default::default() })
            }
            HostAuth::Password | HostAuth::Key => {
                let saved = self.secret(&format!("server:{}", config.id)).await?.ok_or("The sign-in for this server is missing. Edit it and enter it again.")?;
                serde_json::from_str(&saved).map_err(|e| e.to_string())
            }
        }
    }

    pub async fn save(&self, id: &str, c: &Credentials) -> Result<(), String> {
        let value = serde_json::to_string(c).map_err(|e| e.to_string())?;
        self.plugin.call("setSecret", json!({ "key": format!("server:{id}"), "value": value })).await.map(|_| ()).map_err(|e| e.message)
    }

    pub async fn drop_secret(&self, id: &str) {
        let _ = self.plugin.call("deleteSecret", json!({ "key": format!("server:{id}") })).await;
    }
}

/// The wire format: string "ssh-ed25519", then string <32-byte key>, each length-prefixed.
pub fn public_line(public_key: &[u8; 32]) -> String {
    let kind = b"ssh-ed25519";
    let mut blob = Vec::with_capacity(8 + kind.len() + public_key.len());
    blob.extend_from_slice(&(kind.len() as u32).to_be_bytes());
    blob.extend_from_slice(kind);
    blob.extend_from_slice(&(public_key.len() as u32).to_be_bytes());
    blob.extend_from_slice(public_key);
    format!("ssh-ed25519 {} inyon-phone", B64.encode(blob))
}
