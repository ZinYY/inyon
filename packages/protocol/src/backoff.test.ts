import { expect, test } from 'bun:test'
import { attemptAfter, backoff, STABLE_MS } from './backoff.ts'

test('reconnects back off exponentially up to the cap, with full jitter', () => {
  // the top of each step, and nothing past the cap however many tries
  expect([0, 1, 2, 3, 4, 5, 40].map((a) => backoff(a, 1000, 30_000, () => 0.999999))).toEqual([999, 1999, 3999, 7999, 15999, 29999, 29999])
  // anywhere from nothing up to that
  expect(backoff(3, 1000, 30_000, () => 0)).toBe(0)
  expect(backoff(3, 1000, 30_000, () => 0.5)).toBe(4000)
  for (let i = 0; i < 1000; i++) {
    const d = backoff(4, 1000, 30_000)
    expect(d).toBeGreaterThanOrEqual(0)
    expect(d).toBeLessThan(16_000)
  }
})

test('the backoff starts over only after a connection stayed up a while', () => {
  expect(attemptAfter(5, undefined, 1_000_000)).toBe(5)
  // open, then dropped at once: keep backing off
  expect(attemptAfter(5, 1_000_000 - 500, 1_000_000)).toBe(5)
  expect(attemptAfter(5, 1_000_000 - STABLE_MS, 1_000_000)).toBe(0)
})
