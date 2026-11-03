# The kit

Plain HTML, CSS and JS with no build. The kit serves the mockup screens; a site made with
`new-site.mjs --no-mockups` keeps only `blueprint.css`, `lucide.min.js`, `data.js` and `kit.js`, which the
plan page and the matrix use. Every screen loads these in order:
`kit/blueprint.css`, `kit/lucide.min.js`, `kit/data.js`, `kit/kit.js`, `kit/menus.js`, `kit/shell.js`.
Copy `mockups/screens/_template.html` to `mockups/screens/<id>.html` and add the id to `mockups/screens.js`.

## Tokens (kit/blueprint.css)

Swap the `:root` and `[data-theme='dark']` blocks for a product's palette. Everything else reads tokens:
`--bg --panel --raised --sidebar --hover --line --line-strong --field --fg --fg-2 --fg-3 --accent --accent-fg
--accent-soft --logo-tile --bubble --code --ok --warn --warn-soft --danger --danger-soft --attn --question
--diff-add --diff-del --highlight --shadow --shadow-lg --chip --sans --mono --serif`.
Motion: `--dur-exit` 120ms, `--dur-small` 200ms, `--dur-move` 320ms, `--dur-sheet` 480ms; eases `--ease`, `--spring`.
Animate opacity and transform only. The kit turns its own animation off for reduced motion.
Fonts: Geist and Geist Mono ship in `kit/fonts/` (OFL). Replace them with the product's own if it has some.

## CSS classes

- Buttons: `.btn` (+ `.primary .ghost .danger .sm`), `.icon-btn` (+ `.sm .on`), `.chip` (+ `.solid .btn-like`), `.kbd`, `.label`
- Status dots: `.dot.live|info|ask|warn|bad|idle|off`. `live` pulses. Only attention states get colour.
- Surfaces: `.card` (+ `.sm`), `.row` (+ `.active`), `.hair`, `.menu`/`.mi`/`.sep`/`.hd`, `.sheet`, `.toast`, `.tip`
- Inputs: `.input`, `.toggle` (a switch, `aria-checked`), `.seg` with `<button aria-pressed data-value>`,
  `.tabs` with `<button data-tab="x" aria-selected>` and `[data-panel="x"]` in the same parent or `[data-tabs-scope]`
- Lists: `[data-keynav]` with focusable `[data-row]` children get arrows, j/k and Enter
- Shell: `.app` (+ `.with-panel`), `.sidebar` (`.top .scroll .bottom`), `.main`, `.header`, `.crumb`, `.vpanel`,
  `.sec-h`/`.sec-b`, `.srow` (`.t .time .strip`, + `.active .selected .dim`), `.chips-row`,
  `.phone-app`, `.phone-top`, `.phone-nav`, `.swipe`
- Helpers: `.scroll` (overlay scrollbar), `.i14 .i18` icon sizes, `.mono`, `.muted`

## JS (window.K)

- `K.shell.mount({ active, nav, title, crumb, actions, main, panel, phoneTitle, phoneMain, phoneActions, noHeader, onMount, phone })`
  draws the desktop shell (sidebar, header, main, optional right panel), or under 640 px the phone shell (top bar,
  `phoneMain ?? main`, bottom nav from the first five `DATA.nav` entries; `nav: false` hides it).
  `#app` gets an `item:open` event (detail = item id) when a sidebar row is selected in place.
- `K.menu.open(anchorElOr{x,y}, items, { align: 'start'|'end', sheet })`. Items: `{ label, icon, shortcut, hint, sub:[...], danger, disabled, checked, onSelect, toast }`,
  `{ sep: true }`, `{ header: 'Text' }`. An item with no `onSelect` shows a toast naming it. On a phone it opens as a bottom sheet.
- `K.context(el, (el, ev) => items)`, or `data-ctx="name"` plus `K.ctx.register(name, fn)`: right-click, a 500 ms long-press,
  Shift+F10 and the Menu key all open it. Registered by the sample shell: `item` (bulk when several rows are selected) and `group`.
- `window.MENUS`: shared menus, one per object (`item(it) bulk(n) group(g) more()` in the sample). Reuse them for the ⋯ button.
- `K.sheet.open({ title, icon, body, actions:[{ label, kind:'primary'|'ghost'|'danger', icon, onClick({el,close}), toast }], wide, onOpen })`.
  It's a dialog on desktop and a bottom sheet on a phone. Ctrl+Enter presses primary, Esc closes. Return `false` from `onClick` to keep it open.
- `K.toast(msg, { action:{label,onClick}, ms })`, `K.undo(msg, onUndo)`, `K.savedTick(el)` (a small "Saved" after a setting changes).
- Tooltips: `data-tip="Text" data-keys="Ctrl+K" data-tip-pos="top|right"`.
- `K.logo(size)` gives the product mark from `BRAND.logo` (SVG with `{size}`) or a letter tile. `K.mark(name, size)` gives a logo
  from `window.MARKS[name]` or a letter tile. Also `K.icon('lucide-name')`, `K.icons(root)` after inserting HTML, `K.kbd('Ctrl+K')`,
  `K.esc`, `K.el(html)`, `K.$`, `K.$$`, `K.isPhone()`, `K.setTheme('light'|'dark')`.
- `K.enhance(root)` wires toggles, segs, tabs and keynav inside HTML inserted after load. `mount` calls it for you.

Theme: `?theme=dark` in the URL, then the viewer's message, then the saved choice, then the system setting.

Messages: a page takes `{type:'theme', value}` (and the viewer's `notes?`) only from the page it is framed
in, checked with `K.fromParent(e)`. A framed page sends `{type:'theme?'}` to its parent as it starts, and
Inyon answers with the app's theme. Use the same check for any message a screen listens for, and treat what
arrives as text. Inyon frames pages the same way the viewer frames screens, in a sandbox with no storage,
so `localStorage` may throw; the kit already wraps it.

## Data (kit/data.js)

`window.BRAND = { name, logo? }` and `window.DATA = { nav:[{id,label,icon,href}], groups:[{id,label,items:[{id,title,meta,status,href}]}], people, notifications, statusLabel }`
with the helpers `DATA.items`, `DATA.byId(id)` and `DATA.group(id)`. Change the shape freely if you also change `shell.js`.
Data only one screen needs stays inside that screen's page.

## Design notes for the viewer

Each screen has `<template id="notes">`: two sentences on what it shows, then a numbered list. For each
point, say what the feature does, what changes from today, how the phone handles it, and which release ships it.
The viewer shows them beside the frame.

## Screen rules

- Styles for one page go in its `<style>`, with class names prefixed by the screen id. Check every name you add
  against the kit classes: a card called `.live` would also match `.dot.live`.
- Show alternative states (empty, loading, error, offline) with a `?state=` query, tabs or a seg control inside the page.
- Design the phone layout properly; don't just squeeze the desktop. Pass `phoneMain` when the desktop layout doesn't fit.
- Use real-looking content from `DATA`. No lorem ipsum.
