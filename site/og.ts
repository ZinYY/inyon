// Renders the link previews (Open Graph images, 1200×630 PNG) for every page into site/og, plus
// the Apple touch icon. Run it after adding a page or changing a page's title or description:
//
//   bun site/og.ts
//
// The images are committed. The build checks each one against the page it was drawn for (see
// ogKey in build.ts) and falls back to the home page's image, with a warning, when one is stale.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright";
import { cards, ogKey, OG_SIZE, type Card } from "./build.ts";

const SITE = import.meta.dir;
const ROOT = join(SITE, "..");
const OUT = join(SITE, "og");
const HOST = "inyon.tech";

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const icon = readFileSync(join(ROOT, "apps/web/public/icon.svg"), "utf8");
// the kite on its own, drawn large and faint behind a docs card's text
const glyph = icon.match(/<g id="glyph"[\s\S]*?<\/g>/)![0].replace(/#F1F0EC/g, "currentColor");
const phones = `data:image/webp;base64,${readFileSync(join(ROOT, "docs/phones-light.webp")).toString("base64")}`;

const CSS = `
* { box-sizing: border-box; }
html, body { margin: 0; }
body {
  width: ${OG_SIZE.width}px; height: ${OG_SIZE.height}px; overflow: hidden; position: relative;
  background: #e9e8e4; color: #1d1c1a; font-family: "Geist", system-ui, sans-serif;
  -webkit-font-smoothing: antialiased;
}
.dots { position: absolute; inset: 0; color: #cfcec8; }
.brand { position: absolute; left: 72px; top: 64px; display: flex; align-items: center; gap: 16px; }
.brand svg { width: 56px; height: 56px; display: block; }
.brand b { font-size: 36px; font-weight: 600; letter-spacing: -0.03em; }
.chip {
  font: 500 20px "Geist Mono", monospace; color: #4a4844; padding: 5px 16px; margin-left: 6px;
  border: 1.5px solid #d6d5cf; border-radius: 999px; background: rgba(255, 255, 255, 0.45);
}
.text { position: absolute; left: 72px; bottom: 132px; }
.eyebrow {
  font: 500 21px "Geist Mono", monospace; letter-spacing: 0.08em; text-transform: uppercase;
  color: #5f5d58; margin: 0 0 18px;
}
h1 { font-weight: 500; letter-spacing: -0.045em; line-height: 0.98; margin: 0; }
.desc {
  font-size: 28px; line-height: 1.36; color: #4a4844; margin: 20px 0 0; letter-spacing: -0.01em;
  display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden;
}
.foot {
  position: absolute; left: 72px; right: 72px; bottom: 56px; display: flex; justify-content: space-between;
  font: 400 21px "Geist Mono", monospace; color: #5f5d58;
  border-top: 1.5px solid #d6d5cf; padding-top: 22px;
}
.foot b { color: #1d1c1a; font-weight: 500; }
.kite { position: absolute; right: -30px; top: 64px; width: 330px; height: 330px; color: #dcdbd5; }
.phones {
  position: absolute; left: 640px; top: 66px; width: 600px; height: 620px;
  background: url(${phones}) no-repeat; background-size: 1116px auto; background-position: -106px -45px;
}
`;

const dots = `<svg class="dots" width="100%" height="100%" aria-hidden="true"><defs><pattern id="d" width="28" height="28" patternUnits="userSpaceOnUse"><circle cx="2" cy="2" r="1.4" fill="currentColor"/></pattern></defs><rect width="100%" height="100%" fill="url(#d)"/></svg>`;

function page(card: Card): string {
  const brand = `<div class="brand">${icon}<b>Inyon</b>${card.chip ? `<span class="chip">${esc(card.chip)}</span>` : ""}</div>`;
  const path = card.path === "/" ? "" : card.path.replace(/\/$/, "");
  if (card.id === "home") {
    return `${dots}<div class="phones"></div>${brand}
<div class="text" style="width:560px">
  <h1 style="font-size:76px">Your coding agents, from your phone.</h1>
  <p class="desc">Watch, answer and steer Claude Code, Codex and more, running in tmux on your own computers.</p>
</div>
<div class="foot" style="right:640px"><span><b>${HOST}</b></span></div>`;
  }
  // long titles step down so they stay on one line, clear of the kite
  const size = card.title.length > 24 ? 68 : card.title.length > 16 ? 80 : 92;
  return `${dots}<svg class="kite" viewBox="0 0 64 64" aria-hidden="true">${glyph}</svg>${brand}
<div class="text" style="width:820px">
  ${card.eyebrow ? `<p class="eyebrow">${esc(card.eyebrow)}</p>` : ""}
  <h1 style="font-size:${size}px">${esc(card.title)}</h1>
  <p class="desc">${esc(card.description)}</p>
</div>
<div class="foot"><span><b>${HOST}</b>${esc(path)}</span><span>Open source · MIT</span></div>`;
}

mkdirSync(OUT, { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.CHROME || undefined });
const tab = await browser.newPage({ viewport: OG_SIZE, deviceScaleFactor: 1 });
const keys: Record<string, string> = {};
for (const card of cards()) {
  await tab.setContent(
    `<!doctype html><html><head><meta charset="utf-8">
<link href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600&family=Geist+Mono:wght@400;500&display=block" rel="stylesheet">
<style>${CSS}</style></head><body>${page(card)}</body></html>`,
    { waitUntil: "networkidle" },
  );
  await tab.evaluate(() => document.fonts.ready);
  const fonts = await tab.evaluate(() => document.fonts.check('500 40px "Geist"') && document.fonts.check('400 20px "Geist Mono"'));
  if (!fonts) throw new Error("Geist didn't load (it comes from Google Fonts)");
  writeFileSync(join(OUT, `${card.id}.png`), await tab.screenshot({ type: "png" }));
  keys[card.id] = ogKey(card);
  console.log(`  og/${card.id}.png`);
}

// the icon iOS and Android show for a bookmark: the tile edge to edge, as Apple rounds it itself
await tab.setViewportSize({ width: 180, height: 180 });
await tab.setContent(
  `<!doctype html><html><body style="margin:0;background:#1d1c1a">${icon.replace("<svg ", '<svg width="180" height="180" style="display:block;transform:scale(1.16)" ')}</body></html>`,
);
writeFileSync(join(OUT, "apple-touch-icon.png"), await tab.screenshot({ type: "png" }));
await browser.close();

writeFileSync(join(OUT, "keys.json"), `${JSON.stringify(keys, null, 2)}\n`);
console.log(`drew ${Object.keys(keys).length} link previews into site/og`);
