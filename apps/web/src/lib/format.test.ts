import { afterEach, describe, expect, mock, setSystemTime, test } from 'bun:test'
import { basename, bytes, clock, duration, liveWindow, modelName, money, openLink, paneLabel, resetClock, resetsIn, responseTime, shortPath, since, tailPath, timeAgo, tokens } from './format.ts'

const NOW = Date.UTC(2026, 9, 11, 12, 0, 0)
afterEach(() => setSystemTime())

describe('shortPath', () => {
  test('home folders become ~', () => {
    expect(shortPath('/home/me/code')).toBe('~/code')
    expect(shortPath('/Users/me')).toBe('~')
    expect(shortPath('/root/x')).toBe('~/x')
    expect(shortPath('/homework/x')).toBe('/homework/x')
    expect(shortPath('/rootfs/x')).toBe('/rootfs/x')
  })

  test('long paths keep their last segments, within the limit', () => {
    const p = '/home/me/projects/inyon/apps/web/src/components/chat/ChatView.tsx'
    const s = shortPath(p)
    expect(s).toBe('~/…/web/src/components/chat/ChatView.tsx')
    expect(s.length).toBeLessThanOrEqual(42)
    expect(shortPath(p, 20)).toBe('~/…/ChatView.tsx')
  })

  test('one long name is shown whole, without a "…" hiding nothing', () => {
    const long = 'a'.repeat(50)
    expect(shortPath(`~/${long}`)).toBe(`~/${long}`)
    expect(shortPath(`/${long}`)).toBe(`/${long}`)
  })

  test('nothing in, nothing out', () => {
    expect(shortPath(undefined)).toBe('')
    expect(shortPath('')).toBe('')
  })

  test('Windows paths keep their backslashes, and their home becomes ~', () => {
    expect(shortPath('C:\\Users\\abhash\\inyon-demo\\checkout-service')).toBe('~\\inyon-demo\\checkout-service')
    expect(shortPath('D:\\work\\a\\b\\c\\d\\e\\f\\checkout-service\\src', 30)).toBe('D:\\…\\e\\f\\checkout-service\\src')
    expect(shortPath('C:\\demo')).toBe('C:\\demo')
  })
})

test('tailPath: a long folder as its last two parts, with its own separator', () => {
  expect(tailPath('C:\\Users\\abhash\\inyon-demo\\checkout-service')).toBe('…\\inyon-demo\\checkout-service')
  expect(tailPath('/home/me/projects/qa/oc1/very-long-name-here')).toBe('…/oc1/very-long-name-here')
  expect(tailPath('/srv/app')).toBe('/srv/app')
})

test('responseTime: milliseconds under a second, seconds after', () => {
  expect(responseTime(38)).toBe('38 ms')
  expect(responseTime(1373)).toBe('1.4 s')
})

test('basename', () => {
  expect(basename('/a/b/c.ts')).toBe('c.ts')
  expect(basename('/a/b/')).toBe('b')
  expect(basename('/')).toBe('/')
  expect(basename(undefined)).toBe('')
})

test('duration', () => {
  expect(duration(0)).toBe('0s')
  expect(duration(59_400)).toBe('59s')
  expect(duration(59_600)).toBe('1m 0s')
  expect(duration(125_000)).toBe('2m 5s')
  expect(duration(3_600_000)).toBe('1h 0m')
  expect(duration(26 * 3_600_000 + 5 * 60_000)).toBe('26h 5m')
})

describe('timeAgo and since', () => {
  test('recent times read in minutes and hours', () => {
    setSystemTime(NOW)
    expect(timeAgo(NOW - 10_000)).toBe('just now')
    expect(timeAgo(NOW + 60_000)).toBe('just now') // a clock slightly ahead
    expect(timeAgo(new Date(NOW - 10 * 60_000).toISOString())).toBe('10m ago')
    expect(timeAgo(NOW - 3 * 3_600_000)).toBe('3h ago')
  })

  test('older times show the date', () => {
    setSystemTime(NOW)
    const s = timeAgo(NOW - 3 * 86_400_000)
    expect(s).not.toContain('ago')
    expect(s).toMatch(/\d/)
  })

  test('missing or invalid times are blank', () => {
    expect(timeAgo('')).toBe('')
    expect(timeAgo('not a date')).toBe('')
    expect(timeAgo(0)).toBe('')
    expect(since(undefined)).toBe('')
    expect(since('garbage')).toBe('')
  })

  test('since counts up from a time', () => {
    setSystemTime(NOW)
    expect(since(NOW - 90_000)).toBe('1m 30s')
    expect(since(new Date(NOW - 5_000).toISOString())).toBe('5s')
    expect(since(NOW + 5_000)).toBe('0s')
  })
})

test('clock', () => {
  expect(clock('')).toBe('')
  expect(clock(new Date(NOW).toISOString())).toMatch(/\d:\d\d/)
})

describe('modelName', () => {
  test('Claude ids read as family and version', () => {
    expect(modelName('claude-opus-4-1-20250805')).toBe('Opus 4.1')
    expect(modelName('claude-sonnet-4-20250514')).toBe('Sonnet 4')
    expect(modelName('claude-opus-5-5')).toBe('Opus 5.5')
    expect(modelName('claude-haiku-4-5')).toBe('Haiku 4.5')
  })

  test('the 1M-context suffix is the same model', () => {
    expect(modelName('claude-opus-4-6[1m]')).toBe('Opus 4.6')
    expect(modelName('claude-fable-5[1m]')).toBe('Fable 5')
  })

  test('anything else is shown as is', () => {
    expect(modelName('gpt-5-codex')).toBe('gpt-5-codex')
    expect(modelName('claude-3-5-sonnet-20241022')).toBe('claude-3-5-sonnet-20241022')
    expect(modelName(undefined)).toBe('')
  })
})

test('paneLabel', () => {
  expect(paneLabel('%12')).toBe('12')
  expect(paneLabel('12')).toBe('12')
})

test('tokens', () => {
  expect(tokens(undefined)).toBe('—')
  expect(tokens(NaN)).toBe('—')
  expect(tokens(0)).toBe('0')
  expect(tokens(999)).toBe('999')
  expect(tokens(1499)).toBe('1k')
  expect(tokens(1500)).toBe('2k')
  expect(tokens(999_400)).toBe('999k')
  // never "1000k"
  expect(tokens(999_999)).toBe('1M')
  expect(tokens(1_000_000)).toBe('1M')
  expect(tokens(1_250_000)).toBe('1.3M')
  expect(tokens(12_345_678)).toBe('12M')
})

test('bytes', () => {
  expect(bytes(512)).toBe('512 B')
  expect(bytes(2048)).toBe('2 KB')
  expect(bytes(5.5 * 1024 ** 2)).toBe('5.5 MB')
  expect(bytes(250 * 1024 ** 2)).toBe('250 MB')
  expect(bytes(3 * 1024 ** 3)).toBe('3.0 GB')
})

test('money', () => {
  expect(money(undefined)).toBe('—')
  expect(money(0)).toBe('$0.00')
  expect(money(3.456)).toBe('$3.46')
  expect(money(150.4)).toBe('$150')
})

describe('plan windows', () => {
  test('a window past its reset shows as fresh', () => {
    setSystemTime(NOW)
    const w: { pct: number; resetsAt?: number } = { pct: 80, resetsAt: NOW / 1000 - 1 }
    expect(liveWindow(w)).toEqual({ pct: 0, resetsAt: undefined, reset: true })
    expect(w.pct).toBe(80) // the original is left alone
  })

  test('a current window is returned as is', () => {
    setSystemTime(NOW)
    const w = { pct: 40, resetsAt: NOW / 1000 + 60 }
    expect(liveWindow(w)).toBe(w)
    const open = { pct: 40 }
    expect(liveWindow(open)).toBe(open)
    expect(liveWindow(undefined)).toBeUndefined()
  })

  test('resetsIn', () => {
    setSystemTime(NOW)
    const at = (s: number) => NOW / 1000 + s
    expect(resetsIn(undefined)).toBe('')
    expect(resetsIn(at(-5))).toBe('resetting')
    expect(resetsIn(at(30 * 60))).toBe('resets in 30m')
    expect(resetsIn(at(90 * 60))).toBe('resets in 1h 30m')
    expect(resetsIn(at(50 * 3600))).toBe('resets in 2d 2h')
  })

  test('resetClock shows only the time on the same day, the weekday otherwise', () => {
    setSystemTime(NOW)
    expect(resetClock(undefined)).toBe('')
    const soon = new Date(NOW + 60_000)
    expect(resetClock(soon.getTime() / 1000)).toBe(soon.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }))
    const later = new Date(NOW + 3 * 86_400_000)
    expect(resetClock(later.getTime() / 1000)).toBe(later.toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' }))
  })
})

describe('openLink', () => {
  test('opens only web links, in a new tab', () => {
    const open = mock(() => null)
    const orig = window.open
    window.open = open as any
    try {
      openLink('https://example.com')
      openLink('HTTP://example.com/a')
      openLink('javascript:alert(1)')
      openLink('file:///etc/passwd')
      openLink(undefined)
      expect(open.mock.calls as unknown[][]).toEqual([
        ['https://example.com', '_blank', 'noopener'],
        ['HTTP://example.com/a', '_blank', 'noopener'],
      ])
    } finally {
      window.open = orig
    }
  })
})
