// Opens the Inyon app's pages (apps/cli/assets/app) in Chromium with tests/app/mock.js standing in
// for the app, clicks through each screen at a desktop and a narrow size in light and dark, and
// saves a picture of each. Fails on a console error, a page error or a dead click.
//
//   bun apps/cli/tests/app/check.mjs <out-dir>
//
// Playwright comes from the repo's node_modules; CHROME picks a browser binary.
import { createRequire } from 'node:module';
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const assets = join(here, '../../assets/app');
const require = createRequire(join(here, '../../../../package.json'));
const { chromium } = require('playwright');
const out = process.argv[2] || join(here, 'shots');
mkdirSync(out, { recursive: true });

const mock = readFileSync(join(here, 'mock.js'), 'utf8');
const types = { html: 'text/html', css: 'text/css', js: 'text/javascript', woff2: 'font/woff2' };
const colours = ['#c9c6bd', '#9aa5b1', '#b7a99a', '#a3b5a6', '#c2b0b8', '#aab7c4', '#bfb69d'];

const browser = await chromium.launch({ executablePath: process.env.CHROME || undefined, args: ['--disable-gpu'] });
const problems = [];

async function open(page, query) {
  await page.goto(`http://inyon.localhost/index.html?${query}`);
  // the page's content, not only its frame: the first load of a browser can be slow
  await page.waitForSelector('.page > *');
  await page.waitForTimeout(250);
}

for (const theme of ['light', 'dark']) {
  for (const [w, hgt] of [[1200, 780], [720, 640]]) {
    const ctx = await browser.newContext({ viewport: { width: w, height: hgt }, colorScheme: theme, reducedMotion: 'reduce' });
    await ctx.addInitScript(mock);
    await ctx.route('http://inyon.localhost/**', (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === '/temp-file') {
        const n = Number((url.searchParams.get('path') || '').replace(/\D/g, '')) || 0;
        return route.fulfill({ contentType: 'image/svg+xml', body: `<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60"><rect width="80" height="60" fill="${colours[n % colours.length]}"/><circle cx="58" cy="18" r="8" fill="#fff" opacity=".6"/></svg>` });
      }
      const file = url.pathname.slice(1) || 'index.html';
      try {
        return route.fulfill({ body: readFileSync(join(assets, file)), contentType: types[file.split('.').pop()] || 'application/octet-stream' });
      } catch {
        return route.fulfill({ status: 404, body: '' });
      }
    });
    const page = await ctx.newPage();
    page.on('console', (m) => m.type() === 'error' && problems.push(`${theme} ${w}: console: ${m.text()}`));
    page.on('pageerror', (e) => problems.push(`${theme} ${w}: ${e.message}`));
    const shot = async (name) => page.screenshot({ path: join(out, `${name}-${theme}-${w}.png`) });
    const s = theme === 'light' ? 'light' : 'dark';

    // both faces come from the app, never a fallback
    await open(page, `page=home&theme=${s}`);
    const faces = await page.evaluate(async () => { await document.fonts.ready; return [...document.fonts].filter((f) => f.status === 'loaded').map((f) => f.family.replace(/['"]/g, '')); });
    for (const f of ['Geist', 'Geist Mono']) if (!faces.includes(f)) problems.push(`${theme} ${w}: ${f} didn't load (${faces.join(', ') || 'none'})`);
    for (const p of ['home', 'phones', 'hub', 'pages', 'temp', 'activity', 'settings']) {
      await open(page, `page=${p}&theme=${s}`);
      await page.waitForTimeout(p === 'temp' || p === 'activity' ? 500 : 50);
      await shot(p);
    }
    await open(page, `s=fresh&page=home&theme=${s}`);
    await shot('home-fresh');
    await open(page, `s=fresh&page=hub&theme=${s}`);
    await shot('hub-empty');
    await open(page, `s=stopped&page=home&theme=${s}`);
    await shot('home-stopped');

    if (theme === 'light' && w === 1200) {
      // the flows: each step has to change what's on screen
      await open(page, 'page=phones');
      await page.getByRole('button', { name: 'Pair a phone' }).first().click();
      await page.waitForSelector('.qr:not(.empty) svg', { timeout: 4000 });
      await shot('flow-pair-code');
      await open(page, 'page=home');
      await page.getByRole('switch', { name: 'LAN' }).click();
      await page.waitForSelector('.switch.busy');
      await shot('flow-reach-applying');
      await page.waitForFunction(() => !document.querySelector('.switch.busy'), null, { timeout: 4000 });
      if ((await page.getByRole('switch', { name: 'LAN' }).getAttribute('aria-checked')) !== 'true') problems.push('LAN stayed off after turning it on');
      // the last way in can't be turned off
      for (const n of ['Tunnel', 'VPN']) {
        await page.getByRole('switch', { name: n }).click();
        await page.waitForFunction(() => !document.querySelector('.switch.busy'), null, { timeout: 4000 });
      }
      await page.getByRole('switch', { name: 'LAN' }).click();
      await page.waitForSelector('.toast');
      if ((await page.getByRole('switch', { name: 'LAN' }).getAttribute('aria-checked')) !== 'true') problems.push('the last way in was turned off');
      await open(page, 'page=temp');
      await page.waitForSelector('.item');
      // a shift-click after the row it would start from was deleted picks just the row clicked
      const rows = page.getByRole('checkbox', { name: /^Select (?!all$)/ });
      const before = await rows.count();
      await rows.first().click();
      await page.getByRole('button', { name: /^Delete / }).click();
      await page.locator('.dialog').getByRole('button', { name: 'Delete' }).click();
      await page.waitForFunction((n) => document.querySelectorAll('.item').length < n, before);
      await page.waitForTimeout(400);
      await rows.first().click({ modifiers: ['Shift'] });
      if ((await rows.first().getAttribute('aria-checked')) !== 'true') problems.push(`${theme} ${w}: a shift-click after a delete selected nothing`);
      await rows.first().click();
      await page.getByRole('checkbox', { name: 'Select all' }).click();
      await shot('flow-temp-selected');
      await page.getByRole('button', { name: /^Delete / }).click();
      await page.waitForSelector('.dialog');
      await shot('flow-temp-confirm');
      await page.locator('.dialog').getByRole('button', { name: 'Delete' }).click();
      await page.waitForSelector('.empty');
      await open(page, 'page=temp');
      await page.locator('.item').first().click();
      await page.waitForSelector('.tile');
      await shot('flow-temp-item');
      await page.locator('.tile').first().click();
      await page.waitForSelector('.preview');
      await shot('flow-temp-preview');
      await page.keyboard.press('Escape');
      await page.locator('.tile').nth(1).click({ button: 'right' });
      await page.waitForSelector('.menu');
      await shot('flow-temp-menu');
      await page.keyboard.press('Escape');
      await open(page, 's=fresh&page=hub');
      await page.getByRole('button', { name: 'Paste the command…' }).click();
      await page.locator('.dialog textarea').fill('https://hub.example.net extra words');
      await page.locator('.dialog').getByRole('button', { name: 'Add this computer' }).click();
      await page.waitForSelector('.dialog .err-text:not(:empty)');
      await shot('flow-join-error');
      await page.locator('.dialog textarea').fill("INYON_TOKEN=abc sh -c 'x' inyon https://hub.example.net");
      await page.locator('.dialog').getByRole('button', { name: 'Add this computer' }).click();
      await page.waitForSelector('.dialog', { state: 'detached' });
      await open(page, 'page=settings');
      await page.getByRole('button', { name: 'Dark' }).click();
      await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
      await shot('flow-settings-dark');
    }
    await ctx.close();
  }
}
await browser.close();
if (problems.length) {
  console.error(problems.join('\n'));
  process.exit(1);
}
console.log(`ok: pictures in ${out}`);
