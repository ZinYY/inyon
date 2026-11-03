// The Inyon app's window on Windows. Every screen is drawn from one state object the app sends
// (`state`), and every button is a call to the app (`call`). Inputs live in dialogs, never on a
// screen the state redraws, so a refresh can't take what you were typing.
'use strict';

// ---------- the bridge to the app ----------

const pending = new Map();
let seq = 0;
const listeners = {};

window.__inyonReply = (id, ok, value) => {
  const p = pending.get(id);
  if (!p) return;
  pending.delete(id);
  ok ? p.resolve(value) : p.reject(new Error(typeof value === 'string' ? value : 'Something went wrong.'));
};
window.__inyonEvent = (name, value) => (listeners[name] || []).forEach((f) => f(value));

function call(cmd, args = {}) {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    window.ipc.postMessage(JSON.stringify({ id, cmd, args }));
  });
}
const on = (name, f) => (listeners[name] = listeners[name] || []).push(f);

// ---------- small helpers ----------

const ICONS = {
  monitor: '<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>',
  phone: '<rect width="14" height="20" x="5" y="2" rx="2" ry="2"/><path d="M12 18h.01"/>',
  hub: '<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="9" y="2" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>',
  pages: '<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z"/><path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65"/><path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65"/>',
  trash: '<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>',
  activity: '<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>',
  settings: '<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/>',
  refresh: '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
  external: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  minus: '<path d="M5 12h14"/>',
  copy: '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  right: '<path d="m9 18 6-6-6-6"/>',
  left: '<path d="m15 18-6-6 6-6"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  more: '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
  globe: '<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>',
  wifi: '<path d="M12 20h.01"/><path d="M2 8.82a15 15 0 0 1 20 0"/><path d="M5 12.859a10 10 0 0 1 14 0"/><path d="M8.5 16.429a5 5 0 0 1 7 0"/>',
  shield: '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>',
  logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" x2="9" y1="12" y2="12"/>',
  image: '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>',
  power: '<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>',
  play: '<polygon points="6 3 20 12 6 21 6 3"/>',
  stop: '<rect width="14" height="14" x="5" y="5" rx="2"/>',
  rotate: '<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>',
  qr: '<rect width="5" height="5" x="3" y="3" rx="1"/><rect width="5" height="5" x="16" y="3" rx="1"/><rect width="5" height="5" x="3" y="16" rx="1"/><path d="M21 16h-3a2 2 0 0 0-2 2v3"/><path d="M21 21v.01"/><path d="M12 7v3a2 2 0 0 1-2 2H7"/><path d="M3 12h.01"/><path d="M12 3h.01"/><path d="M12 16v.01"/><path d="M16 12h1"/><path d="M21 12v.01"/><path d="M12 21v-1"/>',
  info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  alert: '<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>',
  back: '<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>',
  terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>',
};

function icon(name, cls = 'i') {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('class', cls);
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = ICONS[name] || '';
  return s;
}

// h('div.card', {onclick}, 'text', child): an element, with text always as text
function h(spec, attrs, ...kids) {
  const [tag, ...classes] = spec.split('.');
  const el = document.createElement(tag || 'div');
  if (classes.length) el.className = classes.join(' ');
  if (attrs && (typeof attrs !== 'object' || attrs instanceof Node || Array.isArray(attrs))) {
    kids.unshift(attrs);
    attrs = null;
  }
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'class') el.className += ' ' + v;
    else if (k === 'style') el.setAttribute('style', v);
    else if (k === 'html') el.innerHTML = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const kid of kids.flat(Infinity)) {
    if (kid === null || kid === undefined || kid === false) continue;
    el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  return el;
}

function btn(label, onclick, opts = {}) {
  const b = h('button.btn', { type: 'button', class: opts.cls, title: opts.title, 'aria-label': opts.aria || (opts.iconOnly ? label : null), disabled: opts.disabled }, opts.icon ? icon(opts.icon) : null, opts.iconOnly ? null : label);
  b.addEventListener('click', async (e) => {
    e.stopPropagation();
    if (b.disabled) return;
    const r = onclick(e, b);
    if (r && r.then) {
      b.disabled = true;
      try { await r; } finally { b.disabled = false; }
    }
  });
  return b;
}

function sw(on, onchange, opts = {}) {
  const s = h('button.switch', { type: 'button', role: 'switch', 'aria-checked': on ? 'true' : 'false', 'aria-label': opts.label, disabled: opts.disabled, class: opts.busy ? 'busy' : null });
  s.addEventListener('click', () => {
    const next = s.getAttribute('aria-checked') !== 'true';
    s.setAttribute('aria-checked', next ? 'true' : 'false');
    onchange(next);
  });
  return s;
}

function seg(options, value, onpick) {
  return h('div.seg', { role: 'group' }, options.map(([v, label]) => h('button', { type: 'button', 'aria-pressed': v === value ? 'true' : 'false', onclick: () => v !== value && onpick(v) }, label)));
}

function checkbox(state, onclick, label) {
  const c = h('button.check', { type: 'button', role: 'checkbox', 'aria-checked': state === 'mixed' ? 'mixed' : state ? 'true' : 'false', 'aria-label': label }, icon(state === 'mixed' ? 'minus' : 'check'));
  c.addEventListener('click', (e) => { e.stopPropagation(); onclick(e); });
  return c;
}

const pill = (text, dot) => h('span.pill', h('span.dot', { class: dot }), text);
const chip = (text, title) => h('span.chip', { title: title || text }, text);
const note = (text, kind = '') => h('div.note', { class: kind }, icon(kind ? 'alert' : 'info'), h('div', text));

function size(bytes) {
  if (bytes == null) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let n = bytes;
  while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
  return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
}

function age(ms) {
  if (!ms) return '';
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  if (s < 86400 * 60) return `${Math.floor(s / 86400)} d`;
  return `${Math.floor(s / (86400 * 30))} mo`;
}

const plural = (n, one, many) => `${n} ${n === 1 ? one : many || one + 's'}`;

// ---------- toasts, menus, dialogs ----------

function toast(text, action, ms = 4000) {
  const t = h('div.toast', h('span', text));
  let done = false;
  const close = () => {
    if (done) return;
    done = true;
    t.classList.add('out');
    setTimeout(() => t.remove(), 130);
  };
  if (action) t.append(h('button', { type: 'button', onclick: () => { action.run(); close(); } }, action.label));
  document.getElementById('toasts').append(t);
  setTimeout(close, ms);
  return close;
}

let overlays = 0;
function overlayOpened() {
  overlays++;
  syncView();
}
function overlayClosed() {
  overlays = Math.max(0, overlays - 1);
  syncView();
}

let openMenu = null;
function closeMenu() {
  if (!openMenu) return;
  openMenu.remove();
  openMenu = null;
  overlayClosed();
}

// items: [{ label, icon, run, danger, disabled } | 'sep' | { heading }]. On a screen that shows a
// hub or a page, the app's own menu opens instead, so the view stays on screen under it.
async function menu(items, x, y) {
  closeMenu();
  if (viewShown()) {
    const flat = items.map((it, i) => (it === 'sep' ? { sep: true } : it.heading ? { label: it.heading, disabled: true } : { id: i, label: it.label, disabled: !!it.disabled, checked: !!it.checked }));
    const picked = await call('menu', { x, y, items: flat }).catch(() => null);
    if (picked != null && items[picked] && items[picked].run) items[picked].run();
    return;
  }
  const m = h('div.menu', { role: 'menu' });
  for (const it of items) {
    if (it === 'sep') m.append(h('div.sep'));
    else if (it.heading) m.append(h('div.label', it.heading));
    else m.append(h('button', { type: 'button', role: 'menuitem', class: it.danger ? 'danger' : null, disabled: it.disabled, onclick: () => { closeMenu(); it.run && it.run(); } }, it.icon ? icon(it.icon) : null, it.label, it.checked ? h('span', { style: 'margin-left:auto' }, icon('check')) : null));
  }
  document.body.append(m);
  overlayOpened();
  openMenu = m;
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + 'px';
  const first = m.querySelector('button:not([disabled])');
  if (first) first.focus();
}

function menuAt(el, items) {
  const r = el.getBoundingClientRect();
  menu(items, r.left, r.bottom + 4);
}

// right-click and the ⋯ button share one list
function withMenu(el, items) {
  el.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    menu(typeof items === 'function' ? items() : items, e.clientX, e.clientY);
  });
  return el;
}

document.addEventListener('mousedown', (e) => {
  if (openMenu && !openMenu.contains(e.target)) closeMenu();
});
document.addEventListener('keydown', (e) => {
  if (openMenu) {
    const list = [...openMenu.querySelectorAll('button:not([disabled])')];
    const i = list.indexOf(document.activeElement);
    if (e.key === 'Escape') { closeMenu(); e.preventDefault(); }
    if (e.key === 'ArrowDown') { (list[i + 1] || list[0]).focus(); e.preventDefault(); }
    if (e.key === 'ArrowUp') { (list[i - 1] || list[list.length - 1]).focus(); e.preventDefault(); }
    return;
  }
  if (e.key === 'Escape' && dialogs.length) dialogs[dialogs.length - 1].close();
  if (e.ctrlKey && !e.shiftKey && !e.altKey && /^[1-7]$/.test(e.key)) {
    const p = PAGES[+e.key - 1];
    if (p) { go(p.id); e.preventDefault(); }
  }
});
window.addEventListener('blur', closeMenu);
// no browser menu on the rest of the window; text fields and selected text keep theirs
document.addEventListener('contextmenu', (e) => {
  if (e.defaultPrevented || e.target.closest('input, textarea, [contenteditable]') || String(getSelection())) return;
  e.preventDefault();
});

const dialogs = [];
// A dialog: `build(close)` returns its content. Resolves with what close() was given.
function dialog(build, opts = {}) {
  return new Promise((resolve) => {
    const scrim = h('div.scrim');
    const box = h('div.dialog', { role: 'dialog', 'aria-modal': 'true', class: opts.wide ? 'wide' : null });
    const d = {
      close(value) {
        const i = dialogs.indexOf(d);
        if (i < 0) return;
        dialogs.splice(i, 1);
        scrim.classList.add('out');
        setTimeout(() => scrim.remove(), 130);
        overlayClosed();
        resolve(value);
      },
    };
    box.append(...[build((v) => d.close(v))].flat());
    scrim.append(box);
    scrim.addEventListener('mousedown', (e) => { if (e.target === scrim && !opts.sticky) d.close(); });
    document.body.append(scrim);
    dialogs.push(d);
    overlayOpened();
    const f = box.querySelector('[autofocus]') || box.querySelector('input, textarea, button.primary, button');
    if (f) setTimeout(() => f.focus(), 30);
  });
}

function confirmBox(title, text, okLabel, opts = {}) {
  return dialog((close) => [
    h('h3', title),
    typeof text === 'string' ? h('p', text) : text,
    h('div.foot', btn('Cancel', () => close(false), { cls: 'ghost' }), btn(okLabel, () => close(true), { cls: opts.danger ? 'danger solid' : 'primary' })),
  ]);
}

// ---------- the view: a hub's web app or a page, drawn by the app over a placeholder ----------

let viewWanted = null; // { url, el }
function viewShown() {
  return !!viewWanted && overlays === 0;
}
function syncView() {
  if (!viewWanted || !viewWanted.el.isConnected) {
    call('view', { show: false }).catch(() => {});
    return;
  }
  const r = viewWanted.el.getBoundingClientRect();
  call('view', { show: overlays === 0, url: viewWanted.url, x: r.left, y: r.top, w: r.width, h: r.height }).catch((e) => toast(e.message));
}
function wantView(url, el) {
  viewWanted = url ? { url, el } : null;
  requestAnimationFrame(syncView);
}
window.addEventListener('resize', () => viewWanted && syncView());
on('viewTitle', (v) => {
  const t = document.querySelector('[data-view-title]');
  if (t && v) t.textContent = v;
});
on('viewUrl', (v) => {
  const t = document.querySelector('[data-view-url]');
  if (t && v) t.textContent = v;
});

// ---------- state and screens ----------

let S = null;
let route = { page: 'home' };
const ui = { tempSel: new Set(), fileSel: new Set(), logTab: 'app', forgetting: null, last: '', applying: null };

const PAGES = [
  { id: 'home', label: 'This computer', icon: 'monitor', render: home },
  { id: 'phones', label: 'Phones', icon: 'phone', render: phones },
  { id: 'hub', label: 'Hub', icon: 'hub', render: hub },
  { id: 'pages', label: 'Pages', icon: 'pages', render: pagesScreen },
  { id: 'temp', label: 'Temp files', icon: 'trash', render: temp },
  { id: 'activity', label: 'Activity', icon: 'activity', render: activity },
  { id: 'settings', label: 'Settings', icon: 'settings', render: settings },
];

function go(page, extra = {}) {
  route = { page, ...extra };
  call('remember', { page }).catch(() => {});
  draw(true);
}

function drawNav() {
  const nav = document.getElementById('nav');
  nav.replaceChildren(
    ...PAGES.map((p, i) => {
      const count = p.id === 'phones' && S ? S.phones.length : p.id === 'pages' && S ? S.pages.length : null;
      return [
        i === 2 || i === 5 ? h('div.sep') : null,
        h('button', { type: 'button', 'aria-current': route.page === p.id ? 'page' : null, title: `${p.label} (Ctrl+${i + 1})`, onclick: () => go(p.id) }, icon(p.icon), h('span', p.label), count ? h('span.count', count) : null),
      ];
    }).flat().filter(Boolean),
  );
  const foot = document.getElementById('foot');
  if (!S) return foot.replaceChildren();
  const run = S.app.running;
  foot.replaceChildren(
    h('div', { title: run.length ? `Running: ${run.join(', ')}` : 'Nothing running' }, run.length ? pill(run.join(' · '), '') : pill('Nothing running', 'off')),
    h('div.ver', h('span', `Inyon ${S.version}`), S.update.state === 'available' ? h('button.link', { type: 'button', onclick: () => go('settings') }, `· ${S.update.latest} is out`) : null),
  );
  document.getElementById('host').textContent = S.host;
}

// Redraws the screen when what it shows changed (or `force`), keeping its scroll.
function draw(force) {
  drawNav();
  if (!S) return;
  const page = PAGES.find((p) => p.id === route.page) || PAGES[0];
  const key = JSON.stringify([route, page.deps ? page.deps() : S, ui.forgetting, ui.tempSel.size, ui.fileSel.size, ui.logTab, ui.applying]);
  if (!force && key === ui.last) return;
  ui.last = key;
  const main = document.getElementById('main');
  const old = main.firstElementChild;
  const scroll = old && !force ? old.scrollTop : 0;
  const el = page.render();
  if (old && !force) el.style.animation = 'none';
  main.replaceChildren(el);
  el.scrollTop = scroll;
  if (!el.querySelector('[data-view]')) wantView(null);
}

function head(title, sub, ...actions) {
  return h('div.head', h('div', h('h1', title), sub ? h('p', sub) : null), actions.length ? h('div.actions', actions) : null);
}

// ----- This computer -----

function linkState() {
  const l = S.link;
  if (!l.setUp) return ['Not set up', 'off'];
  if (l.running && !l.addresses.length) return ['Starting', 'off work'];
  if (l.running) return ['Running', ''];
  return ['Stopped', 'attn'];
}

function home() {
  const [state, dot] = linkState();
  const l = S.link;
  const linkCard = h('section.card',
    h('h2', icon('phone'), 'Phone link'),
    h('p.sub', l.setUp ? 'Your phone connects to this computer directly, end to end encrypted.' : 'Set up the link so the Inyon app on your phone can reach this computer.'),
    h('div', { style: 'display:flex;align-items:center;gap:10px;margin-bottom:10px' }, pill(state, dot), l.autostart ? chip('starts at sign-in') : null),
    l.addresses.length ? h('div.rows', l.addresses.map((a) => h('div.row', { style: 'min-height:40px' }, h('div.grow.mono.selectable', a), btn('Copy', () => copy(a), { icon: 'copy', iconOnly: true, cls: 'ghost sm icon', title: 'Copy' })))) : null,
    h('div.foot',
      !l.setUp ? btn('Set up', () => setUp(), { cls: 'primary', icon: 'play' }) : null,
      l.setUp && !l.running ? btn('Start', () => job('link', 'start'), { cls: 'primary', icon: 'play' }) : null,
      l.setUp && l.running ? btn('Restart', () => job('link', 'restart'), { icon: 'rotate' }) : null,
      l.setUp && l.running ? btn('Stop', () => job('link', 'stop'), { icon: 'stop' }) : null,
      btn('Pair a phone', () => { go('phones'); pair(); }, { icon: 'qr', cls: 'ghost' }),
    ),
  );
  return h('div.page', h('div.wrap',
    head(S.host, `${S.platformName} · Inyon ${S.version}`),
    S.notice ? h('div', { style: 'margin-bottom:14px' }, note(S.notice, 'warn')) : null,
    h('div.grid.two', linkCard, reachCard(), hubCard(), phonesCard()),
  ));
}

function reachCard() {
  const r = S.reach;
  const want = ui.applying || r.applying;
  const val = (k) => (want ? want[k] : r[k]);
  const rows = [
    ['tunnel', 'globe', 'Tunnel', 'Any network, through a free Cloudflare tunnel', r.tunnelUrl],
    ['lan', 'wifi', 'LAN', 'Phones on the same Wi-Fi or network', r.lanIp],
    ['vpn', 'shield', 'VPN', r.vpns.length ? 'Your private network' : 'Tailscale, NetBird, ZeroTier or WireGuard', r.vpns.join(' · ')],
  ];
  const flip = (k, v) => {
    const next = { tunnel: val('tunnel'), lan: val('lan'), vpn: val('vpn'), [k]: v };
    if (!next.tunnel && !next.lan && !next.vpn) {
      draw(true);
      return toast('Keep at least one way in, or phones can’t reach this computer.');
    }
    ui.applying = next;
    draw();
    call('reach', next).then(() => toast('Saved. The link restarts with the new settings.')).catch((e) => toast(e.message)).finally(async () => { await refresh(); ui.applying = null; draw(); });
  };
  return h('section.card.reach',
    h('h2', icon('globe'), 'How phones reach it'),
    h('p.sub', 'Turn on each way your phone may connect. Changes apply right away.'),
    h('div.rows', rows.map(([k, ic, t, d, detail]) => h('div.row',
      h('div.ico', icon(ic)),
      h('div.grow', h('div.t', t), h('div.d', d), detail && val(k) ? h('div.d.mono.selectable', detail) : null),
      sw(val(k), (v) => flip(k, v), { label: t, busy: !!want, disabled: !!want || !S.link.setUp }),
    ))),
    want ? h('div', { style: 'margin-top:10px' }, note('Applying… the link restarts with the new settings.')) : null,
    r.error ? h('div', { style: 'margin-top:10px' }, note(r.error, 'err')) : null,
    !want && (r.lan || r.vpn) && r.firewall === false ? h('div', { style: 'margin-top:10px' }, note('Windows Firewall still keeps phones on your network out. Turn LAN or VPN off and on again to add the rule; Windows asks for permission.', 'warn')) : null,
  );
}

function hubCard() {
  const j = S.joined;
  if (!j) {
    return h('section.card',
      h('h2', icon('hub'), 'Hub'),
      h('p.sub', 'A hub shows all your machines and sessions in one place, in the app and in a browser.'),
      h('div.foot', btn('Add this computer to a hub', () => joinDialog(), { cls: 'primary', icon: 'plus' }), btn('Open a hub', () => addHubDialog(), { cls: 'ghost' })),
    );
  }
  const st = j.running ? (j.problem ? ['Can’t connect', 'err'] : ['Connected', '']) : ['Stopped', 'attn'];
  return h('section.card',
    h('h2', icon('hub'), 'Hub'),
    h('p.sub', 'This computer is a machine on your hub; its sessions show there.'),
    h('div', { style: 'display:flex;align-items:center;gap:10px;flex-wrap:wrap' }, pill(st[0], st[1]), chip(j.url)),
    j.problem ? h('div', { style: 'margin-top:10px' }, note(j.problem, 'err')) : null,
    h('div.foot',
      btn('Open the hub', () => { openHub(j.url); }, { cls: 'primary', icon: 'hub' }),
      j.running ? btn('Restart', () => job('node', 'restart'), { icon: 'rotate' }) : btn('Start', () => job('node', 'start'), { icon: 'play' }),
      btn('Disconnect', () => leave(), { cls: 'ghost', icon: 'logout' }),
    ),
  );
}

function phonesCard() {
  const n = S.phones.length;
  const last = S.phones.map((p) => p.lastSeen).filter(Boolean).sort().pop();
  return h('section.card',
    h('h2', icon('phone'), 'Phones'),
    h('p.sub', n ? `${plural(n, 'phone')} paired${last ? `, last seen ${last}` : ''}.` : 'No phone is paired with this computer yet.'),
    h('div.foot', btn('Pair a phone', () => { go('phones'); pair(); }, { cls: n ? '' : 'primary', icon: 'qr' }), n ? btn('Manage', () => go('phones'), { cls: 'ghost' }) : null),
  );
}

async function job(role, action) {
  try {
    const m = await call('job', { role, action });
    toast(m || 'Done.');
  } catch (e) {
    toast(e.message);
  }
  refresh();
}

async function setUp() {
  const pick = await dialog((close) => {
    const v = { tunnel: true, lan: false, vpn: S.reach.vpns.length > 0 };
    const rows = [['tunnel', 'Tunnel', 'Any network, through Cloudflare'], ['lan', 'LAN', 'The same Wi-Fi'], ['vpn', 'VPN', 'Tailscale, NetBird and others']];
    return [
      h('h3', 'Set up the phone link'),
      h('p', 'Pick how your phone may reach this computer. You can change this any time.'),
      h('div.rows', rows.map(([k, t, d]) => h('div.row', h('div.grow', h('div.t', t), h('div.d', d)), sw(v[k], (on) => { v[k] = on; }, { label: t })))),
      h('div.foot', btn('Cancel', () => close(null), { cls: 'ghost' }), btn('Set up', () => close(v), { cls: 'primary' })),
    ];
  });
  if (!pick) return;
  if (!pick.tunnel && !pick.lan && !pick.vpn) return toast('Keep at least one way in.');
  const t = toast('Setting up… this takes a moment.', null, 60000);
  try {
    await call('setup', pick);
    t();
    toast('The phone link is set up and running.');
  } catch (e) {
    t();
    toast(e.message, null, 8000);
  }
  refresh();
}

// ----- Phones -----

function phones() {
  const p = S.pairing;
  const list = S.phones.filter((x) => x.key !== ui.forgetting);
  return h('div.page', h('div.wrap',
    head('Phones', 'Phones paired with this computer connect to it directly, with or without a hub.', btn(p.card === 'idle' || p.card === 'used' || p.card === 'expired' ? 'Pair a phone' : 'New code', () => pair(), { cls: 'primary', icon: 'qr', disabled: ['asking', 'starting', 'slow'].includes(p.card) })),
    p.card !== 'idle' ? pairingCard(p) : null,
    h('section.card', { style: 'margin-top:14px' },
      h('h2', 'Paired phones'),
      list.length
        ? h('div.rows', list.map((ph) => {
            const items = () => [{ label: 'Forget this phone', icon: 'trash', danger: true, run: () => forget(ph) }];
            return withMenu(h('div.row', h('div.ico', { style: 'width:34px;height:34px;border-radius:10px;display:grid;place-items:center;background:var(--raised);border:1px solid var(--line)' }, icon('phone')), h('div.grow', h('div.t', ph.name), h('div.d', `paired ${ph.paired} · ${ph.lastSeen ? 'seen ' + ph.lastSeen : 'never seen'}`)), btn('More', (e, b) => menuAt(b, items()), { icon: 'more', iconOnly: true, cls: 'ghost sm icon' })), items);
          }))
        : h('div.empty', h('div.big', icon('phone')), h('h2', 'No phones yet'), h('p', 'Pair your phone by scanning a code with the Inyon app.')),
    ),
  ));
}

function pairingCard(p) {
  const waiting = ['asking', 'starting', 'slow'].includes(p.card);
  const left = p.expiresAt ? Math.max(0, Math.round((p.expiresAt - Date.now()) / 60000)) : null;
  const words = {
    asking: ['Making a code…', 'Asking the link for a pairing code.'],
    starting: ['Starting the tunnel…', 'So your phone can reach this computer from any network. This takes a few seconds.'],
    slow: ['The tunnel is still starting', 'The code shows here once it’s up. Activity has the log.'],
    code: ['Scan with the Inyon app', p.lan ? 'For now this works on the same network or VPN; it changes once the tunnel is up.' : `It works once${left != null ? `, for ${plural(left, 'more minute')}` : ''}.`],
    used: ['Paired', `${p.phone || 'Your phone'} is paired. Make another code for the next phone.`],
    expired: ['That code expired', 'Codes last 10 minutes. Make a new one when your phone is ready.'],
    failed: ['Couldn’t make a code', p.error || 'The link didn’t answer.'],
  }[p.card] || ['', ''];
  return h('section.card',
    h('div.pair',
      p.card === 'code' && p.qr ? h('div.qr', { html: p.qr, 'aria-label': 'Pairing code' }) : h('div.qr.empty', icon(waiting ? 'qr' : p.card === 'used' ? 'check' : p.card === 'failed' ? 'alert' : 'phone')),
      h('div',
        h('div.big-status', words[0]),
        h('p', { style: 'color:var(--fg-3);margin:6px 0 14px' }, words[1]),
        p.card === 'code' && p.id ? h('div', { style: 'display:flex;gap:18px;flex-wrap:wrap;margin-bottom:14px' }, h('div', h('div.hint', 'ID'), h('div.code.selectable', p.id)), h('div', h('div.hint', 'Code'), h('div.code.selectable', p.code))) : null,
        h('div', { style: 'display:flex;gap:8px;flex-wrap:wrap' },
          p.card === 'code' && p.link ? btn('Copy link', () => copy(p.link), { icon: 'copy' }) : null,
          waiting || p.card === 'code' ? btn('Cancel', () => call('pairCancel').then(refresh), { cls: 'ghost' }) : null,
          p.card === 'slow' || p.card === 'failed' ? btn('Show the log', () => { ui.logTab = 'link'; go('activity'); }, { cls: 'ghost' }) : null,
        ),
      ),
    ),
  );
}

async function pair() {
  try {
    await call('pair');
  } catch (e) {
    toast(e.message);
  }
  refresh();
}

function forget(ph) {
  ui.forgetting = ph.key;
  draw();
  let undone = false;
  toast(`Forgot ${ph.name}.`, { label: 'Undo', run: () => { undone = true; ui.forgetting = null; draw(); } }, 5000);
  setTimeout(async () => {
    if (undone) return;
    try {
      await call('forget', { key: ph.key });
    } catch (e) {
      toast(e.message);
    }
    ui.forgetting = null;
    refresh();
  }, 5000);
}

// ----- Hub -----

function openHub(url) {
  call('settings', { patch: { activeHub: url } }).then((s) => { S.settings = s; go('hub'); }).catch((e) => toast(e.message));
}

function hub() {
  const hubs = S.hubs;
  if (!hubs.length) {
    return h('div.page', h('div.wrap',
      head('Hub', 'A hub shows every machine and session in one place, and keeps them going while this computer sleeps.'),
      h('div.grid.two',
        h('section.card', h('h2', icon('plus'), 'Add this computer to a hub'), h('p.sub', 'Paste the command from your hub’s Machines page (Add a machine). This computer then shows up there, with its sessions.'), h('div.foot', btn('Paste the command…', () => joinDialog(), { cls: 'primary' }))),
        h('section.card', h('h2', icon('globe'), 'Open a hub'), h('p.sub', 'See a hub you already run, its machines, sessions and pages, here in this window. You sign in once.'), h('div.foot', btn('Enter its address…', () => addHubDialog(), { cls: 'primary' }))),
        h('section.card', h('h2', icon('hub'), 'Create a hub'), h('p.sub', S.wsl ? 'Run a hub on this computer, inside WSL. Your other machines and phones then connect to it.' : 'Hubs run on Linux and macOS. On Windows a hub runs inside WSL, which isn’t installed here yet.'), h('div.foot', btn(S.wsl ? 'Create a hub in WSL…' : 'How to install WSL', () => createHub(), { cls: S.wsl ? '' : 'ghost' }))),
      ),
    ));
  }
  const active = hubs.find((x) => x.url === S.settings.activeHub) || hubs[0];
  const vp = h('div.viewport', { 'data-view': '' }, h('div.loading', 'Loading the hub…'));
  const pick = btn(active.name, (e, b) => menuAt(b, [
    { heading: 'Hubs' },
    ...hubs.map((x) => ({ label: x.name, checked: x.url === active.url, run: () => openHub(x.url) })),
    'sep',
    { label: 'Open another hub…', icon: 'plus', run: () => addHubDialog() },
    { label: 'Add this computer to a hub…', icon: 'plus', run: () => joinDialog() },
    'sep',
    { label: active.joined ? 'Disconnect this computer' : 'Remove from this list', icon: active.joined ? 'logout' : 'x', danger: true, run: () => (active.joined ? leave() : removeHub(active)) },
  ]), { icon: 'down', cls: 'ghost' });
  const page = h('div.page.flush',
    h('div.bar',
      h('div.picker', pick),
      h('span.url.mono', { 'data-view-url': '' }, active.url),
      h('div.spacer'),
      btn('Back', () => call('viewGo', { to: 'back' }), { icon: 'back', iconOnly: true, cls: 'ghost icon', title: 'Back' }),
      btn('Reload', () => call('viewGo', { to: 'reload' }), { icon: 'refresh', iconOnly: true, cls: 'ghost icon', title: 'Reload' }),
      btn('Open in your browser', () => call('openUrl', { url: active.url }), { icon: 'external', iconOnly: true, cls: 'ghost icon', title: 'Open in your browser' }),
    ),
    vp,
  );
  wantView(active.url, vp);
  return page;
}

function joinDialog() {
  return dialog((close) => {
    const ta = h('textarea.input', { placeholder: "INYON_TOKEN=… sh -c '…' inyon https://hub…   or   powershell … -Hub '…' -Token '…'", autofocus: true, spellcheck: 'false' });
    const err = h('div.err-text');
    const go_ = btn('Add this computer', async () => {
      err.textContent = '';
      try {
        const m = await call('join', { text: ta.value });
        close(true);
        toast(m || 'This computer is on the hub now.');
        refresh();
      } catch (e) {
        err.textContent = e.message;
      }
    }, { cls: 'primary' });
    return [
      h('h3', 'Add this computer to a hub'),
      h('p', 'On your hub, open Machines and choose Add a machine, then copy the Windows or the Linux command and paste it here. Nothing runs in a terminal; the token is used once.'),
      h('div.field', ta, err),
      h('div.foot', btn('Cancel', () => close(false), { cls: 'ghost' }), go_),
    ];
  }, { wide: true });
}

function addHubDialog() {
  return dialog((close) => {
    const url = h('input.input', { placeholder: 'https://hub.example.com or http://100.64.0.5:4777', autofocus: true, spellcheck: 'false' });
    const name = h('input.input', { placeholder: 'Optional' });
    const err = h('div.err-text');
    const add = btn('Open', async () => {
      err.textContent = '';
      try {
        const s = await call('hubAdd', { url: url.value, name: name.value });
        S.hubs = s.hubs;
        S.settings = s.settings;
        close(true);
        go('hub');
      } catch (e) {
        err.textContent = e.message;
      }
    }, { cls: 'primary' });
    url.addEventListener('keydown', (e) => e.key === 'Enter' && add.click());
    return [
      h('h3', 'Open a hub'),
      h('p', 'Its machines, sessions and pages open in this window. You sign in with the hub’s password once.'),
      h('div.field', h('label', 'Address'), url),
      h('div.field', h('label', 'Name'), name),
      err,
      h('div.foot', btn('Cancel', () => close(false), { cls: 'ghost' }), add),
    ];
  });
}

async function removeHub(x) {
  const s = await call('hubRemove', { url: x.url }).catch((e) => toast(e.message));
  if (s) { S.hubs = s.hubs; S.settings = s.settings; draw(true); }
}

async function leave() {
  const ok = await confirmBox('Disconnect from the hub?', `This computer stops being a machine on ${S.joined ? S.joined.url : 'the hub'}. Its sessions keep running here; add it again any time.`, 'Disconnect', { danger: true });
  if (!ok) return;
  try {
    toast(await call('leave'));
  } catch (e) {
    toast(e.message);
  }
  refresh();
}

async function createHub() {
  if (!S.wsl) return call('openUrl', { url: 'https://learn.microsoft.com/windows/wsl/install' });
  const ok = await confirmBox('Create a hub in WSL?', `Inyon installs a hub in your WSL distribution (${S.wsl}) and starts it. It takes a minute or two; then you set its password here.`, 'Create the hub');
  if (!ok) return;
  const t = toast('Creating the hub in WSL…', null, 300000);
  try {
    const url = await call('hubCreate');
    t();
    toast('The hub is running.');
    openHub(url);
    refresh();
  } catch (e) {
    t();
    toast(e.message, null, 10000);
  }
}

// ----- Pages -----

function pagesScreen() {
  if (route.open) {
    const pg = S.pages.find((p) => p.slug === route.open);
    if (pg) {
      const vp = h('div.viewport', { 'data-view': '' }, h('div.loading', 'Opening…'));
      const page = h('div.page.flush',
        h('div.bar',
          btn('All pages', () => go('pages'), { icon: 'left', cls: 'ghost' }),
          h('span.title', { 'data-view-title': '' }, pg.name),
          h('span.url.mono', pg.path),
          h('div.spacer'),
          btn('Reload', () => call('viewGo', { to: 'reload' }), { icon: 'refresh', iconOnly: true, cls: 'ghost icon', title: 'Reload' }),
          btn('Open the folder', () => call('openPath', { path: pg.path }), { icon: 'folder', iconOnly: true, cls: 'ghost icon', title: 'Open the folder' }),
        ),
        vp,
      );
      wantView(pg.url, vp);
      return page;
    }
  }
  const items = (pg) => [
    { label: 'Open', icon: 'external', run: () => go('pages', { open: pg.slug }) },
    { label: 'Open the folder', icon: 'folder', run: () => call('openPath', { path: pg.path }) },
    'sep',
    { label: 'Remove from Pages', icon: 'x', danger: true, run: () => call('pageRemove', { slug: pg.slug }).then(() => { toast(`${pg.name} is off the list; its folder stays.`); refresh(); }).catch((e) => toast(e.message)) },
  ];
  return h('div.page', h('div.wrap',
    head('Pages', 'Sites your agents made on this computer, like plans and mockups. Pages on your hub’s machines are under Hub › Pages.', btn('Add a folder', () => addPage(), { icon: 'plus', cls: 'primary' })),
    h('section.card', S.pages.length
      ? h('div.rows', S.pages.map((pg) => withMenu(h('div.row.click', { onclick: () => go('pages', { open: pg.slug }) }, icon('pages'), h('div.grow', h('div.t', pg.name), h('div.d.mono', pg.path)), pg.project ? chip(pg.project) : null, btn('More', (e, b) => menuAt(b, items(pg)), { icon: 'more', iconOnly: true, cls: 'ghost sm icon' })), () => items(pg))))
      : h('div.empty', h('div.big', icon('pages')), h('h2', 'No pages yet'), h('p', 'When an agent builds a plan or mockup site, it adds it here. You can add any folder with an index.html.'), btn('Add a folder', () => addPage(), { icon: 'plus' }))),
  ));
}

async function addPage() {
  let path;
  try {
    path = await call('pickFolder');
  } catch (e) {
    return toast(e.message);
  }
  if (!path) return;
  try {
    const m = await call('pageAdd', { path });
    toast(m);
    refresh();
  } catch (e) {
    toast(e.message, null, 7000);
  }
}

// ----- Temp files -----

let tempData = null;
let tempLoading = false;
let fileData = null;

async function loadTemp(force) {
  if (tempLoading || (tempData && !force)) return;
  tempLoading = true;
  try {
    tempData = await call('temp');
  } catch (e) {
    tempData = { error: e.message, roots: [], items: [] };
  }
  tempLoading = false;
  ui.tempSel = new Set([...ui.tempSel].filter((p) => tempData.items.some((i) => i.path === p)));
  draw(true);
}

async function loadFiles(path) {
  fileData = { path, loading: true, files: [] };
  draw(true);
  try {
    fileData = { path, files: await call('tempFiles', { path }) };
  } catch (e) {
    fileData = { path, error: e.message, files: [] };
  }
  draw(true);
}

const fileUrl = (p) => `temp-file?path=${encodeURIComponent(p)}`;

function temp() {
  if (route.item) return tempItem();
  if (!tempData) {
    loadTemp();
    return h('div.page', h('div.wrap', head('Temp files', 'Looking through the temporary folders agents use…'), h('section.card', [1, 2, 3, 4].map(() => h('div.skel', { style: 'margin:14px 0;width:' + (40 + Math.random() * 50) + '%' })))));
  }
  const d = tempData;
  const items = d.items;
  const sel = items.filter((i) => ui.tempSel.has(i.path));
  const selSize = sel.reduce((n, i) => n + i.size, 0);
  const all = items.length && sel.length === items.length ? true : sel.length ? 'mixed' : false;
  const total = d.roots.reduce((n, r) => n + (r.size || 0), 0);
  const toggle = (p, e) => {
    // a range from the last row clicked, while it's still listed (it may have been deleted)
    const a = e && e.shiftKey ? items.findIndex((i) => i.path === ui.lastTemp) : -1;
    const b = items.findIndex((i) => i.path === p);
    if (a >= 0 && b >= 0) {
      for (let i = Math.min(a, b); i <= Math.max(a, b); i++) ui.tempSel.add(items[i].path);
    } else if (ui.tempSel.has(p)) ui.tempSel.delete(p);
    else ui.tempSel.add(p);
    ui.lastTemp = p;
    draw();
  };
  const rowMenu = (it) => [
    { label: 'Open', icon: 'right', run: () => openItem(it) },
    { label: 'Show in Explorer', icon: 'folder', run: () => call('openPath', { path: it.path, reveal: true }) },
    { label: 'Copy the path', icon: 'copy', run: () => copy(it.path) },
    'sep',
    { label: 'Delete', icon: 'trash', danger: true, run: () => removeTemp([it]) },
  ];
  return h('div.page', h('div.wrap',
    head('Temp files', `What agents left in temporary folders on this computer: pasted images, scratchpads and uploads. ${total ? size(total) + ' in all.' : ''}`,
      btn('Refresh', () => loadTemp(true), { icon: 'refresh', cls: 'ghost' })),
    d.error ? note(d.error, 'err') : null,
    h('div.totals', d.roots.filter((r) => r.exists !== false).map((r) => chip(`${r.label} · ${size(r.size)}`, r.path))),
    h('div.toolbar',
      checkbox(all, () => { if (all === true) ui.tempSel.clear(); else items.forEach((i) => ui.tempSel.add(i.path)); draw(); }, 'Select all'),
      h('span.hint', sel.length ? `${sel.length} selected · ${size(selSize)}` : `${plural(items.length, 'item')}`),
      btn('Older than a week', () => { ui.tempSel = new Set(items.filter((i) => Date.now() - i.modified > 7 * 86400000 && !i.recent).map((i) => i.path)); draw(); }, { cls: 'ghost sm' }),
      h('div.grow'),
      sel.length ? btn(`Delete ${size(selSize)}`, () => removeTemp(sel), { icon: 'trash', cls: 'danger' }) : null,
    ),
    h('section.card', { style: 'padding:6px 20px' }, items.length
      ? h('div.rows', items.map((it) => withMenu(h('div.row.click.item', { class: ui.tempSel.has(it.path) ? 'sel' : null, onclick: () => openItem(it) },
          checkbox(ui.tempSel.has(it.path), (e) => toggle(it.path, e), `Select ${it.name}`),
          it.images && it.images.length ? h('div.thumbs', it.images.slice(0, 3).map((p) => h('img', { src: fileUrl(p), loading: 'lazy', alt: '' }))) : h('div.thumbs', h('div', { style: 'width:36px;height:36px;border-radius:8px;display:grid;place-items:center;background:var(--raised);border:1px solid var(--line);color:var(--fg-3)' }, icon(it.kind === 'dir' ? 'folder' : 'file'))),
          h('div.grow', h('div.t', it.name), h('div.d', `${rootLabel(it.root)} · ${it.kind === 'dir' ? plural(it.files, 'file') : 'file'}`, it.recent ? ' · in use?' : '')),
          h('span.size', size(it.size)),
          h('span.age', age(it.modified)),
          icon('right'),
        ), () => rowMenu(it))))
      : h('div.empty', h('div.big', icon('check')), h('h2', 'Nothing to clean up'), h('p', 'No agent left temporary files on this computer.'))),
  ));
}

const rootLabel = (id) => ((tempData && tempData.roots.find((r) => r.id === id)) || { label: id }).label;

function openItem(it) {
  if (it.kind !== 'dir') return previewFile({ path: it.path, rel: it.name, size: it.size, kind: guessKind(it.path) });
  ui.fileSel.clear();
  route = { page: 'temp', item: it.path };
  loadFiles(it.path);
}

const guessKind = (p) => (/\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(p) ? 'image' : /\.(txt|md|json|log|js|ts|tsx|rs|py|html?|css|ya?ml|toml|csv|sh|ps1)$/i.test(p) ? 'text' : 'other');

function tempItem() {
  const it = (tempData && tempData.items.find((i) => i.path === route.item)) || { name: route.item, path: route.item };
  const f = fileData && fileData.path === route.item ? fileData : { loading: true, files: [] };
  const files = f.files || [];
  const sel = files.filter((x) => ui.fileSel.has(x.path));
  const all = files.length && sel.length === files.length ? true : sel.length ? 'mixed' : false;
  const tileMenu = (x) => [
    { label: 'Preview', icon: 'image', run: () => previewFile(x) },
    { label: 'Show in Explorer', icon: 'folder', run: () => call('openPath', { path: x.path, reveal: true }) },
    { label: 'Copy the path', icon: 'copy', run: () => copy(x.path) },
    'sep',
    { label: 'Delete', icon: 'trash', danger: true, run: () => removeFiles([x]) },
  ];
  return h('div.page', h('div.wrap',
    h('div', { style: 'margin-bottom:10px' }, btn('Temp files', () => go('temp'), { icon: 'left', cls: 'ghost sm' })),
    head(it.name, it.path, btn('Show in Explorer', () => call('openPath', { path: it.path, reveal: true }), { icon: 'folder', cls: 'ghost' }), btn(`Delete all${it.size ? ' · ' + size(it.size) : ''}`, () => removeTemp([it]).then((done) => done && go('temp')), { icon: 'trash', cls: 'danger' })),
    f.error ? note(f.error, 'err') : null,
    h('div.toolbar',
      checkbox(all, () => { if (all === true) ui.fileSel.clear(); else files.forEach((x) => ui.fileSel.add(x.path)); draw(); }, 'Select all'),
      h('span.hint', f.loading ? 'Loading…' : sel.length ? `${sel.length} selected · ${size(sel.reduce((n, x) => n + x.size, 0))}` : plural(files.length, 'file')),
      h('div.grow'),
      sel.length ? btn('Delete selected', () => removeFiles(sel), { icon: 'trash', cls: 'danger' }) : null,
    ),
    h('div.files-grid', files.map((x) => withMenu(h('div.tile', { class: ui.fileSel.has(x.path) ? 'sel' : null, onclick: () => previewFile(x) },
      h('div.pic', x.kind === 'image' ? h('img', { src: fileUrl(x.path), loading: 'lazy', alt: '' }) : icon(x.kind === 'text' ? 'file' : 'file')),
      h('div.cap', h('div', { title: x.rel }, x.rel), h('span', `${size(x.size)} · ${age(x.modified)}`)),
      checkbox(ui.fileSel.has(x.path), () => { ui.fileSel.has(x.path) ? ui.fileSel.delete(x.path) : ui.fileSel.add(x.path); draw(); }, `Select ${x.rel}`),
    ), () => tileMenu(x)))),
  ));
}

async function previewFile(x) {
  let body;
  if (x.kind === 'image') body = h('img', { src: fileUrl(x.path), alt: x.rel });
  else {
    body = h('pre', 'Loading…');
    call('tempPreview', { path: x.path }).then((p) => { body.textContent = p.text != null ? p.text + (p.truncated ? '\n\n… (cut short)' : '') : 'This file can’t be shown as text.'; }).catch((e) => { body.textContent = e.message; });
  }
  await dialog((close) => {
    const box = h('div.preview',
      h('div.bar', h('span.title', x.rel), h('span.url.mono', size(x.size)), h('div.spacer'),
        btn('Copy the path', () => copy(x.path), { icon: 'copy', iconOnly: true, cls: 'ghost icon', title: 'Copy the path' }),
        btn('Show in Explorer', () => call('openPath', { path: x.path, reveal: true }), { icon: 'folder', iconOnly: true, cls: 'ghost icon', title: 'Show in Explorer' }),
        btn('Delete', async () => { if (await removeFiles([x])) close(); }, { icon: 'trash', iconOnly: true, cls: 'ghost icon', title: 'Delete' }),
        btn('Close', () => close(), { icon: 'x', iconOnly: true, cls: 'ghost icon', title: 'Close' })),
      h('div.body', body));
    return box;
  });
}

async function removeTemp(list) {
  const total = list.reduce((n, i) => n + i.size, 0);
  const recent = list.filter((i) => i.recent).length;
  const ok = await confirmBox(
    `Delete ${list.length === 1 ? list[0].name : plural(list.length, 'item')}?`,
    h('div', h('p', `This frees ${size(total)}. It can’t be undone.`), recent ? note(`${plural(recent, 'item')} changed in the last 30 minutes; an agent may still be using ${recent === 1 ? 'it' : 'them'}.`, 'warn') : null),
    'Delete',
    { danger: true },
  );
  if (!ok) return false;
  return doRemove(list.map((i) => i.path));
}

async function removeFiles(list) {
  const ok = await confirmBox(`Delete ${list.length === 1 ? list[0].rel : plural(list.length, 'file')}?`, `This frees ${size(list.reduce((n, x) => n + x.size, 0))}. It can’t be undone.`, 'Delete', { danger: true });
  if (!ok) return false;
  const done = await doRemove(list.map((x) => x.path));
  if (done && route.item) loadFiles(route.item);
  return done;
}

async function doRemove(paths) {
  try {
    const r = await call('tempRemove', { paths });
    paths.forEach((p) => { ui.tempSel.delete(p); ui.fileSel.delete(p); });
    toast(r.failed && r.failed.length ? `Freed ${size(r.freed)}; ${plural(r.failed.length, 'item')} couldn’t be deleted: ${r.failed[0][1]}` : `Freed ${size(r.freed)}.`, null, 6000);
    loadTemp(true);
    return true;
  } catch (e) {
    toast(e.message, null, 6000);
    return false;
  }
}

// ----- Activity -----

let logText = '';
let checks = null;

function activity() {
  if (!checks) runChecks();
  const tabs = [['app', 'Inyon app'], ['link', 'Phone link'], ['node', 'Hub connection'], ['update', 'Updates']];
  const pre = h('pre.log', logText || 'Loading…');
  setTimeout(() => (pre.scrollTop = pre.scrollHeight), 0);
  return h('div.page', h('div.wrap',
    head('Activity', 'What Inyon checked and what it wrote down.'),
    h('section.card',
      h('h2', 'Checks', h('span', { style: 'margin-left:auto' }, btn('Run again', () => runChecks(true), { icon: 'refresh', cls: 'ghost sm' }))),
      !checks || checks.running ? h('p.sub', 'Running the checks…') : h('div.rows', checks.list.map((c) => h('div.row.check-row', { style: 'align-items:flex-start;min-height:40px' }, h('span.dot', { class: c.ok ? '' : 'err' }), h('div.grow', h('div.t', c.check), c.detail ? h('div.d.selectable', { style: 'white-space:normal' }, c.detail) : null)))),
    ),
    h('section.card', { style: 'margin-top:14px' },
      h('h2', 'Logs', h('span', { style: 'margin-left:auto;display:flex;gap:6px' }, btn('Refresh', () => loadLog(), { icon: 'refresh', iconOnly: true, cls: 'ghost sm icon', title: 'Refresh' }), btn('Open the folder', () => call('openPath', { path: 'logs' }), { icon: 'folder', iconOnly: true, cls: 'ghost sm icon', title: 'Open the folder' }))),
      h('div.tabs', { role: 'tablist' }, tabs.map(([k, l]) => h('button', { type: 'button', role: 'tab', 'aria-selected': ui.logTab === k ? 'true' : 'false', onclick: () => { ui.logTab = k; logText = ''; draw(true); loadLog(); } }, l))),
      pre,
    ),
  ));
}

async function runChecks(again) {
  if (checks && checks.running) return;
  checks = { running: true, list: checks ? checks.list : [] };
  if (again) draw(true);
  try {
    checks = { list: await call('checks') };
  } catch (e) {
    checks = { list: [{ ok: false, check: 'doctor', detail: e.message }] };
  }
  if (route.page === 'activity') draw(true);
}

async function loadLog() {
  try {
    logText = await call('log', { which: ui.logTab, lines: 400 });
  } catch (e) {
    logText = e.message;
  }
  const pre = document.querySelector('pre.log');
  if (pre && route.page === 'activity') {
    const atEnd = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 40;
    pre.textContent = logText || 'Nothing written yet.';
    if (atEnd) pre.scrollTop = pre.scrollHeight;
  }
}

// ----- Settings -----

function settings() {
  const st = S.settings;
  const set = (patch) => call('settings', { patch }).then((s) => { S.settings = s; applyTheme(); draw(true); }).catch((e) => toast(e.message));
  const u = S.update;
  const upd = {
    looking: 'Looking for updates…',
    current: 'You have the newest version.',
    available: `${u.latest} is out.`,
    updating: `Updating to ${u.latest}… Inyon restarts when it’s done.`,
    done: `Updated to ${u.latest}.`,
    failed: 'Couldn’t check for updates. Activity has the log.',
  }[u.state] || '';
  const row = (t, d, control) => h('div.row', h('div.grow', h('div.t', t), d ? h('div.d', { style: 'white-space:normal' }, d) : null), control);
  return h('div.page', h('div.wrap',
    head('Settings'),
    h('div.grid',
      h('section.card', h('h2', 'General'), h('div.rows',
        row('Start Inyon when you sign in', 'The phone link and the hub connection start in the background.', sw(S.link.autostart || (S.joined && S.joined.autostart), (v) => call('autostart', { on: v }).then((m) => { toast(m); refresh(); }).catch((e) => toast(e.message)), { label: 'Start at sign-in', disabled: !S.link.setUp && !S.joined })),
        row('Show this window at sign-in', 'Otherwise Inyon starts in the notification area.', sw(st.openAtSignIn, (v) => set({ openAtSignIn: v }), { label: 'Show the window at sign-in' })),
        row('Closing the window', st.closeToTray ? 'Inyon keeps running in the notification area.' : 'Inyon quits, and everything it runs stops.', seg([[true, 'Keep running'], [false, 'Quit']], st.closeToTray, (v) => set({ closeToTray: v }))),
      )),
      h('section.card', h('h2', 'Appearance'), h('div.rows', row('Theme', null, seg([['system', 'System'], ['light', 'Light'], ['dark', 'Dark']], st.theme, (v) => set({ theme: v }))))),
      h('section.card', h('h2', 'Updates'), h('div.rows',
        row(`Inyon ${S.version}`, upd, u.state === 'available' ? btn(`Update to ${u.latest}`, () => call('update', { install: true }).then(refresh).catch((e) => toast(e.message)), { cls: 'primary', icon: 'download' }) : btn('Check now', () => call('update', { install: false }).then(refresh).catch((e) => toast(e.message)), { cls: 'ghost', disabled: u.state === 'looking' || u.state === 'updating' })),
        row('Check for updates by itself', 'Once a day, and when Inyon starts.', sw(st.checkUpdates, (v) => set({ checkUpdates: v }), { label: 'Check for updates by itself' })),
      )),
      h('section.card', h('h2', 'Phone link'), h('div.rows',
        row('Port', 'Where the link listens on this computer, for the LAN, a VPN and the tunnel.', h('span.mono', S.link.port)),
        row('Pairing and phones', null, btn('Phones', () => go('phones'), { cls: 'ghost' })),
      )),
      h('section.card', h('h2', 'About'), h('div.rows',
        row('Documentation', 'How Inyon works, and what each setting does.', btn('inyon.tech', () => call('openUrl', { url: 'https://inyon.tech/docs/' }), { icon: 'external', cls: 'ghost' })),
        row('The Inyon folder', 'Settings, pairings and logs.', btn('Open', () => call('openPath', { path: 'inyon' }), { icon: 'folder', cls: 'ghost' })),
        row('Quit Inyon', 'Stops everything Inyon runs on this computer until you open it again.', btn('Quit', () => quit(), { icon: 'power', cls: 'ghost' })),
        row('Uninstall Inyon', 'Removes Inyon, its pairings and logs from this computer.', btn('Uninstall…', () => uninstall(), { cls: 'danger' })),
      )),
    ),
  ));
}

async function quit() {
  const ok = await confirmBox('Quit Inyon?', 'The phone link and the hub connection stop until you open Inyon again or sign in.', 'Quit');
  if (ok) call('quit').catch(() => {});
}

async function uninstall() {
  const ok = await confirmBox('Uninstall Inyon?', 'What it runs stops, and its programs, pairings and logs are deleted from this computer. Your agents and their sessions stay.', 'Uninstall', { danger: true });
  if (!ok) return;
  try {
    toast(await call('uninstall'), null, 10000);
  } catch (e) {
    toast(e.message, null, 8000);
  }
}

// ---------- the rest ----------

async function copy(text) {
  try {
    await call('copy', { text });
    toast('Copied.');
  } catch (e) {
    toast(e.message);
  }
}

let themeSent = null;
function applyTheme() {
  const want = S ? S.settings.theme : 'system';
  const dark = want === 'dark' || (want === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  // the title bar and the app's own menus follow
  if (themeSent !== dark) {
    themeSent = dark;
    call('theme', { dark }).catch(() => {});
  }
}
matchMedia('(prefers-color-scheme: dark)').addEventListener('change', applyTheme);

let refreshing = false;
async function refresh() {
  if (refreshing) return;
  refreshing = true;
  try {
    const first = !S;
    S = await call('state');
    if (first) {
      route = { page: S.settings.lastPage && PAGES.some((p) => p.id === S.settings.lastPage) ? S.settings.lastPage : 'home' };
      if (S.startPage) route = { page: S.startPage };
    }
    applyTheme();
    draw(first);
  } catch (e) {
    console.error(e);
  }
  refreshing = false;
}

on('state', (s) => { S = s; applyTheme(); draw(); });
on('go', (v) => { go(v.page, v.extra || {}); if (v.pair) pair(); });
on('toast', (t) => toast(t));

refresh();
setInterval(() => { if (!document.hidden) refresh(); }, 1500);
setInterval(() => { if (!document.hidden && route.page === 'activity') loadLog(); }, 3000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
