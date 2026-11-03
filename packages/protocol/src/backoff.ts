/**
 * How long to wait before reconnecting: capped exponential backoff with full jitter, a random time
 * between 0 and min(cap, base·2^attempt). After a hub or a computer restarts, every app, phone and
 * machine would otherwise come back at the same moments.
 */
export const backoff = (attempt: number, baseMs: number, capMs: number, random: () => number = Math.random) =>
  Math.floor(random() * Math.min(capMs, baseMs * 2 ** Math.min(attempt, 30)))

/**
 * How long a connection has to stay up before its backoff starts from the first step again. One
 * that opens and drops at once (a crash loop, a proxy that cuts upgrades) keeps backing off.
 */
export const STABLE_MS = 30_000

/** The next attempt number after a connection that was up since `openedAt` closed. */
export const attemptAfter = (attempt: number, openedAt: number | undefined, now = Date.now()) => (openedAt !== undefined && now - openedAt >= STABLE_MS ? 0 : attempt)
