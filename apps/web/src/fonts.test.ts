import { expect, test } from 'bun:test'
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { renderHook } from '@testing-library/react'
import { useStore } from './lib/store.ts'
import { useReplyFont } from './lib/skin.tsx'
import { loadSerifFont, serifAsked } from './lib/serif.ts'

const sheets = ['fonts.css', 'serif.css'].map((f) => [f, readFileSync(path.join(import.meta.dir, f), 'utf8')] as const)
const urlsIn = (css: string) => [...css.matchAll(/url\(([^)]+)\)/g)].map((m) => m[1]!.replace(/['"]/g, ''))

// Vite leaves a url() it can't resolve as it is, and the hub answers that path with the app's
// page, so a wrong font path builds fine and quietly falls back to the system font.
test('every font file the stylesheets name can be found', () => {
  for (const [, css] of sheets) {
    const urls = urlsIn(css)
    expect(urls.length).toBeGreaterThan(0)
    for (const url of urls) {
      const file = url.startsWith('.') ? path.resolve(import.meta.dir, url) : Bun.resolveSync(url, import.meta.dir)
      expect(existsSync(file)).toBe(true)
    }
  }
})

// the first open fetches about 50 KB of fonts: the two Geist faces, Latin only
test('the first load has only the Latin subsets of Geist and Geist Mono', () => {
  const [[, fonts], [, serif]] = sheets as [readonly [string, string], readonly [string, string]]
  expect(urlsIn(fonts)).toEqual([
    '@fontsource-variable/geist/files/geist-latin-wght-normal.woff2',
    '@fontsource-variable/geist-mono/files/geist-mono-latin-wght-normal.woff2',
  ])
  expect(urlsIn(serif)).toEqual(['@fontsource-variable/source-serif-4/files/source-serif-4-latin-wght-normal.woff2'])
  for (const [, css] of sheets) {
    expect(css).not.toContain('latin-ext')
    // a face arriving late swaps in, as it always did; nothing blocks text on it
    expect(css.match(/font-display: swap/g)?.length).toBe(urlsIn(css).length)
  }
})

test('the serif face is asked for by the first session whose replies read in serif', () => {
  expect(serifAsked()).toBe(false)
  // Codex reads in sans: nothing to fetch
  expect(renderHook(() => useReplyFont('codex')).result.current).toBe('sans')
  expect(serifAsked()).toBe(false)
  expect(renderHook(() => useReplyFont('claude')).result.current).toBe('serif')
  expect(serifAsked()).toBe(true)
  // a second ask is a no-op
  loadSerifFont()
  expect(serifAsked()).toBe(true)
})

test('the setting that puts every reply in serif reads as serif for a sans agent', () => {
  useStore.getState().setSettings({ replyFont: 'serif' })
  expect(renderHook(() => useReplyFont('codex')).result.current).toBe('serif')
})
