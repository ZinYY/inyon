// A stand-in for the Inyon app behind the window's pages, with made-up data, so the pages can be
// opened in a browser and clicked through by tests/app/check.mjs. Same calls and answers as
// src/desktop/mod.rs.
(() => {
  const now = Date.now();
  const params = new URLSearchParams(location.search);
  const scenario = params.get('s') || 'running';
  const hour = 3600e3;
  const S = {
    version: '0.21.0',
    host: 'studio-pc',
    platformName: 'Windows 11',
    app: { running: ['link', 'tunnel'], pid: 4120 },
    update: { state: 'current', latest: '0.21.0' },
    link: { setUp: true, running: true, autostart: true, port: 4780, addresses: ['wss://quiet-river-1234.trycloudflare.com/link', 'ws://100.71.20.9:4780/link'] },
    reach: { tunnel: true, lan: false, vpn: true, applying: null, error: null, firewall: true, tunnelUrl: 'quiet-river-1234.trycloudflare.com', lanIp: '192.168.1.24', vpns: ['NetBird 100.71.20.9'] },
    phones: [
      { key: 'a1b2c3d4e5f6', name: 'Pixel 8', paired: '12 Oct', lastSeen: '22:27' },
      { key: 'f6e5d4c3b2a1', name: 'Galaxy Tab', paired: '3 Oct', lastSeen: '18 Oct' },
    ],
    pairing: { card: 'idle' },
    joined: { url: 'https://hub.example.net', running: true, problem: null, autostart: true },
    hubs: [{ url: 'https://hub.example.net', name: 'hub.example.net', joined: true }],
    pages: [
      { slug: 'launch-plan', name: 'Launch plan', path: 'C:\\Users\\sam\\work\\plans\\launch', project: 'checkout-service', url: 'http://inyonpage.localhost/launch-plan/' },
      { slug: 'billing-mockups', name: 'Billing mockups', path: 'C:\\Users\\sam\\work\\billing-api\\mockups', project: 'billing-api', url: 'http://inyonpage.localhost/billing-mockups/' },
    ],
    settings: { theme: params.get('theme') || 'system', closeToTray: true, openAtSignIn: false, checkUpdates: true, activeHub: 'https://hub.example.net', lastPage: params.get('page') || 'home' },
    wsl: 'Ubuntu',
    notice: null,
  };
  if (scenario === 'fresh') {
    Object.assign(S, { app: { running: [], pid: 4120 }, phones: [], joined: null, hubs: [], pages: [] });
    S.link = { setUp: false, running: false, autostart: false, port: 4780, addresses: [] };
    S.reach = { ...S.reach, tunnel: true, vpn: false };
  }
  if (scenario === 'stopped') {
    S.link.running = false;
    S.link.addresses = [];
    S.app.running = [];
    S.joined.running = false;
    S.update = { state: 'available', latest: '0.21.1' };
  }
  const svg = (c) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48"><rect width="64" height="48" fill="${c}"/></svg>`)}`;
  const temp = {
    v: 1,
    host: 'studio-pc',
    roots: [
      { id: 'claude-tmp', label: 'Claude Code', path: 'C:\\Users\\sam\\AppData\\Local\\Temp\\claude', size: 1.84e9, items: 3, exists: true },
      { id: 'inyon-uploads', label: 'Inyon uploads', path: 'C:\\Users\\sam\\.inyon\\uploads', size: 24.6e6, items: 2, exists: true },
      { id: 'claude-paste', label: 'Claude paste cache', path: 'C:\\Users\\sam\\.claude\\paste-cache', size: 1.4e6, items: 1, exists: true },
    ],
    items: [
      { id: 'a', root: 'claude-tmp', path: 'T\\claude\\checkout-service\\7409eef9', name: 'checkout-service · 7409eef9', kind: 'dir', size: 1.21e9, files: 412, modified: now - 9 * 86400e3, recent: false, images: ['img-1.png', 'img-2.png', 'img-3.png'] },
      { id: 'b', root: 'claude-tmp', path: 'T\\claude\\billing-api\\88629c69', name: 'billing-api · 88629c69', kind: 'dir', size: 590e6, files: 133, modified: now - 2 * hour, recent: false, images: ['img-4.png'] },
      { id: 'c', root: 'claude-tmp', path: 'T\\claude\\billing-api\\191bfe25', name: 'billing-api · 191bfe25', kind: 'dir', size: 38e6, files: 21, modified: now - 10 * 60e3, recent: true, images: [] },
      { id: 'd', root: 'inyon-uploads', path: 'U\\2026-10-20', name: '20 Oct', kind: 'dir', size: 18e6, files: 9, modified: now - 4 * 86400e3, recent: false, images: ['img-5.png', 'img-6.png'] },
      { id: 'e', root: 'inyon-uploads', path: 'U\\2026-10-24', name: '24 Oct', kind: 'dir', size: 6.6e6, files: 3, modified: now - 3 * hour, recent: false, images: ['img-7.png'] },
      { id: 'f', root: 'claude-paste', path: 'P\\paste-cache', name: 'paste-cache', kind: 'dir', size: 1.4e6, files: 31, modified: now - 30 * 86400e3, recent: false, images: [] },
    ],
  };
  const files = Array.from({ length: 14 }, (_, i) => ({ path: `img-${(i % 7) + 1}.png`, rel: i % 3 === 2 ? `scratchpad/notes-${i}.md` : `images/paste-${i + 1}.png`, size: 180e3 + i * 41e3, modified: now - i * hour * 5, kind: i % 3 === 2 ? 'text' : 'image' }));
  window.__mockFiles = { svg };
  let pairTimer = null;
  const handlers = {
    state: () => S,
    remember: () => null,
    reach: (a) => new Promise((r) => setTimeout(() => { Object.assign(S.reach, a); r(null); }, 900)),
    setup: (a) => new Promise((r) => setTimeout(() => { Object.assign(S.reach, a); S.link = { ...S.link, setUp: true, running: true, autostart: true, addresses: ['ws://100.71.20.9:4780/link'] }; r(null); }, 900)),
    job: (a) => { if (a.role === 'link') { S.link.running = a.action !== 'stop'; } else if (S.joined) S.joined.running = a.action !== 'stop'; return `The ${a.role === 'link' ? 'phone link' : 'hub connection'} ${a.action === 'stop' ? 'stopped' : 'is running'}.`; },
    pair: () => {
      clearTimeout(pairTimer);
      S.pairing = { card: 'asking' };
      pairTimer = setTimeout(() => {
        S.pairing = { card: 'starting' };
        pairTimer = setTimeout(() => {
          S.pairing = { card: 'code', qr: '<svg viewBox="0 0 29 29" shape-rendering="crispEdges"><path fill="#1d1c1a" d="M0 0h7v7H0zM22 0h7v7h-7zM0 22h7v7H0zM9 2h2v2H9zM12 4h3v2h-3zM10 9h4v3h-4zM16 10h3v2h-3zM9 14h2v6H9zM13 16h6v2h-6zM21 13h3v3h-3zM22 19h3v4h-3zM16 22h4v3h-4zM11 24h3v3h-3z"/><path fill="#fff" d="M1 1h5v5H1zM23 1h5v5h-5zM1 23h5v5H1z"/><path fill="#1d1c1a" d="M2 2h3v3H2zM24 2h3v3h-3zM2 24h3v3H2z"/></svg>', link: 'https://inyon.tech/app/#pair?x=…', id: 'ZS5C-3EPG', code: '801 356', expiresAt: Date.now() + 10 * 60e3, lan: false };
        }, 900);
      }, 500);
      return null;
    },
    pairCancel: () => { clearTimeout(pairTimer); S.pairing = { card: 'idle' }; return null; },
    forget: (a) => { S.phones = S.phones.filter((p) => p.key !== a.key); return null; },
    join: (a) => { if (!/INYON_TOKEN=|-Token '/.test(a.text)) throw new Error('Paste the command from the hub’s Machines page (Add a machine), or the hub’s address (https://…).'); S.joined = { url: 'https://hub.example.net', running: true, problem: null }; S.hubs = [{ url: 'https://hub.example.net', name: 'hub.example.net', joined: true }]; return 'studio-pc is on hub.example.net now.'; },
    leave: () => { S.joined = null; S.hubs = S.hubs.map((x) => ({ ...x, joined: false })); return 'This computer left the hub.'; },
    hubAdd: (a) => { if (!/^https?:\/\//.test(a.url)) throw new Error(`${a.url || 'That'} isn’t a web address.`); S.hubs.push({ url: a.url, name: a.name || new URL(a.url).host, joined: false }); S.settings.activeHub = a.url; return { hubs: S.hubs, settings: S.settings }; },
    hubRemove: (a) => { S.hubs = S.hubs.filter((x) => x.url !== a.url); return { hubs: S.hubs, settings: S.settings }; },
    hubCreate: () => new Promise((r) => setTimeout(() => { S.hubs.push({ url: 'http://localhost:4777', name: 'This computer (WSL)', joined: false }); r('http://localhost:4777'); }, 1200)),
    view: () => null,
    theme: () => null,
    viewGo: () => null,
    menu: () => null,
    openUrl: () => null,
    openPath: () => null,
    pickFolder: () => 'C:\\Users\\sam\\work\\plans\\pricing',
    pageAdd: (a) => { S.pages.push({ slug: 'pricing', name: 'Pricing', path: a.path, project: 'plans', url: '' }); return '"Pricing" is in Pages.'; },
    pageRemove: (a) => { S.pages = S.pages.filter((p) => p.slug !== a.slug); return null; },
    temp: () => new Promise((r) => setTimeout(() => r(JSON.parse(JSON.stringify(temp))), 300)),
    tempFiles: () => files,
    tempPreview: (a) => ({ path: a.path, mime: 'text/markdown', size: 1200, text: '# Notes\n\n- the checkout total rounds twice\n- write a test for it first\n', truncated: false }),
    tempRemove: (a) => { const gone = temp.items.filter((i) => a.paths.includes(i.path)); temp.items = temp.items.filter((i) => !a.paths.includes(i.path)); return { removed: a.paths, freed: gone.reduce((n, i) => n + i.size, 0) || 420e3, failed: [] }; },
    checks: () => [
      { ok: true, check: 'engine', detail: 'built in (0.21.0)' },
      { ok: true, check: 'psmux', detail: '3.3.4' },
      { ok: true, check: 'agents', detail: 'claude, codex' },
      { ok: true, check: 'link', detail: 'running, answering on 2 addresses' },
      { ok: false, check: 'firewall', detail: 'Windows Firewall keeps phones on your network out; turn LAN off and on again to add the rule.' },
    ],
    log: (a) => Array.from({ length: 30 }, (_, i) => `2026-10-24 12:${String(10 + i).padStart(2, '0')} ${a.which}: ${['started the link (pid 4188)', 'tunnel: wss://quiet-river-1234.trycloudflare.com/link', 'published 2 addresses', 'paired with Pixel 8'][i % 4]}`).join('\n'),
    settings: (a) => { Object.assign(S.settings, a.patch); return S.settings; },
    autostart: (a) => { S.link.autostart = a.on; return a.on ? 'Inyon starts when you sign in.' : 'Inyon no longer starts when you sign in.'; },
    update: (a) => { S.update = a.install ? { state: 'updating', latest: S.update.latest } : { state: 'current', latest: S.version }; return null; },
    copy: () => null,
    quit: () => null,
    uninstall: () => 'Inyon is uninstalled.',
  };
  window.__mock = { S, temp };
  window.ipc = {
    postMessage(raw) {
      const { id, cmd, args } = JSON.parse(raw);
      const f = handlers[cmd];
      Promise.resolve()
        .then(() => {
          if (!f) throw new Error(`no ${cmd} in the mock`);
          return f(args || {});
        })
        .then((v) => window.__inyonReply(id, true, JSON.parse(JSON.stringify(v === undefined ? null : v))))
        .catch((e) => window.__inyonReply(id, false, e.message));
    },
  };
})();
