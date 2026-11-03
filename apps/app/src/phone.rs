//! The phone core inside the app (the Android build, from 1.0): the screens' requests and their
//! socket go to it in-process (crates/inyon-phone's `native`), not to a hub over the network. The
//! platform starts the core once per process and hands it over with [`install`]; the screens may
//! come and go while it runs.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use futures_util::StreamExt;
use inyon_phone::native::{Frame, PhoneHandle};
use inyon_protocol::v2::ClientMsg2;
use inyon_protocol::ClientMsg;
use tokio::sync::watch;

use crate::api::{error_of, ApiError};
use crate::socket::{Dropped, Event};
use crate::state::{wire, wire2};

struct Installed {
    phone: PhoneHandle,
    /// where the app keeps its own settings, next to the core's records
    dir: PathBuf,
}

static PHONE: OnceLock<Installed> = OnceLock::new();

/// Whether the app is on screen; the platform says when it changes (Android: the activity's
/// start and stop).
fn visible_tx() -> &'static watch::Sender<bool> {
    static VISIBLE: OnceLock<watch::Sender<bool>> = OnceLock::new();
    VISIBLE.get_or_init(|| watch::Sender::new(false))
}

/// Hands the running core to the screens; only the first call counts.
pub fn install(phone: PhoneHandle, dir: &Path) {
    let _ = PHONE.set(Installed { phone, dir: dir.to_owned() });
}

pub fn handle() -> Option<&'static PhoneHandle> {
    PHONE.get().map(|i| &i.phone)
}

/// What a computer or hub lists this phone as: "Nothing A142", the maker then the model, unless the
/// model already starts with the maker (apps/web/src/lib/deviceName.ts).
pub fn readable_name(maker: &str, model: &str) -> String {
    let (maker, model) = (maker.trim(), model.trim());
    if model.is_empty() {
        return if maker.is_empty() { "Phone".into() } else { maker.into() };
    }
    if maker.is_empty() || model.to_lowercase().starts_with(&maker.to_lowercase()) {
        return model.into();
    }
    let mut m = maker.chars();
    let first = m.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{} {model}", m.as_str())
}

/// Where the app's own settings live, once the core is installed.
pub fn data_dir() -> Option<&'static Path> {
    PHONE.get().map(|i| i.dir.as_path())
}

fn opened_tx() -> &'static watch::Sender<Option<String>> {
    static OPENED: OnceLock<watch::Sender<Option<String>>> = OnceLock::new();
    OPENED.get_or_init(|| watch::Sender::new(None))
}

/// A path to open in the app (a tapped notification's), for the screen to follow.
pub fn open(href: &str) {
    opened_tx().send_replace(Some(href.to_owned()));
}

/// The path waiting to be opened, taken so a screen opened later doesn't open it again.
pub fn take_opened() -> Option<String> {
    let mut taken = None;
    opened_tx().send_if_modified(|v| {
        taken = v.take();
        false
    });
    taken
}

/// Says when there's a path to open ([`take_opened`]); one waiting from before counts.
pub fn opened() -> watch::Receiver<Option<String>> {
    let mut rx = opened_tx().subscribe();
    // one opened before the screen was there still counts
    if rx.borrow().is_some() {
        rx.mark_changed();
    }
    rx
}

/// Whether the app is on screen now.
pub fn is_visible() -> bool {
    *visible_tx().borrow()
}

/// The app came on screen (true) or left it: the core polls less while nobody looks.
pub fn set_visible(on: bool) {
    if visible_tx().send_replace(on) != on {
        if let Some(p) = handle() {
            p.background(!on);
        }
    }
}

/// Says when the first machine arrives while the app is on screen (one paired just now), so the
/// platform can start what only makes sense with a machine to stay connected to.
#[derive(Default)]
pub struct FirstMachine {
    had: std::cell::Cell<bool>,
}

impl FirstMachine {
    /// The machines now and whether the app is on screen; true when there were none before. Off
    /// screen it waits: the app coming back on screen starts things anyway.
    pub fn seen(&self, machines: usize, visible: bool) -> bool {
        if !visible {
            return false;
        }
        let first = machines > 0 && !self.had.get();
        self.had.set(machines > 0);
        first
    }
}

/// A request to the core, answered as the hub's HTTP API would answer it.
pub async fn request(method: &str, path: &str, body: Option<String>) -> Result<String, ApiError> {
    let gone = || ApiError { status: 0, message: "Inyon's core isn't running. Close the app and open it again.".into() };
    let phone = handle().ok_or_else(gone)?;
    let a = phone.request(method, path, body.map(String::into_bytes)).await.map_err(|_| gone())?;
    let text = String::from_utf8_lossy(&a.body).into_owned();
    if !(200..300).contains(&a.status) {
        return Err(error_of(a.status, &text));
    }
    Ok(text)
}

/// The first message on the socket: protocol v2, uncompressed, since nothing crosses a network.
pub fn hello() -> String {
    wire2(&ClientMsg2::Hello { v: 2, zip: None })
}

/// How long the app may be off screen before it asks for no frames but an overview catch-up when
/// an agent's state changes, as the browser's socket does.
const QUIET_AFTER_MS: u64 = 15_000;

/// The app's socket to the core, until the screen goes away: `Up` once connected, then messages
/// both ways. It goes quiet a while after the app leaves the screen.
pub async fn run(mut outbox: futures_channel::mpsc::UnboundedReceiver<String>, mut on: impl FnMut(Event)) {
    let Some(phone) = handle() else {
        // no core to talk to: the screen says it's offline and stays so
        on(Event::Down(Dropped { code: None, reason: "Inyon's core isn't running".into(), timeout: false }));
        return std::future::pending().await;
    };
    let (tx, mut frames) = futures_channel::mpsc::unbounded::<Frame>();
    let screen = phone.connect(move |f| drop(tx.unbounded_send(f)));
    screen.send(hello());
    on(Event::Up);
    let mut visible = visible_tx().subscribe();
    let mut hidden_since = (!*visible.borrow_and_update()).then(tokio::time::Instant::now);
    let mut quiet = false;
    loop {
        let quiet_at = hidden_since.filter(|_| !quiet).map(|t| t + std::time::Duration::from_millis(QUIET_AFTER_MS));
        tokio::select! {
            f = frames.next() => match f {
                Some(Frame::Text(t)) => on(Event::Message { text: Some(t), binary: None }),
                Some(Frame::Binary(b)) => on(Event::Message { text: None, binary: Some(b) }),
                // the core stopped
                None => {
                    on(Event::Down(Dropped { code: None, reason: "Inyon's core stopped".into(), timeout: false }));
                    return std::future::pending().await;
                }
            },
            m = outbox.next() => match m {
                Some(text) => screen.send(text),
                None => return,
            },
            Ok(()) = visible.changed() => {
                if *visible.borrow_and_update() {
                    hidden_since = None;
                    if quiet {
                        quiet = false;
                        screen.send(wire(&ClientMsg::Quiet { on: false }));
                    }
                } else if hidden_since.is_none() {
                    hidden_since = Some(tokio::time::Instant::now());
                }
            }
            () = tokio::time::sleep_until(quiet_at.unwrap_or_else(tokio::time::Instant::now)), if quiet_at.is_some() => {
                quiet = true;
                screen.send(wire(&ClientMsg::Quiet { on: true }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_is_named_by_its_maker_and_model_as_the_web_app_names_it() {
        assert_eq!(readable_name("Nothing", "A142"), "Nothing A142");
        assert_eq!(readable_name("samsung", "SM-S918B"), "Samsung SM-S918B");
        assert_eq!(readable_name("Google", "google Pixel 9"), "google Pixel 9");
        assert_eq!(readable_name(" Fairphone ", ""), "Fairphone");
        assert_eq!(readable_name("", ""), "Phone");
        assert_eq!(readable_name("", "Pixel 9"), "Pixel 9");
    }

    #[test]
    fn the_first_machine_paired_on_screen_is_noticed_once() {
        let f = FirstMachine::default();
        assert!(!f.seen(0, true));
        // off screen nothing counts yet
        assert!(!f.seen(1, false));
        assert!(f.seen(1, true));
        assert!(!f.seen(2, true));
        // all removed, then one paired again
        assert!(!f.seen(0, true));
        assert!(f.seen(1, true));
    }

    #[test]
    fn the_socket_asks_for_nothing_compressed() {
        assert_eq!(hello(), r#"{"t":"hello","v":2}"#);
    }

    #[test]
    fn a_request_without_a_core_says_how_to_get_one_back() {
        // no core is installed in the tests
        let e = futures_util::FutureExt::now_or_never(request("GET", "/api/session", None)).unwrap().unwrap_err();
        assert_eq!(e.status, 0);
        assert!(e.message.contains("open it again"), "{}", e.message);
    }
}
