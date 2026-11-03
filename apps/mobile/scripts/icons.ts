// Renders every app icon and splash screen from apps/web/public/icon.svg (run: bun --filter @inyon/mobile icons)
import sharp from 'sharp'
import path from 'node:path'

const root = path.join(import.meta.dir, '..')
const svg = await Bun.file(path.join(root, '../web/public/icon.svg')).text()
const BG = '#1D1C1A'
const glyph = svg.match(/<g id="glyph"[\s\S]*?<\/g>/)![0]
const tile = svg.match(/<path id="tile"[^>]*\/>/)![0]

/**
 * The kite alone on a transparent square, `scale` of the tile's width, centred. The kite reaches
 * 33 of the tile's 64 units out from the middle, so 0.55 keeps it inside Android's 66dp safe
 * circle and 0.7 inside a maskable icon's 80%.
 */
const mark = (scale: number, colour?: string) => {
  const off = (64 - 64 * scale) / 2
  const g = colour ? glyph.replaceAll('#F1F0EC', colour) : glyph
  return Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><g transform="translate(${off} ${off}) scale(${scale})">${g}</g></svg>`)
}
// a full-bleed square or circle behind the kite, for places that cut their own shape
const filled = (shape: 'square' | 'circle', scale: number) =>
  Buffer.from(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${shape === 'square' ? `<rect width="64" height="64" fill="${BG}"/>` : `<circle cx="32" cy="32" r="32" fill="${BG}"/>`}<g transform="translate(${(64 - 64 * scale) / 2} ${(64 - 64 * scale) / 2}) scale(${scale})">${glyph}</g></svg>`,
  )
const png = (input: Buffer, size: number) => sharp(input, { density: 72 * Math.max(1, size / 64) }).resize(size, size).png()

const web = path.join(root, '../web/public')
// the browser's notification badge: white on transparent (shown as a mask)
await png(mark(0.86, '#FFFFFF'), 96).toFile(`${web}/badge-96.png`)
// home screen, install prompt and Apple touch icon
await png(Buffer.from(svg), 192).toFile(`${web}/icon-192.png`)
await png(Buffer.from(svg), 512).toFile(`${web}/icon-512.png`)
await png(filled('square', 0.7), 512).toFile(`${web}/icon-maskable-512.png`)
await png(filled('square', 0.78), 180).flatten({ background: BG }).toFile(`${web}/apple-touch-icon.png`)
await png(Buffer.from(svg), 32).toFile(`${web}/favicon-32.png`)

const res = path.join(root, 'android/app/src/main/res')
// the launcher icons are shared with the Dioxus build (apps/app/android), which installs over this app
const shared = path.join(root, 'android/app/src/shared/res')
for (const [dir, size] of [['mdpi', 48], ['hdpi', 72], ['xhdpi', 96], ['xxhdpi', 144], ['xxxhdpi', 192]] as const) {
  await png(Buffer.from(svg), size).toFile(`${shared}/mipmap-${dir}/ic_launcher.png`)
  await png(filled('circle', 0.78), size).toFile(`${shared}/mipmap-${dir}/ic_launcher_round.png`)
  // adaptive icons are 108dp with a 66dp safe circle in the middle
  const fg = Math.round((size * 108) / 48)
  await png(mark(0.55), fg).toFile(`${shared}/mipmap-${dir}/ic_launcher_foreground.png`)
}
await Bun.write(`${shared}/values/ic_launcher_background.xml`, `<?xml version="1.0" encoding="utf-8"?>\n<resources>\n    <color name="ic_launcher_background">${BG}</color>\n</resources>\n`)

async function splash(file: string, w: number, h: number) {
  const s = Math.round(Math.min(w, h) * 0.42)
  const icon = await png(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${tile.replace(`fill="${BG}"`, 'fill="#252422"')}${glyph}</svg>`), s).toBuffer()
  await sharp({ create: { width: w, height: h, channels: 3, background: BG } }).composite([{ input: icon, gravity: 'center' }]).png().toFile(file)
}
await splash(`${res}/drawable/splash.png`, 480, 320)
for (const [dir, w, h] of [['mdpi', 320, 480], ['hdpi', 480, 800], ['xhdpi', 720, 1280], ['xxhdpi', 960, 1600], ['xxxhdpi', 1280, 1920]] as const) {
  await splash(`${res}/drawable-port-${dir}/splash.png`, w, h)
  await splash(`${res}/drawable-land-${dir}/splash.png`, h, w)
}

const xc = path.join(root, 'ios/App/App/Assets.xcassets')
// iOS rounds the corners itself and wants no transparency
await png(filled('square', 0.78), 1024).flatten({ background: BG }).toFile(`${xc}/AppIcon.appiconset/AppIcon-512@2x.png`)
for (const f of ['splash-2732x2732.png', 'splash-2732x2732-1.png', 'splash-2732x2732-2.png']) await splash(`${xc}/Splash.imageset/${f}`, 2732, 2732)
console.log('icons and splash screens written')
