# Brief for parallel screen builders

Hand each worker this brief with its own list of screen ids. Fill in the angle brackets.

---

You are building interactive HTML mockups for <product>'s plan: <one sentence on what the plan does>.
The site is at `<site dir>`. It's served at `<url>` and opened in a viewer that frames each screen at
390×844 and 1440×900.

Read before writing:
1. `<skill>/references/kit.md`, then skim `kit/blueprint.css`, `kit/kit.js`, `kit/shell.js`, `kit/menus.js`
   and `kit/data.js` so you reuse what exists.
2. `research/` in the site: the feature list and how things work today. Every feature in your sections
   must show up on your screens as a control, a menu item, a card or a state.
3. `mockups/screens/_template.html` and your entries in `mockups/screens.js`.
4. <design and writing skills or guides to follow>.

Rules:
- One file per screen: `mockups/screens/<id>.html`. Styles for the page go in its own `<style>`, prefixed
  with the screen id. Don't edit `kit/`, except to append a missing shared menu to `kit/menus.js`. Other
  builders are working in parallel.
- Everything is interactive. Each button, row, tab and chip does something: it changes state, opens a
  menu or sheet, or shows a toast saying what would happen. Right-click and long-press work wherever the
  real app would have a menu.
- The phone layout is a real design: bottom sheets, thumb reach, 44 px targets.
- Design notes go in `<template id="notes">`.
- Copy is plain, short and in sentence case. Use real-looking content from `DATA`, never lorem ipsum.

Check (required: fix what you find, then check once more):
- `node <skill>/scripts/shoot.mjs <url>/mockups/screens/<id>.html <scratch>/<id>-d.png 1440 900 light`, then
  in `dark`, then at `390 844`, plus shots with menus and sheets open (`click:<css>`, `rclick:<css>`).
- `AUDIT=1` must report no console errors and no dead controls.
- Look at every picture: nothing clipped or overlapping, nothing under 12 px, dark mode readable, no
  sideways scroll on the phone.
- If the plan tracks progress, run `python3 <skill>/scripts/progress.py --file <site>/progress.json task <id> doing|done "<note>"`.

Report back: the files you wrote, one line per screen on its states and interactions, which features you
covered, anything you couldn't fit, and any change to the kit.
