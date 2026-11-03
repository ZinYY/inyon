//! The cold-start intro (components/Intro.tsx): a computer drops in, a light trails behind it and
//! draws the line, and the kite is where it lands. Under 1.2 s, the first page load of a browser
//! tab's session only (the web app's key, so the two apps show it once between them), and a tap
//! skips it. It ends by flying the mark onto the header's mark (measured, so it lands exactly) as
//! the page fades in behind; where no header mark turns up within a second it shrinks and fades in
//! place. Under reduced motion nothing moves: the finished mark shows for a moment and fades.

use dioxus::prelude::*;

use crate::{dom, platform};

const SEEN: &str = "inyon.intro.seen";
/// when the drawing is done and the hand-off starts
const HANDOFF_AT: u32 = 1000;
/// how long the finished mark waits for a header mark to land on
const HOLD_MS: f64 = 1200.0;
const HANDOFF_MS: u32 = 320;
/// the overlay goes once the flight is over
const SETTLE_MS: u32 = HANDOFF_MS + 80;
const MARK: &str = "intro-mark";
/// the overlay's frame, inline (the stylesheet can arrive a moment after the first paint)
const FRAME: &str = "position: fixed; inset: 0; z-index: 90; display: grid; place-items: center";
const RUNNING: &str = "intro-running";

const SQUIRCLE: &str = "M25 0H39C60 0 64 4 64 25V39C64 60 60 64 39 64H25C4 64 0 60 0 39V25C0 4 4 0 25 0Z";
const KITE: &str =
    "M33.5 9H48.5A3.5 3.5 0 0 1 52 12.5V33.5A3.5 3.5 0 0 1 48.5 37H33.5A3.5 3.5 0 0 1 30 33.5V12.5A3.5 3.5 0 0 1 33.5 9ZM41 18.4A4.6 4.6 0 1 0 41 27.6A4.6 4.6 0 1 0 41 18.4Z";
const LINE: &str = "M38.5 36.5C35 48 25 42 17.5 49.5";

/// Whether this page load shows the intro: the first in this browser tab's session.
pub fn due() -> bool {
    dom::first_in_session(SEEN)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// the stylesheet isn't in yet: the ground only, so the drawing starts whole once it is
    Wait,
    Draw,
    Handoff,
    Gone,
}

/// The transform that puts a mark drawn in `from` exactly over `to` (left, top, width, height):
/// the same drawing in both, so one translate and one scale.
pub fn flight(from: (f64, f64, f64, f64), to: (f64, f64, f64, f64)) -> String {
    let scale = to.2 / from.2;
    let dx = to.0 + to.2 / 2.0 - (from.0 + from.2 / 2.0);
    let dy = to.1 + to.3 / 2.0 - (from.1 + from.3 / 2.0);
    format!("transform: translate({dx:.1}px, {dy:.1}px) scale({scale:.4})")
}

#[component]
pub fn Intro() -> Element {
    let mut phase = use_signal(|| if dom::styled() { Phase::Draw } else { Phase::Wait });
    let mut style = use_signal(String::new);
    let still = use_hook(dom::reduced_motion);

    // flies the mark onto the header's; waits up to `until` for a header mark first
    let handoff = move |until: f64| {
        spawn(async move {
            while *phase.peek() == Phase::Draw && dom::intro_target().is_none() && platform::clock().now < until {
                platform::sleep(50).await;
            }
            if *phase.peek() != Phase::Draw {
                return;
            }
            let from = dom::rect(MARK).map(|(l, t, r, b)| (l, t, r - l, b - t));
            style.set(match (from, dom::intro_target()) {
                (Some(f), Some(t)) if f.2 > 0.0 => flight(f, t),
                _ => "transform: scale(0.6); opacity: 0".into(),
            });
            phase.set(Phase::Handoff);
            platform::sleep(SETTLE_MS).await;
            phase.set(Phase::Gone);
            dom::root_class(RUNNING, false);
        });
    };
    use_hook(move || {
        dom::root_class(RUNNING, true);
        let drawn = if still { 400 } else { HANDOFF_AT };
        spawn(async move {
            // the clock starts with the drawing; a page without its stylesheet after 3 s goes without
            let mut waited = 0;
            while !dom::styled() && waited < 3000 {
                platform::sleep(30).await;
                waited += 30;
            }
            if *phase.peek() == Phase::Wait {
                phase.set(Phase::Draw);
            }
            platform::sleep(drawn).await;
            handoff(platform::clock().now + HOLD_MS);
        });
    });
    use_drop(|| dom::root_class(RUNNING, false));

    let p = phase();
    if p == Phase::Gone {
        return rsx! {};
    }
    let class = format!("intro{}{}", if p == Phase::Handoff { " intro-off" } else { "" }, if still { " intro-still" } else { "" });
    // a tap wants it gone: fly if the header is there, otherwise fade at once
    let skip = move || {
        let now = *phase.peek();
        match now {
            Phase::Draw => handoff(0.0),
            // nothing drawn yet: gone at once
            Phase::Wait => {
                let mut ph = phase;
                ph.set(Phase::Gone);
                dom::root_class(RUNNING, false);
            }
            Phase::Handoff | Phase::Gone => {}
        }
    };
    rsx! {
        div {
            class: "{class}",
            style: if p == Phase::Handoff { FRAME.to_string() } else { format!("{FRAME}; background: var(--bg, {})", if dom::theme() == "dark" { "#1b1a18" } else { "#e9e8e4" }) },
            role: "presentation",
            "aria-hidden": "true",
            "data-testid": "intro",
            onpointerdown: move |_| skip(),
            onkeydown: move |_| skip(),
            if p != Phase::Wait {
                div { id: MARK, class: "intro-mark", style: if p == Phase::Handoff { format!("width: 160px; height: 160px; {}", style()) } else { "width: 160px; height: 160px".to_string() },
                    svg { view_box: "0 0 64 64", class: "intro-svg",
                        path { class: "intro-tile", d: SQUIRCLE, fill: "var(--logo-tile)" }
                        g { class: "intro-glyph",
                            path { class: "intro-kite", "fill-rule": "evenodd", d: KITE }
                            path { class: "intro-line", d: LINE, fill: "none", "stroke-width": "3.8", "stroke-linecap": "round" }
                            circle { class: "intro-node", cx: "15.5", cy: "51", r: "4" }
                            circle { class: "intro-light", r: "2.6" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_flies_onto_the_header_mark_with_one_translate_and_one_scale() {
        // a 160px mark in the middle of a 390×844 screen onto a 26px one at (16, 13)
        let t = flight((115.0, 342.0, 160.0, 160.0), (16.0, 13.0, 26.0, 26.0));
        assert_eq!(t, "transform: translate(-166.0px, -396.0px) scale(0.1625)");
    }
}
