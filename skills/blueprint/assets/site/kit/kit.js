/* Blueprint kit. Plain JS, no build. Every screen page loads, in order:
   kit/blueprint.css, kit/lucide.min.js, kit/data.js, kit/kit.js, kit/menus.js, kit/shell.js
   API on window.K (see references/kit.md in the skill). */
(function () {
  const K = (window.K = {});
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  K.$ = $; K.$$ = $$;
  K.esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  K.el = (html) => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
  K.icon = (name, cls = '') => `<i data-lucide="${name}" class="${cls}"></i>`;
  K.icons = (root = document) => { if (window.lucide) lucide.createIcons({ attrs: { 'stroke-width': 1.75 }, root }); };
  K.isPhone = () => window.innerWidth < 640;
  K.kbd = (combo) => `<span class="kbd">${combo.split('+').map((k) => `<span>${K.esc(k)}</span>`).join('')}</span>`;

  /* ---------- theme ---------- */
  const params = new URLSearchParams(location.search);
  const THEMES = ['light', 'dark'];
  function setTheme(t) { if (!THEMES.includes(t)) return; document.documentElement.dataset.theme = t; try { localStorage.setItem('bp-theme', t); } catch {} }
  let initial = params.get('theme');
  if (!THEMES.includes(initial)) { try { initial = localStorage.getItem('bp-theme'); } catch {} }
  if (!THEMES.includes(initial)) initial = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  setTheme(initial);
  K.setTheme = setTheme;
  // Messages count only from the page this one is framed in (the viewer, or Inyon), never from a
  // frame inside it or another window. K.fromParent(e) is that check, for pages adding their own.
  K.fromParent = (e) => window.parent !== window && e.source === window.parent && !!e.data && typeof e.data.type === 'string';
  window.addEventListener('message', (e) => {
    if (!K.fromParent(e) || e.data.type !== 'theme' || !THEMES.includes(e.data.value)) return;
    setTheme(e.data.value);
    document.dispatchEvent(new CustomEvent('bp:theme', { detail: e.data.value }));
  });
  // Framed (in Inyon, or in the viewer): ask for the theme now rather than wait for the frame's load
  // event, since a link followed inside the frame drops ?theme= and the sandbox has no localStorage.
  if (window.parent !== window) { try { window.parent.postMessage({ type: 'theme?' }, '*'); } catch {} }

  /* ---------- marks: letter tiles, or real logos from window.MARKS / window.BRAND ---------- */
  const tile = (letters, size) => `<svg viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><rect width="24" height="24" rx="5.5" fill="var(--logo-tile)"/><text x="12" y="16" text-anchor="middle" font-family="Geist Mono, monospace" font-size="${letters.length > 1 ? 9 : 11}" font-weight="600" fill="var(--panel)">${K.esc(letters)}</text></svg>`;
  // K.mark('github') -> window.MARKS.github (an SVG string with {size}) if set, else a letter tile.
  K.mark = function (name, size = 18) {
    const m = window.MARKS && window.MARKS[name];
    if (m) return m.replace(/\{size\}/g, size);
    return tile((name || '?').slice(0, 1).toUpperCase(), size);
  };
  // K.logo() -> the product's own mark: window.BRAND.logo (SVG string with {size}) or its first letter.
  K.logo = function (size = 20) {
    const b = window.BRAND || {};
    if (b.logo) return b.logo.replace(/\{size\}/g, size);
    return tile((b.name || 'B').slice(0, 1).toUpperCase(), size);
  };

  /* ---------- menus ---------- */
  let openMenu = null;
  function closeMenu(instant) {
    if (!openMenu) return;
    const m = openMenu; openMenu = null;
    m.scrim && m.scrim.remove();
    if (instant) { m.el.remove(); return; }
    m.el.classList.add('closing'); setTimeout(() => m.el.remove(), 120);
    if (m.opts.onClose) m.opts.onClose();
    if (m.returnFocus && document.contains(m.returnFocus)) m.returnFocus.focus({ preventScroll: true });
  }
  function renderItems(menu, items, stack) {
    menu.innerHTML = '';
    if (menu.classList.contains('as-sheet')) menu.appendChild(K.el('<div class="grab"></div>'));
    if (stack.length) {
      const back = K.el(`<button class="mi" role="menuitem"><span class="ic">${K.icon('chevron-left')}</span><span>${K.esc(stack[stack.length - 1].label)}</span></button>`);
      back.onclick = () => { stack.pop(); renderItems(menu, stack.length ? stack[stack.length - 1].sub : menu._root, stack); };
      menu.appendChild(back); menu.appendChild(K.el('<div class="sep"></div>'));
    }
    for (const it of items) {
      if (!it) continue;
      if (it.sep) { menu.appendChild(K.el('<div class="sep" role="separator"></div>')); continue; }
      if (it.header) { menu.appendChild(K.el(`<div class="hd">${K.esc(it.header)}</div>`)); continue; }
      const b = K.el(`<button class="mi${it.danger ? ' danger' : ''}" role="menuitem" ${it.disabled ? 'aria-disabled="true"' : ''}>
        <span class="ic">${it.icon ? K.icon(it.icon) : ''}</span><span>${K.esc(it.label)}</span>
        ${it.checked ? `<span class="check">${K.icon('check')}</span>` : ''}
        ${it.sub ? `<span class="chev">${K.icon('chevron-right')}</span>` : it.shortcut ? `<span class="sc">${K.esc(it.shortcut)}</span>` : it.hint ? `<span class="sc">${K.esc(it.hint)}</span>` : ''}
      </button>`);
      if (it.sub) {
        if (menu.classList.contains('as-sheet')) b.onclick = () => { stack.push(it); renderItems(menu, it.sub, stack); };
        else {
          const openSub = () => {
            if (menu._child && menu._child.item === it) return;
            if (menu._child) menu._child.el.remove();
            const r = b.getBoundingClientRect();
            const sub = K.el('<div class="menu" role="menu"></div>');
            sub._root = it.sub; document.body.appendChild(sub); renderItems(sub, it.sub, []); K.icons(sub);
            const w = sub.offsetWidth, h = sub.offsetHeight;
            let x = r.right + 4, y = r.top - 5;
            if (x + w > innerWidth - 8) x = r.left - w - 4;
            if (y + h > innerHeight - 8) y = Math.max(8, innerHeight - h - 8);
            sub.style.left = x + 'px'; sub.style.top = y + 'px';
            menu._child = { el: sub, item: it };
            if (openMenu) openMenu.subs.push(sub);
          };
          b.addEventListener('mouseenter', openSub); b.onclick = openSub;
        }
      } else {
        b.addEventListener('mouseenter', () => { if (menu._child) { menu._child.el.remove(); menu._child = null; } });
        b.onclick = () => { closeAll(); if (it.onSelect) it.onSelect(); else if (it.toast !== false) K.toast(it.toast || `${it.label}`); };
      }
      menu.appendChild(b);
    }
    K.icons(menu);
  }
  function closeAll() { if (openMenu) { openMenu.subs.forEach((s) => s.remove()); closeMenu(); } }
  K.menu = {
    /* target: an element (anchored below it) or {x, y}; items: [{label, icon, shortcut, hint, sub, danger, disabled, checked, onSelect, toast}, {sep:true}, {header}] */
    open(target, items, opts = {}) {
      closeAll();
      const sheet = K.isPhone() && opts.sheet !== false;
      const el = K.el(`<div class="menu${sheet ? ' as-sheet' : ''}" role="menu" tabindex="-1"></div>`);
      el._root = items;
      let scrim = null;
      if (sheet) { scrim = K.el('<div class="scrim"></div>'); scrim.onclick = closeAll; document.body.appendChild(scrim); }
      document.body.appendChild(el);
      renderItems(el, items, []);
      if (!sheet) {
        const w = el.offsetWidth, h = el.offsetHeight;
        let x, y;
        if (target instanceof Element) { const r = target.getBoundingClientRect(); x = opts.align === 'end' ? r.right - w : r.left; y = r.bottom + 4; if (y + h > innerHeight - 8) y = r.top - h - 4; }
        else { x = target.x; y = target.y; if (y + h > innerHeight - 8) y = Math.max(8, target.y - h); }
        if (x + w > innerWidth - 8) x = innerWidth - w - 8;
        el.style.left = Math.max(8, x) + 'px'; el.style.top = Math.max(8, y) + 'px';
      }
      openMenu = { el, scrim, opts, subs: [], returnFocus: document.activeElement };
      el.focus({ preventScroll: true });
      return el;
    },
    close: closeAll,
  };
  document.addEventListener('pointerdown', (e) => { if (openMenu && !e.target.closest('.menu') && !e.target.closest('.scrim')) closeAll(); }, true);
  document.addEventListener('keydown', (e) => {
    if (!openMenu) return;
    const menus = [openMenu.el, ...openMenu.subs];
    const active = menus.find((m) => m.contains(document.activeElement)) || openMenu.el;
    const items = $$('.mi:not([aria-disabled="true"])', active);
    const i = items.indexOf(document.activeElement);
    if (e.key === 'Escape') { e.preventDefault(); if (openMenu.subs.length && active !== openMenu.el) { active.remove(); openMenu.subs = openMenu.subs.filter((s) => s !== active); openMenu.el._child = null; openMenu.el.focus(); } else closeAll(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); (items[i + 1] || items[0])?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); (items[i - 1] || items[items.length - 1])?.focus(); }
    else if (e.key === 'ArrowRight' && document.activeElement?.querySelector('.chev')) { e.preventDefault(); document.activeElement.click(); setTimeout(() => $('.mi', openMenu.subs[openMenu.subs.length - 1])?.focus()); }
    else if (e.key === 'ArrowLeft' && active !== openMenu.el) { e.preventDefault(); active.remove(); openMenu.subs.pop(); openMenu.el._child = null; openMenu.el.focus(); }
  });

  /* ---------- context menus: right-click, long-press (500 ms), Shift+F10, Menu key ---------- */
  const registry = {};
  K.ctx = { register(name, fn) { registry[name] = fn; } };
  function itemsFor(el, ev) { const fn = el._ctx || registry[el.dataset.ctx]; return fn ? fn(el, ev) : null; }
  K.context = (el, fn) => { el._ctx = fn; if (!el.dataset.ctx) el.dataset.ctx = '_'; };
  document.addEventListener('contextmenu', (e) => {
    if (e.shiftKey) return; // Shift+right-click keeps the browser's menu, as in the app
    const host = e.target.closest('[data-ctx]');
    if (!host) return;
    const items = itemsFor(host, e);
    if (!items) return;
    e.preventDefault();
    K.menu.open({ x: e.clientX, y: e.clientY }, items);
  });
  document.addEventListener('keydown', (e) => {
    if ((e.key === 'F10' && e.shiftKey) || e.key === 'ContextMenu') {
      const host = document.activeElement?.closest('[data-ctx]');
      if (!host) return;
      e.preventDefault();
      const items = itemsFor(host, e); if (!items) return;
      const r = host.getBoundingClientRect(); K.menu.open({ x: r.left + 12, y: r.bottom - 4 }, items);
    }
  });
  let lp = null;
  document.addEventListener('pointerdown', (e) => {
    if (e.pointerType === 'mouse') return;
    const host = e.target.closest('[data-ctx]'); if (!host) return;
    const x = e.clientX, y = e.clientY;
    host.classList.add('press-hold');
    lp = { host, x, y, t: setTimeout(() => { host.classList.remove('press-hold'); const items = itemsFor(host, e); if (items) { navigator.vibrate && navigator.vibrate(12); K.menu.open({ x, y }, items); } lp = null; }, 500) };
  });
  const cancelLp = () => { if (lp) { clearTimeout(lp.t); lp.host.classList.remove('press-hold'); lp = null; } };
  document.addEventListener('pointermove', (e) => { if (lp && Math.hypot(e.clientX - lp.x, e.clientY - lp.y) > 8) cancelLp(); });
  document.addEventListener('pointerup', cancelLp); document.addEventListener('pointercancel', cancelLp);

  /* ---------- sheets (dialog on desktop, bottom sheet on phone) ---------- */
  K.sheet = {
    open({ title, icon, body, actions, wide, onOpen }) {
      const scrim = K.el('<div class="scrim"></div>');
      const el = K.el(`<div class="sheet${wide ? ' wide' : ''}" role="dialog" aria-modal="true" aria-label="${K.esc(title)}">
        <header>${icon ? K.icon(icon) : ''}<h3>${K.esc(title)}</h3><button class="icon-btn sm" data-close aria-label="Close">${K.icon('x')}</button></header>
        <div class="body"></div>${actions && actions.length ? '<footer></footer>' : ''}</div>`);
      const b = $('.body', el); if (typeof body === 'string') b.innerHTML = body; else if (body) b.appendChild(body);
      const ret = document.activeElement;
      const close = () => { scrim.classList.add('closing'); el.classList.add('closing'); setTimeout(() => { scrim.remove(); el.remove(); }, 120); document.removeEventListener('keydown', onKey); ret && ret.focus && ret.focus({ preventScroll: true }); };
      const onKey = (e) => { if (e.key === 'Escape' && !openMenu) close(); if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') { const p = $('footer .primary', el); p && p.click(); } };
      (actions || []).forEach((a) => { const btn = K.el(`<button class="btn ${a.kind || ''}">${a.icon ? K.icon(a.icon) : ''}${K.esc(a.label)}</button>`); btn.onclick = () => { if (a.onClick && a.onClick({ el, close }) === false) return; close(); if (a.toast) K.toast(a.toast); }; $('footer', el).appendChild(btn); });
      scrim.onclick = close; $('[data-close]', el).onclick = close;
      document.body.append(scrim, el); document.addEventListener('keydown', onKey);
      K.icons(el); K.enhance(el);
      setTimeout(() => ($('input,textarea,[autofocus]', el) || $('footer .primary', el) || el).focus({ preventScroll: true }), 30);
      onOpen && onOpen({ el, close });
      return { el, close };
    },
  };

  /* ---------- toasts ---------- */
  let toasts;
  K.toast = function (msg, { action, ms = 3200 } = {}) {
    if (!toasts) { toasts = K.el('<div class="toasts" role="status" aria-live="polite"></div>'); document.body.appendChild(toasts); }
    const t = K.el(`<div class="toast"><span>${K.esc(msg)}</span>${action ? `<button>${K.esc(action.label)}</button>` : ''}</div>`);
    const kill = () => { t.classList.add('closing'); setTimeout(() => t.remove(), 120); };
    if (action) $('button', t).onclick = () => { action.onClick && action.onClick(); kill(); };
    toasts.appendChild(t); setTimeout(kill, ms);
  };
  K.undo = (msg, onUndo) => K.toast(msg, { action: { label: 'Undo', onClick: () => { onUndo && onUndo(); K.toast('Restored'); } }, ms: 5000 });

  /* ---------- tooltips: data-tip="Text" data-keys="Ctrl+K" ---------- */
  let tipEl = null, tipTimer = null, lastTipClose = 0, tipHide = null;
  function hideTip() { clearTimeout(tipTimer); clearTimeout(tipHide); if (tipEl) { tipEl.remove(); tipEl = null; lastTipClose = Date.now(); } }
  function showTip(host) {
    hideTip();
    tipEl = K.el(`<div class="tip" role="tooltip">${K.esc(host.dataset.tip)}${host.dataset.keys ? K.kbd(host.dataset.keys) : ''}</div>`);
    document.body.appendChild(tipEl);
    const r = host.getBoundingClientRect(), w = tipEl.offsetWidth, h = tipEl.offsetHeight;
    let x = r.left + r.width / 2 - w / 2, y = host.dataset.tipPos === 'top' ? r.top - h - 6 : r.bottom + 6;
    if (host.dataset.tipPos === 'right') { x = r.right + 6; y = r.top + r.height / 2 - h / 2; }
    tipEl.style.left = Math.max(6, Math.min(innerWidth - w - 6, x)) + 'px'; tipEl.style.top = Math.max(6, y) + 'px';
    tipHide = setTimeout(hideTip, 5000);
  }
  document.addEventListener('pointerover', (e) => { const h = e.target.closest('[data-tip]'); if (!h || e.pointerType !== 'mouse') return; clearTimeout(tipTimer); tipTimer = setTimeout(() => showTip(h), Date.now() - lastTipClose < 400 ? 100 : 500); });
  document.addEventListener('pointerout', (e) => { if (e.target.closest('[data-tip]')) hideTip(); });
  document.addEventListener('focusin', (e) => { const h = e.target.closest?.('[data-tip]'); if (h && h.matches(':focus-visible')) showTip(h); });
  document.addEventListener('focusout', hideTip); document.addEventListener('keydown', hideTip, true); document.addEventListener('pointerdown', hideTip, true);

  /* ---------- declarative controls ---------- */
  K.enhance = function (root = document) {
    $$('.toggle:not([data-ok])', root).forEach((t) => {
      t.dataset.ok = 1; t.setAttribute('role', 'switch'); t.tabIndex = 0; if (!t.hasAttribute('aria-checked')) t.setAttribute('aria-checked', 'false');
      const flip = () => { t.setAttribute('aria-checked', t.getAttribute('aria-checked') === 'true' ? 'false' : 'true'); t.dispatchEvent(new Event('change', { bubbles: true })); if (t.dataset.saved !== 'off') savedTick(t); };
      t.onclick = flip; t.onkeydown = (e) => { if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); flip(); } };
    });
    $$('.seg:not([data-ok])', root).forEach((s) => {
      s.dataset.ok = 1; const thumb = K.el('<span class="thumb"></span>'); s.prepend(thumb);
      const place = () => { const on = $('button[aria-pressed="true"]', s) || $('button', s); if (!on) return; thumb.style.width = '100px'; thumb.style.transform = `translateX(${on.offsetLeft}px) scaleX(${on.offsetWidth / 100})`; };
      $$('button', s).forEach((b) => (b.onclick = () => { $$('button', s).forEach((x) => x.setAttribute('aria-pressed', x === b)); place(); s.dispatchEvent(new CustomEvent('change', { detail: b.dataset.value, bubbles: true })); }));
      requestAnimationFrame(place); new ResizeObserver(place).observe(s);
    });
    $$('.tabs:not([data-ok])', root).forEach((tabs) => {
      tabs.dataset.ok = 1; const scope = tabs.closest('[data-tabs-scope]') || tabs.parentElement;
      const pick = (b) => { $$('button[data-tab]', tabs).forEach((x) => x.setAttribute('aria-selected', x === b)); $$('[data-panel]', scope).forEach((p) => (p.hidden = p.dataset.panel !== b.dataset.tab)); tabs.dispatchEvent(new CustomEvent('tabchange', { detail: b.dataset.tab, bubbles: true })); };
      $$('button[data-tab]', tabs).forEach((b) => (b.onclick = () => pick(b)));
      const cur = $('button[aria-selected="true"]', tabs) || $('button[data-tab]', tabs); cur && pick(cur);
    });
    $$('[data-keynav]:not([data-ok])', root).forEach((list) => {
      list.dataset.ok = 1;
      list.addEventListener('keydown', (e) => {
        const rows = $$('[data-row]', list).filter((r) => r.offsetParent); const i = rows.indexOf(document.activeElement.closest('[data-row]'));
        if (e.key === 'ArrowDown' || e.key === 'j') { e.preventDefault(); (rows[i + 1] || rows[0])?.focus(); }
        if (e.key === 'ArrowUp' || e.key === 'k') { e.preventDefault(); (rows[i - 1] || rows[rows.length - 1])?.focus(); }
        if (e.key === 'Enter' && rows[i]) rows[i].click();
      });
    });
    K.icons(root);
  };
  function savedTick(el) {
    const t = K.el('<span class="mono" style="font-size:12px;color:var(--fg-3);margin-left:8px;animation:fade-in 200ms">Saved</span>');
    el.after(t); setTimeout(() => { t.style.transition = 'opacity 120ms'; t.style.opacity = 0; setTimeout(() => t.remove(), 120); }, 1200);
  }
  K.savedTick = savedTick;

  /* ---------- design notes for the viewer ---------- */
  K.notes = function () { const n = $('template#notes'); return n ? n.innerHTML : ''; };
  window.addEventListener('message', (e) => { if (K.fromParent(e) && e.data.type === 'notes?') e.source.postMessage({ type: 'notes', html: K.notes(), title: document.title }, '*'); });

  document.addEventListener('DOMContentLoaded', () => { K.enhance(document); });
})();
