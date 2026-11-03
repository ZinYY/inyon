import { expect, test } from 'bun:test'
import { AGENTS } from '@inyon/protocol'

// WCAG relative luminance and contrast, for the pairs a session skin actually draws
const lum = (hex: string) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!
}
const contrast = (a: string, b: string) => {
  const [x, y] = [lum(a), lum(b)]
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)
}

for (const [kind, def] of Object.entries(AGENTS)) {
  for (const theme of ['light', 'dark'] as const) {
    test(`${kind} ${theme}: muted text, links and button labels stay readable`, () => {
      const t = def.skin[theme]
      for (const ground of [t.bg, t.panel, t.raised, t.hover]) expect(contrast(t.fg3, ground)).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t.accentFg, t.accent)).toBeGreaterThanOrEqual(4.5)
      expect(contrast(t.accentText, t.bg)).toBeGreaterThanOrEqual(4.5)
    })
  }
}
