// node shoot.mjs <url> <out.png> <w> <h> <light|dark> [step ...]
// Steps run in order before the screenshot:
//   click:<css>   rclick:<css>   hover:<css>   press:<key>   wait:<ms>   eval:<js>
// FULL=1 takes the whole page instead of the viewport. Lazy images are loaded first either way.
// Prints console errors and, with AUDIT=1, buttons that did nothing when clicked.
import { launch, loadImages } from './lib.mjs';
const [url, out, w, h, theme = 'light', ...steps] = process.argv.slice(2);
if (!url || !out || !w || !h) { console.error('usage: node shoot.mjs <url> <out.png> <w> <h> <light|dark> [click:<css> ...]'); process.exit(2); }
const withTheme = url + (url.includes('?') ? '&' : '?') + 'theme=' + theme;
const b = await launch();
const phone = +w < 640;
const p = await b.newPage({ viewport: { width: +w, height: +h }, deviceScaleFactor: 2, colorScheme: theme, hasTouch: phone, isMobile: phone });
const errs = [];
p.on('console', (m) => { if (m.type() === 'error') errs.push(m.text()); });
p.on('pageerror', (e) => errs.push('PAGEERROR ' + e.message));
await p.goto(withTheme, { waitUntil: 'networkidle' });
await p.waitForTimeout(400);
for (const s of steps) {
  const i = s.indexOf(':'); const k = s.slice(0, i), v = s.slice(i + 1);
  if (k === 'click') await p.click(v);
  else if (k === 'rclick') await p.click(v, { button: 'right' });
  else if (k === 'hover') await p.hover(v);
  else if (k === 'press') await p.keyboard.press(v);
  else if (k === 'wait') await p.waitForTimeout(+v);
  else if (k === 'eval') await p.evaluate(v);
  if (k !== 'wait') await p.waitForTimeout(450);
}
await loadImages(p);
await p.screenshot({ path: out, fullPage: !!process.env.FULL });
if (process.env.AUDIT) {
  // Reload before each click so a redraw can't hide the next button. A button that changes nothing
  // (no DOM change, no toast, no menu, no navigation) is dead.
  const SEL = 'button:not([disabled]), [role=button], a[href="#"]';
  const count = await p.evaluate((sel) => [...document.querySelectorAll(sel)].filter((x) => x.offsetParent).length, SEL);
  const dead = [];
  for (let i = 0; i < Math.min(count, 160); i++) {
    for (let t = 0; t < 3; t++) { try { await p.goto(withTheme, { waitUntil: 'load' }); break; } catch { await p.waitForTimeout(300); } }
    await p.waitForTimeout(150);
    const r = await p.evaluate(async ([sel, i]) => {
      const btn = [...document.querySelectorAll(sel)].filter((x) => x.offsetParent)[i];
      if (!btn) return null;
      let changed = false;
      const mo = new MutationObserver(() => (changed = true));
      mo.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
      const href = location.href; let nav = false; addEventListener('beforeunload', () => (nav = true));
      try { btn.click(); } catch {}
      await new Promise((r) => setTimeout(r, 60));
      mo.disconnect();
      if (changed || nav || location.href !== href) return null;
      return (btn.getAttribute('aria-label') || btn.dataset.tip || btn.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 50) || btn.outerHTML.slice(0, 80);
    }, [SEL, i]).catch(() => null);
    if (r) dead.push(r);
  }
  console.log(`audited ${Math.min(count, 160)} of ${count} buttons`);
  console.log(dead.length ? 'DEAD CONTROLS:\n  ' + dead.join('\n  ') : 'no dead controls');
}
console.log(errs.length ? 'ERRORS:\n' + errs.join('\n') : 'no console errors');
await b.close();
