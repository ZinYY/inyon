---
title: The Inyon app on Windows
nav: Windows app
description: The Inyon window and tray icon on Windows: how phones reach the computer, pairing, a hub's machines and sessions, Pages, temp files, logs and settings.
---

On Windows, Inyon is an app with a window and an icon in the notification area. From 0.21 the window covers everything the computer does for Inyon: the phone link and how phones reach it, the phones you paired, a hub and its machines, your Pages, the files agents leave behind, the logs and the settings. Nothing in it opens a terminal.

The window is drawn by Microsoft Edge WebView2, which is part of Windows 11 and comes with Edge on Windows 10. If it's missing, Inyon says so and opens Microsoft's download page; the phone link and the hub connection keep running without the window.

## Opening and closing it

Click the Inyon icon to show the window, and click it again to hide it. **Inyon** in the Start menu shows it too. Closing the window hides it while the phone link or the hub connection runs; the first time, a note says Inyon is still running. With nothing running, closing it ends the app.

Settings › General › **Closing the window** chooses **Keep running** (the default) or **Quit**, which stops everything Inyon runs. At sign-in Inyon starts in the notification area without the window, unless **Show this window at sign-in** is on.

Before 0.21 the window had a fixed size, and after the first close the icon couldn't open it again until you quit Inyon.

## The icon's menu

Right-click the icon for:

| Item | Does |
|---|---|
| The first line | What runs: *Running: link, tunnel*, or *Running: hub connection* |
| Open Inyon, Hide Inyon | Shows or hides the window |
| Pair a phone… | The window on Phones, with a new pairing code |
| Phones reach this computer | LAN, VPN and Tunnel, each ticked when it's on; choose one to turn it on or off |
| Start the phone link, Stop the phone link | Starts or stops the link without closing Inyon |
| Phones, Hub, Temp files, Settings | Opens the window on that page |
| Start at sign-in | Ticked when the phone link or hub connection starts when you sign in |
| Check for updates | Looks for a new version and opens Settings › Updates |
| Open the logs | The folder with `link.log`, `node.log`, `app.log` and `window.log` |
| Quit Inyon | Stops everything the app runs, and the icon goes. It starts again when you next sign in, or when you open Inyon |

The menu follows the app's theme, light or dark.

## This computer

The phone link: whether it runs, whether it starts at sign-in, and the addresses phones use, each with a copy button. **Restart**, **Stop** or **Start**, and **Pair a phone**.

**How phones reach it** has three switches, and each change applies right away, in the background:

| Switch | Phones connect |
|---|---|
| Tunnel | From any network, through a free Cloudflare tunnel. The tunnel's address is shown when it's up |
| LAN | On the same Wi-Fi or network |
| VPN | Over Tailscale, NetBird or ZeroTier, shown with this computer's address on it |

LAN and VPN need a Windows Firewall rule for the link's port; turning either on adds it, and Windows asks for administrator permission once. For LAN only, turn Tunnel and VPN off. The last switch that's on can't be turned off, since phones would have no way in. Before 0.21 opening the window made a new pairing code each time, and with Tunnel on its card said "Starting the tunnel…" while it waited.

## Phones

**Pair a phone** shows a QR code to scan in the Android app, with the ID and code to type instead. The code works once, for 10 minutes. While the tunnel starts, it works on the same network or VPN. Paired phones are listed with when each was last seen; **Forget** removes one, with Undo for a few seconds.

## Hub

The Hub page shows a hub's own web app inside the window, signed in like any browser, so you see every machine on it, their sessions and its Pages. The bar above it has Back, Reload, Open in your browser, and a menu to switch between hubs.

| Button | Does |
|---|---|
| Add this computer to a hub | Paste the command from the hub's Machines page (Add a machine). The computer joins the hub and its connection starts, as `inyon join` does |
| Open a hub | Shows any hub by its address, without joining it. Addresses on `localhost`, `127.x` and `100.x` default to `http://`, others to `https://` |
| Create a hub | Hubs run on Linux and macOS. On Windows this installs one in WSL and opens it at `http://localhost:4777`. Without WSL it opens Microsoft's guide to installing it |

A hub you joined with its certificate's SHA-256 (a hub on the LAN with a certificate of its own) opens without a warning when it shows that same certificate, and only then. Links that leave the hub open in your browser.

## Pages

The sites you added with [`inyon pages add`](pages.md) on this computer, each opened inside the window. **Add a folder** picks a folder with an `index.html`. Right-click a page to open it, show its folder, or remove it from Pages (the folder stays). Pages on other machines show in their hub's web app.

## Temp files

What agents left in temporary folders on this computer: Claude Code's session folders with pasted images and scratchpads, its paste cache, Inyon's uploads and others, as described in [Temp files](temp-files.md). Select items with their checkboxes, all of them, or **Older than a week**, and delete them after a confirmation. Click an item to see its files, with pictures shown as thumbnails; click a file to preview it. Items changed in the last few minutes are marked *in use?*, since a running agent may still need them.

## Activity

The checks `inyon doctor` runs, each with what it found, and the last lines of each log (the app, the link, the hub connection and updates), refreshed every few seconds while the page is open.

## Settings

| Section | Settings |
|---|---|
| General | Start Inyon when you sign in, Show this window at sign-in, When you close the window |
| Appearance | System, Light or Dark. The title bar and menus follow |
| Updates | Check now (Update to the new version when one is out), and Check for updates by itself (once a day and when Inyon starts) |
| Phone link | The port the link listens on |
| About | The documentation, the Inyon folder, Quit Inyon, and Uninstall Inyon (after a confirmation) |

The window's own settings are in `%USERPROFILE%\.inyon\node\desktop.json`, and WebView2 keeps its data (the hubs' sign-in cookies) in `%USERPROFILE%\.inyon\node\webview`.

## Keyboard

Ctrl+1 to Ctrl+7 switch pages, in the sidebar's order. Esc closes a menu or dialog, and the arrow keys move through a menu. Every row with a menu opens it on right-click too.
