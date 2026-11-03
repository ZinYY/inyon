/* Feature matrix: one row per item in the plan (a feature, a part, a task), from your research notes.
   Row: [area, feature, want, status, today, release, phone, desktop, screen, note, url]
   want: what the item is (from the reference product, the brief or the user). status: a v from MATRIX_STATUSES.
   today: what exists now. release: a v from MATRIX_RELEASES (a version, a store, a phase) or ''.
   screen: a mockup id or ''. url: an outside link for the row (a product page, an issue) or ''. */
(function () {
  const H = 'have', P = 'partial', N = 'new', S = 'skipped';
  const R = [
  ['Lists', 'Open a list offline', 'Lists open instantly with no signal.', P, 'Lists open only after the server answers.', '2.4', 'Opens from the local copy; an offline chip shows in the header.', 'Same, in the browser with a service worker.', 'list', ''],
  ['Lists', 'Tick items offline', 'Ticks made offline sync later.', N, '', '2.4', 'Tick, swipe to remove, long-press for more.', 'Click, right-click for more.', 'list', 'Queued edits show a small clock until they sync.'],
  ['Lists', 'Merge two people’s edits', 'Two phones editing one list both keep their changes.', N, '', '2.5', 'Silent merge; a history sheet shows who did what.', 'Same, history in the side panel.', 'list', ''],
  ['Lists', 'Share a list', 'Invite someone to a list.', H, 'Share by email from the list menu.', '', 'Share sheet with the system share targets.', 'Share dialog.', '', ''],
  ['Recipes', 'Recipes with ingredients', 'A recipe knows its ingredients and servings.', N, '', '2.6', 'Recipe page with a servings stepper.', 'Two columns: ingredients and steps.', 'recipe', ''],
  ['Recipes', 'Add a recipe to a list', 'Missing ingredients land on the shop list.', N, '', '2.6', 'One button, then an undo toast.', 'Button and a picker for the target list.', 'recipe', 'Skips what the pantry already has.'],
  ['Settings', 'Offline storage limit', 'Choose how much the phone keeps.', N, '', '2.4', 'Settings row with a size picker.', 'Same.', 'settings', ''],
  ['Settings', 'Import recipes from a link', 'Paste a link and get the recipe.', S, '', '', '', '', '', 'Left out: most recipe sites block scraping. Open question in Decisions.'],
  ];
  window.MATRIX = R.map(([area, feature, want, status, today, release, phone, desktop, screen, note, url]) => ({ area, feature, want, status, today, release, phone, desktop, screen, note, url }));
  // Status order, labels and marks. mark: full | half | ring | dash, by list order when left out.
  // tone: ok | warn | bad colours the mark; leave it out for states that don't need attention.
  window.MATRIX_STATUSES = [
    { v: H, l: 'Have', tone: 'ok' },
    { v: P, l: 'Partial', tone: 'warn' },
    { v: N, l: 'New' },
    { v: S, l: 'Skipped' },
  ];
  window.MATRIX_RELEASES = [
    { v: '2.4', name: 'Lists on the phone' },
    { v: '2.5', name: 'Merging edits' },
    { v: '2.6', name: 'Recipes fill a list' },
  ];
  window.MATRIX_AREAS = ['Lists', 'Recipes', 'Settings'];
  // Page words. Others you can set: title, one, many, search, area, feature, status, statusName, release,
  // today, todayNone, todaySkipped, phone, desktop, note, link and the menu words (see references/plan.md).
  // A column or field whose label is '' is left out.
  window.MATRIX_LABELS = {
    want: 'Wanted',
    sub: 'Every feature in the plan, what Pantry does today, and the release that brings it.',
    subShort: 'Every feature and when it ships.',
    foot: 'From the research notes, checked against Pantry 2.3 on 1 March 2027.',
  };
})();
