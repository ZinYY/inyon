import { afterEach, describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { installHiddenFlag } from './hidden.ts'

function setVisibility(v: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => v })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('pausing animations off screen', () => {
  afterEach(() => {
    delete (document as { visibilityState?: unknown }).visibilityState
    document.documentElement.removeAttribute('data-hidden')
  })

  test('the page is marked hidden while it is off screen and unmarked when it comes back', () => {
    setVisibility('visible')
    installHiddenFlag()
    expect(document.documentElement.hasAttribute('data-hidden')).toBe(false)
    setVisibility('hidden')
    expect(document.documentElement.hasAttribute('data-hidden')).toBe(true)
    setVisibility('visible')
    expect(document.documentElement.hasAttribute('data-hidden')).toBe(false)
  })

  test('styles.css pauses every animation on a hidden page', () => {
    const css = readFileSync(new URL('../styles.css', import.meta.url), 'utf8')
    expect(css).toMatch(/\[data-hidden\] \*,\s*\[data-hidden\] \*::before,\s*\[data-hidden\] \*::after\s*\{\s*animation-play-state: paused !important;/)
  })
})
