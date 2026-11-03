# The plan page, matrix and progress board

## index.html, top to bottom

1. **Top bar**: logo, name, links to Screens and Feature matrix, theme button. Keep it as is.
   Parts that link to the screens sit between `<!-- mockups -->` markers; `new-site.mjs --no-mockups`
   removes them and renumbers the sections.
2. **Eyebrow**: `Plan · <date> · from <current version>`.
3. **h1**: the outcome in under eight words. No product name, no colon.
4. **Lede**: two sentences. The first says what's wrong today, the second says what the plan does about it.
5. **Buttons**: Open the screens, Feature by feature.
6. **Where it stands**: the progress board. It's driven by `progress.json`, so don't edit its HTML.
7. **Contents**: one link per numbered h2.
8. **Sections**, numbered `01`, `02` and so on. Pick from these and keep the order:
   - *Short version*: three or four `.short` items (icon, a bold sentence, one more line), then four `.stats`
     (features mapped, screens, releases, and the one number the plan moves).
   - *What changes*: a Today / After table.
   - *How it works*: a figure with a caption that tells the story on its own, then a `.call` box for the main trade-off.
   - *Screens*: one sentence, then `#gallery`, which fills itself from `SCREENS` and the stills in `shots/`.
   - *Releases*: the `R` array in the page script. Each release gets `v`, `r` (the id progress.json uses), `t` (title),
     `s` (one-line subtitle) and the lists `scope` and `check` ("Done when"). Use `{ phase: 'Name' }` rows to group them.
   - *Bugs we won't ship*, *Compatibility*, *Risks*: tables. Use risks only if they're real, each with what you'll do.
   - *Decisions*: `.dec` cards tagged `Decided`, `Question`, `Trade-off`, `Flag` or `Gap`. Put open questions here, so
     the reader can answer them.
9. **Footer**: name and links.

Writing: plain words, sentences of 8 to 18 words, real numbers with units, sentence-case headings, at most one em dash
on the page. Cut every sentence that doesn't change what the reader knows. Leave out "seamless", "robust", "leverage",
"unlock" and similar words.

## matrix-data.js

Each row is `[area, feature, want, status, today, release, phone, desktop, screen, note, url]`. A row is
any item the plan tracks: a feature, a part, a task.
- `want`: the item, as the brief, the reference product or the user describes it.
- `status`: a `v` from `MATRIX_STATUSES`. A skipped row gives the reason in `note`.
- `today`: what exists now, checked in the code. Empty for a new item.
- `release`: a `v` from `MATRIX_RELEASES` (a version, a store, a phase), or `''`.
- `screen`: a mockup id. The row then links to it in the viewer.
- `url`: an outside link (a store page, an issue). It shows as ↗ in the row and the details, and as
  "Open link" in the row's menu.

`MATRIX_STATUSES` is a list of `{ v, l, mark?, tone? }` in display order. Without it the page uses
have, partial, new and skipped. `mark` is `full`, `half`, `ring` or `dash`; left out, marks follow the
list order. `tone` is `ok`, `warn` or `bad` and colours the mark. Leave it out unless the state needs
attention. The summary has one tile per status plus the total, and the tiles fill their rows.

`MATRIX_RELEASES` is `[{ v, name }]` and `MATRIX_AREAS` sets the area order.

`MATRIX_LABELS` holds the page's words. `want` (the column title, for example "In <reference app>"),
`sub`, `subShort` and `foot` (the source and the date it was checked) are the usual ones. The rest default
to the software wording:

| Key | Default |
|---|---|
| `title`, `one`, `many`, `search` | Feature matrix, feature, features, Search features |
| `area`, `feature`, `status`, `statusName`, `release` | Area, Feature, `<BRAND>`, Status, Release |
| `today`, `todayNone`, `todaySkipped` | `<BRAND>` today, Nothing yet, Not planned |
| `phone`, `desktop`, `note`, `link` | On the phone, Desktop and browser, Note, Link |
| `menuShow`, `menuHide`, `menuMockup`, `menuLink` | Show details, Hide details, Open mockup, Open link |
| `menuArea`, `menuRelease`, `allAreas`, `menuCopy` | Only {area}, Only release {release}, All areas, Copy as text |
| `copied`, `copyFailed`, `sortNone` | Copied, Could not copy, Inventory |

A label set to `''` leaves out its column or field: `area`, `want`, `status`, `today`, `release`,
`phone`, `desktop` and `note`. "Copy as text" uses the same labels.

A parts list, for example:

```js
window.MATRIX_STATUSES = [{ v: 'have', l: 'In the drawer' }, { v: 'ordered', l: 'Ordered' }, { v: 'buy', l: 'To buy', tone: 'warn' }];
window.MATRIX_RELEASES = [{ v: 'Reichelt', name: 'Passives and power' }, { v: 'Mouser', name: 'Pot and caps' }];
window.MATRIX_LABELS = { title: 'Parts list', one: 'part', many: 'parts', search: 'Search parts', feature: 'Part',
  want: 'Spec', status: 'Status', release: 'Store', menuRelease: 'Only {release}', today: '', phone: 'Quantity', desktop: 'Price each' };
// row: ['Regulator', 'LM317T', 'Adjustable regulator, TO-220.', 'have', '', 'Reichelt', '2', '0.48 €', '', '', 'https://…']
```

The matrix loads `mockups/screens.js` only when some row has a `screen`. A site made with `--no-mockups`
sets `window.MATRIX_MOCKUPS = false`, which turns mockup links off.

## progress.json

```json
{ "updated": "ISO time", "now": "what is being worked on",
  "releases": [ { "v": "2.4", "r": "R1", "title": "…", "status": "todo|doing|done|blocked",
                  "tasks": [ { "id": "r1-x", "title": "…", "status": "todo", "note": "optional", "link": "optional URL" } ] } ],
  "log": [ { "t": "ISO time", "text": "…" } ] }
```

Give each task an id that's unique across the file, and end each release with a "Release X" task. Let
`progress.py` set the release status and the timestamps; don't edit them by hand while workers run. The
board opens the first release that's in progress. The newest log line goes first, and only 200 are kept.

The board shows everything in the file as text, and a task's `link` only when it is http(s) or relative.
Inyon can push a fresh copy into the page with `{type:'bp-progress', data}`, which the board accepts only
from the page it is framed in; keep `render(d)` and that listener if you rework the board.
