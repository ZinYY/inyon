// bun scripts/readme/frames.ts in.webp out.png [ms…]: a contact sheet of an animated WebP at the
// given times, to check a recording without playing it.
import sharp from 'sharp'

// --crop x,y,w,h cuts each frame to a region first
const args = process.argv.slice(2)
const cropArg = args.indexOf('--crop')
const crop = cropArg >= 0 ? args.splice(cropArg, 2)[1]!.split(',').map(Number) : undefined
const [input, output, ...times] = args
if (!input || !output) throw new Error('usage: frames.ts in.webp out.png [ms…]')
const meta = await sharp(input, { animated: true }).metadata()
const delays = meta.delay ?? []
const pages = meta.pages ?? 1
const total = delays.reduce((a, b) => a + b, 0)
const at = times.length ? times.map(Number) : [0, 0.25, 0.5, 0.75, 0.98].map((f) => Math.floor(f * total))
const pageAt = (ms: number) => {
  let t = 0
  for (let i = 0; i < pages; i++) {
    t += delays[i] ?? 0
    if (t > ms) return i
  }
  return pages - 1
}
const w = crop ? crop[2]! : meta.width!
const h = crop ? crop[3]! : (meta.pageHeight ?? meta.height!)
const cols = Math.min(at.length, 3)
const rows = Math.ceil(at.length / cols)
const frame = (ms: number) => {
  const img = sharp(input, { page: pageAt(ms) })
  return crop ? img.extract({ left: crop[0]!, top: crop[1]!, width: crop[2]!, height: crop[3]! }) : img
}
const tiles = await Promise.all(at.map(async (ms) => ({ input: await frame(ms).png().toBuffer(), ms })))
await sharp({ create: { width: cols * w, height: rows * h, channels: 3, background: '#888' } })
  .composite(tiles.map((t, i) => ({ input: t.input, left: (i % cols) * w, top: Math.floor(i / cols) * h })))
  .png()
  .toFile(output)
console.log(`${pages} frames, ${total} ms; sampled at ${at.join(', ')} ms -> ${output}`)
