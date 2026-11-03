// node stills.mjs <site-dir> [id ...] [--audit]
// Renders shots/<id>-d.webp (1440x900 light), -dd (dark) and -p (390x844 light) for every screen in
// mockups/screens.js, serving the site itself. Without sharp it writes PNGs and says so; the plan page
// wants WebP, so install sharp (`bun add -d sharp`) or convert them. --audit also runs the dead-control
// check on each screen at both sizes.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { launch, serve, sharp as getSharp, loadImages } from './lib.mjs';
const args = process.argv.slice(2);
const audit = args.includes('--audit');
const [dir = '.', ...only] = args.filter((a) => a !== '--audit');
const root = resolve(dir);
const window = {}; new Function('window', readFileSync(join(root, 'mockups/screens.js'), 'utf8'))(window);
const ids = only.length ? only : window.SCREENS.flatMap((g) => g.items.map((s) => s.id));
const sharp = getSharp();
if (!sharp) console.log('sharp not found: writing PNG stills (the plan page looks for .webp)');
mkdirSync(join(root, 'shots'), { recursive: true });
const srv = await serve(root);
const b = await launch();
let bad = 0;
for (const id of ids) {
  for (const [suf, w, h, theme] of [['d', 1440, 900, 'light'], ['dd', 1440, 900, 'dark'], ['p', 390, 844, 'light']]) {
    const phone = w < 640;
    const p = await b.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: phone ? 2 : 1.5, colorScheme: theme, hasTouch: phone, isMobile: phone });
    const errs = []; p.on('pageerror', (e) => errs.push(e.message)); p.on('console', (m) => m.type() === 'error' && errs.push(m.text()));
    const url = `${srv.url}mockups/screens/${id}.html?theme=${theme}`;
    const r = await p.goto(url, { waitUntil: 'networkidle' }).catch(() => null);
    if (!r || !r.ok()) { console.log(id, suf, 'missing'); bad++; await p.close(); continue; }
    await p.waitForTimeout(500);
    await loadImages(p);
    const png = await p.screenshot();
    if (sharp) await sharp(png).webp({ quality: 82 }).toFile(join(root, 'shots', `${id}-${suf}.webp`));
    else writeFileSync(join(root, 'shots', `${id}-${suf}.png`), png);
    let dead = [];
    if (audit && suf !== 'dd') {
      const SEL = 'button:not([disabled]), [role=button], a[href="#"]';
      const n = await p.evaluate((sel) => [...document.querySelectorAll(sel)].filter((x) => x.offsetParent).length, SEL);
      for (let i = 0; i < Math.min(n, 160); i++) {
        await p.goto(url, { waitUntil: 'load' }).catch(() => {}); await p.waitForTimeout(120);
        const d = await p.evaluate(async ([sel, i]) => {
          const btn = [...document.querySelectorAll(sel)].filter((x) => x.offsetParent)[i]; if (!btn) return null;
          let changed = false; const mo = new MutationObserver(() => (changed = true));
          mo.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
          const href = location.href; let nav = false; addEventListener('beforeunload', () => (nav = true)); try { btn.click(); } catch {}
          await new Promise((r) => setTimeout(r, 60)); mo.disconnect();
          return changed || nav || location.href !== href ? null : (btn.getAttribute('aria-label') || btn.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 50) || btn.outerHTML.slice(0, 80);
        }, [SEL, i]).catch(() => null);
        if (d) dead.push(d);
      }
    }
    if (errs.length || dead.length) bad++;
    console.log(id, suf, errs.length ? 'ERR ' + errs[0] : 'ok', dead.length ? `DEAD: ${dead.join(' | ')}` : '');
    await p.close();
  }
}
await b.close(); srv.close();
process.exit(bad ? 1 : 0);
