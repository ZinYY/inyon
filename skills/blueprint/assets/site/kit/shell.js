/* The app shell for mockups: desktop (sidebar + header + main + optional right panel) and phone
   (top bar + main + bottom nav), built from window.BRAND and window.DATA. Screens call K.shell.mount({...}).
   opts: { active: itemId, nav: navId|false, title, crumb: html, actions: html, main: html, panel: html|null,
           phoneTitle, phoneMain: html (defaults to main), phoneActions: html, noHeader, onMount(root) }
   Restyle or rebuild this file for your product; screens only depend on K.shell.mount. */
(function () {
  const esc = (s) => K.esc(s);
  const ic = (n) => K.icon(n);
  const go = (href) => `data-act="go" data-href="${esc(href)}"`;

  function row(it, active) {
    return `<div class="srow${it.id === active ? ' active' : ''}${it.status === 'off' ? ' dim' : ''}" data-row="${it.id}" data-id="${it.id}" data-ctx="item" tabindex="0" role="button" aria-label="${esc(it.title)}, ${esc(DATA.statusLabel[it.status] || '')}">
      <span class="dot ${it.status}" aria-hidden="true"></span>
      <span class="t">${esc(it.title)}</span>
      <span class="time">${esc(it.meta || '')}</span>
      <span class="strip"><button class="icon-btn sm" data-act="archive" data-tip="Archive" aria-label="Archive">${ic('archive')}</button><button class="icon-btn sm" data-act="imenu" aria-label="More">${ic('ellipsis')}</button></span>
    </div>`;
  }
  function sidebar(o) {
    const unread = DATA.notifications.filter((n) => n.unread).length;
    return `<aside class="sidebar" aria-label="Navigation">
      <div class="top">${K.logo(20)}<b style="margin-left:8px;font-size:14px;letter-spacing:-.01em">${esc(BRAND.name)}</b>
        <span style="flex:1"></span>
        <button class="icon-btn" data-act="inbox" data-tip="Notifications" aria-label="Notifications, ${unread} unread" style="position:relative">${ic('bell')}${unread ? `<span style="position:absolute;top:0;right:0;min-width:17px;height:17px;border-radius:9px;background:var(--fg);color:var(--accent-fg);font:600 12px/17px var(--mono)">${unread}</span>` : ''}</button>
        <button class="icon-btn" data-act="more" aria-label="More options">${ic('ellipsis')}</button>
      </div>
      <div class="scroll" style="flex:1;padding:0 6px 8px" data-keynav>
        ${DATA.nav.filter((n) => n.id !== 'settings').map((n) => `<button class="row" ${go(n.href)} aria-current="${o.nav === n.id}" style="width:100%;font-size:13.5px${o.nav === n.id ? ';background:var(--accent-soft)' : ''}">${ic(n.icon)} ${esc(n.label)}</button>`).join('')}
        ${DATA.groups.map((g) => `<div class="sec" style="margin-top:10px"><button class="sec-h" data-ctx="group" data-gid="${g.id}" aria-expanded="true" style="width:100%">${ic('chevron-down')}${esc(g.label)} <span>${g.items.length}</span></button>
          <div class="sec-b">${g.items.map((it) => row(it, o.active)).join('')}</div></div>`).join('')}
        <button class="row" data-act="new" style="width:100%;margin-top:6px;color:var(--fg-2);font-size:13px">${ic('plus')} New list</button>
      </div>
      <div class="bottom">
        <button class="icon-btn" data-act="search" data-tip="Search" data-keys="Ctrl+K" aria-label="Search">${ic('search')}</button>
        <span style="flex:1"></span>
        <button class="icon-btn" ${go('settings.html')} data-tip="Settings" data-keys="Ctrl+," aria-label="Settings">${ic('settings')}</button>
      </div>
    </aside>`;
  }
  function header(o) {
    const it = DATA.byId(o.active);
    return `<div class="header">
      <div class="crumb">${o.crumb || (it ? `${esc(DATA.group(it.group).label)} <span aria-hidden="true">/</span> <b>${esc(it.title)}</b>` : `<b>${esc(o.title || '')}</b>`)}</div>
      <span style="flex:1"></span>
      ${o.actions || ''}
      <button class="icon-btn ${o.panel ? 'on' : ''}" data-act="panel" data-tip="Side panel" aria-label="Side panel">${ic('panel-right')}</button>
    </div>`;
  }
  function phoneNav(cur) {
    return `<nav class="phone-nav">${DATA.nav.slice(0, 5).map((n) => `<button ${go(n.href)} aria-current="${cur === n.id}">${ic(n.icon)}<span>${esc(n.label)}</span></button>`).join('')}</nav>`;
  }

  K.shell = {
    row, sidebar, header,
    mount(o) {
      const root = document.getElementById('app');
      if (K.isPhone() && o.phone !== false) {
        root.innerHTML = `<div class="phone-app"><div class="phone-top"><h1>${esc(o.phoneTitle || o.title || BRAND.name)}</h1>${o.phoneActions || ''}</div><div class="scroll" style="flex:1;min-height:0;display:flex;flex-direction:column">${o.phoneMain ?? o.main}</div>${o.nav === false ? '' : phoneNav(o.nav)}</div>`;
      } else {
        root.innerHTML = `<div class="app${o.panel ? ' with-panel' : ''}">${sidebar(o)}<main class="main">${o.noHeader ? '' : header(o)}<div style="flex:1;min-height:0;display:flex;flex-direction:column">${o.main}</div></main>${o.panel ? `<aside class="vpanel">${o.panel}</aside>` : ''}</div>`;
      }
      wire(root);
      K.enhance(root);
      o.onMount && o.onMount(root);
    },
  };

  function wire(root) {
    root.addEventListener('click', (e) => {
      const act = e.target.closest('[data-act]');
      const srow = e.target.closest('.srow');
      if (act) {
        const a = act.dataset.act;
        e.stopPropagation();
        if (a === 'go') { const h = act.dataset.href; if (location.pathname.endsWith('/' + h)) return K.toast('You are already here'); location.href = h + location.search; return; }
        if (a === 'more') return K.menu.open(act, MENUS.more());
        if (a === 'inbox') return K.menu.open(act, [{ header: 'Notifications' }, ...DATA.notifications.map((n) => ({ label: n.text, hint: n.ago, icon: n.unread ? 'circle-dot' : 'circle' })), { sep: true }, { label: 'Mark all read', icon: 'check-check' }], { align: 'end' });
        if (a === 'new') return K.sheet.open({ title: 'New list', icon: 'plus', body: '<label class="label" for="nl">Name</label><input id="nl" class="input" placeholder="Weekend shop" style="margin-top:6px">', actions: [{ label: 'Cancel', kind: 'ghost' }, { label: 'Create', kind: 'primary', toast: 'List created' }] });
        if (a === 'search') return K.toast('Opens search (Ctrl+K)');
        if (a === 'panel') { const app = act.closest('.app'); if (app) { app.classList.toggle('with-panel'); act.classList.toggle('on'); if (!app.querySelector('.vpanel')) K.toast('Opens the side panel'); } return; }
        if (srow) {
          const it = DATA.byId(srow.dataset.id);
          if (a === 'imenu') return K.menu.open(act, MENUS.item(it), { align: 'end' });
          if (a === 'archive') return K.undo(`Archived “${it.title}”`);
        }
        return;
      }
      const secH = e.target.closest('.sec-h');
      if (secH && secH.nextElementSibling?.classList.contains('sec-b')) { const b = secH.nextElementSibling; b.hidden = !b.hidden; secH.setAttribute('aria-expanded', !b.hidden); secH.querySelector('svg')?.remove(); secH.insertAdjacentHTML('afterbegin', K.icon(b.hidden ? 'chevron-right' : 'chevron-down')); K.icons(secH); return; }
      if (srow) {
        if (e.ctrlKey || e.metaKey) { srow.classList.toggle('selected'); return; }
        K.$$('.srow.selected', root).forEach((r) => r.classList.remove('selected'));
        const it = DATA.byId(srow.dataset.id);
        if (it.href && !location.pathname.endsWith('/' + it.href)) { location.href = it.href + location.search; return; }
        K.$$('.srow.active', root).forEach((r) => r.classList.remove('active'));
        srow.classList.add('active');
        root.dispatchEvent(new CustomEvent('item:open', { detail: srow.dataset.id, bubbles: true }));
      }
    });
  }

  K.ctx.register('item', (el) => {
    const sel = K.$$('.srow.selected');
    if (sel.length > 1 && el.classList.contains('selected')) return MENUS.bulk(sel.length);
    return MENUS.item(DATA.byId(el.dataset.id));
  });
  K.ctx.register('group', (el) => MENUS.group(DATA.group(el.dataset.gid)));
})();
