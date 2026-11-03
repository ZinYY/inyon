---
title: Your own copy in a browser
nav: Browser copy
description: Serve the phone app as a static page and pair computers with it from any browser, with no hub.
---

The core that runs inside the Android app also builds as a static web page. Serve it anywhere that serves files, open it in a browser, and pair computers with it exactly as you would with the app. There's still no hub and no password: the page talks to each computer itself.

## Get it

Each release includes `inyon-web.tar.gz`. Or build it from source. From 0.23 the core is Rust compiled to WebAssembly, so the build also needs Rust with the wasm target and `wasm-bindgen-cli` at the version `crates/inyon-phone/Cargo.toml` pins:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version <the pinned version>
bun --filter @inyon/web build:hosted
```

Unpack it on any static host. It has to be served over HTTPS. If your server adds its own `Content-Security-Policy` header, its `script-src` needs `'wasm-unsafe-eval'`, or the browser won't start the core. The page's own policy already allows it.

## Point your computers at it

Pairing QR codes open this website by default. To make them open your copy instead, set `INYON_WEB_URL` on each computer to its address before pairing:

```sh
INYON_WEB_URL=https://inyon.example.com/ inyon pair
```

## How it keeps the pairing secret

The pairing code rides after the `#` in the link, which browsers never send to a server, and the page takes it out of the address bar before anything loads. The page's own key and any SSH secrets are kept in the browser's IndexedDB, sealed with a key the browser won't hand out.

## What's different from the app

- **Only `wss://`.** An HTTPS page can't open plain `ws://` connections, so it reaches computers through their tunnel, or an HTTPS address you publish with `--url`. It can't use a private network's plain address the way the app does.
- **No direct SSH.** A page can't open SSH connections, so a server without `inyon` goes through a paired computer.
- **Updates** come from your server: the page checks its own `version.json` and offers to reload.
- **Install it.** The page can be installed as an app; it offers *Install Inyon* when the browser allows it.

## Notifications

Once you allow notifications, the page hands its push subscription to each paired computer over the encrypted link. When Claude Code needs you or finishes, the computer encrypts the alert itself and sends it straight to the browser's push service (RFC 8291), so alerts arrive with the page closed. This needs `inyon` 0.10 or later on the computer. `inyon push` lists the browsers a computer sends to.

## iPhone and iPad

Add the page to the Home Screen first and pair from there. Safari forgets a site's data after 7 days without a visit, which would lose your pairings, and only Home Screen apps get notifications.
