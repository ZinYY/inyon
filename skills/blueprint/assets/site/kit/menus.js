/* Shared menus. One list per thing, used by its right-click, long-press and ⋯ button alike, so the three
   never drift apart. Items follow the K.menu shape; an item without onSelect shows a toast naming it. */
(function () {
  window.MENUS = {
    item(it) {
      return [
        { label: 'Open', icon: 'arrow-up-right', shortcut: 'Enter' },
        { label: 'Rename…', icon: 'pencil', shortcut: 'F2', onSelect: () => K.sheet.open({ title: 'Rename list', icon: 'pencil', body: `<input class="input" value="${K.esc(it.title)}" aria-label="Name">`, actions: [{ label: 'Cancel', kind: 'ghost' }, { label: 'Save', kind: 'primary', toast: 'Renamed' }] }) },
        { label: 'Share with', icon: 'users', sub: DATA.people.map((p) => ({ label: p.name, checked: p.id === 'p2' })) },
        { label: 'Duplicate', icon: 'copy' },
        { sep: true },
        { label: it.status === 'off' ? 'Restore' : 'Archive', icon: 'archive' },
        { label: 'Delete', icon: 'trash-2', danger: true, onSelect: () => K.undo(`Deleted “${it.title}”`) },
      ];
    },
    bulk(n) {
      return [{ header: `${n} selected` }, { label: 'Archive', icon: 'archive' }, { label: 'Share with…', icon: 'users' }, { sep: true }, { label: 'Delete', icon: 'trash-2', danger: true, onSelect: () => K.undo(`Deleted ${n} lists`) }];
    },
    group(g) {
      return [{ label: 'New list here', icon: 'plus', shortcut: 'Ctrl+N' }, { label: 'Rename section…', icon: 'pencil' }, { label: 'Collapse', icon: 'chevrons-up' }, { sep: true }, { label: 'Delete section', icon: 'trash-2', danger: true, onSelect: () => K.undo(`Deleted “${g.label}”`) }];
    },
    more() {
      return [
        { label: 'Theme', icon: 'sun-moon', sub: [{ label: 'Light', onSelect: () => K.setTheme('light') }, { label: 'Dark', onSelect: () => K.setTheme('dark') }] },
        { label: 'Keyboard shortcuts', icon: 'keyboard', shortcut: 'Ctrl+/' },
        { label: 'Help', icon: 'circle-help' },
        { sep: true },
        { label: 'Sign out', icon: 'log-out' },
      ];
    },
  };
})();
