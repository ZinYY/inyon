/* Sample data for the example project, "Pantry": a shared grocery list and meal planner.
   Replace all of it with your product's own nouns. Keep the shape the shell reads:
   BRAND, DATA.nav, DATA.groups[].items[], DATA.notifications. Screen-only data stays in the screen page. */
window.BRAND = {
  name: 'Pantry',
  // logo: '<svg viewBox="0 0 24 24" width="{size}" height="{size}">…</svg>',  // optional; else a letter tile
};
window.DATA = {
  // Desktop sidebar links and the phone's bottom bar (first five). href = a screen in mockups/screens/.
  nav: [
    { id: 'home', label: 'Today', icon: 'sun', href: 'home.html' },
    { id: 'lists', label: 'Lists', icon: 'list-checks', href: 'list.html' },
    { id: 'recipes', label: 'Recipes', icon: 'book-open', href: 'recipe.html' },
    { id: 'inbox', label: 'Inbox', icon: 'bell', href: 'home.html' },
    { id: 'settings', label: 'Settings', icon: 'settings', href: 'settings.html' },
  ],
  // Sidebar sections. status = live | info | ask | warn | bad | idle | off (see .dot in blueprint.css).
  groups: [
    { id: 'g-week', label: 'This week', items: [
      { id: 'l1', title: 'Big shop', meta: '14 left', status: 'live', href: 'list.html' },
      { id: 'l2', title: 'Market on Saturday', meta: '6', status: 'idle', href: 'list.html' },
      { id: 'l3', title: 'Party snacks', meta: 'due Fri', status: 'warn', href: 'list.html' },
    ] },
    { id: 'g-home', label: 'Household', items: [
      { id: 'l4', title: 'Cleaning', meta: '3', status: 'idle', href: 'list.html' },
      { id: 'l5', title: 'Pharmacy', meta: 'asked', status: 'ask', href: 'list.html' },
      { id: 'l6', title: 'Old list', meta: 'May', status: 'off', href: 'list.html' },
    ] },
  ],
  people: [{ id: 'p1', name: 'Mira' }, { id: 'p2', name: 'Jonas' }, { id: 'p3', name: 'Ada' }],
  notifications: [
    { id: 'n1', text: 'Jonas ticked off oat milk', ago: '2m', unread: true },
    { id: 'n2', text: 'Ada asked: which pharmacy?', ago: '1h', unread: true },
    { id: 'n3', text: 'Party snacks is due Friday', ago: '3h', unread: false },
  ],
  statusLabel: { live: 'Shopping now', info: 'Updated', ask: 'Has a question', warn: 'Due soon', bad: 'Failed', idle: 'Idle', off: 'Archived' },
};
DATA.items = DATA.groups.flatMap((g) => g.items.map((i) => ({ ...i, group: g.id })));
DATA.byId = (id) => DATA.items.find((i) => i.id === id);
DATA.group = (id) => DATA.groups.find((g) => g.id === id);
