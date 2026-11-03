import { afterEach, expect, test } from 'bun:test'
import { keyLabel, shortcut } from './keys.ts'

afterEach(() => (document.body.innerHTML = ''))

const press = (key: string, target: Element, mods: Partial<Record<'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey', boolean>> = {}) =>
  ({ key, code: /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target, ...mods })

test('N opens a new session only while nothing takes typing; Alt+N works from the message box too', () => {
  document.body.innerHTML = '<textarea></textarea><div class="xterm"><textarea></textarea></div>'
  const [box, term] = [...document.querySelectorAll('textarea')]
  expect(shortcut(press('n', document.body), document.body)).toBe('new')
  // typing an "n" into a message is just that
  expect(shortcut(press('n', box!), box!)).toBeUndefined()
  // in a session a key with nothing focused moves into the message box before this runs
  expect(shortcut(press('n', document.body), box!)).toBeUndefined()
  // on a Mac ⌥N types a dead key ("˜"): the key's code still says N
  expect(shortcut({ ...press('n', box!, { altKey: true }), key: 'Dead' }, box!)).toBe('new')
  // in the terminal Alt keys are the shell's
  expect(shortcut(press('n', term!, { altKey: true }), term!)).toBeUndefined()
  expect(shortcut(press('n', document.body, { ctrlKey: true }), document.body)).toBeUndefined()
})

test('Ctrl K or ⌘K searches from anywhere', () => {
  document.body.innerHTML = '<textarea></textarea>'
  const box = document.querySelector('textarea')!
  expect(shortcut(press('k', box, { ctrlKey: true }), box)).toBe('search')
  expect(shortcut(press('k', box, { metaKey: true }), box)).toBe('search')
  expect(shortcut(press('k', box), box)).toBeUndefined()
})

test('shortcuts are written the way this computer names its keys', () => {
  const ua = Object.getOwnPropertyDescriptor(Navigator.prototype, 'userAgent') ?? Object.getOwnPropertyDescriptor(navigator, 'userAgent')
  const as = (s: string) => Object.defineProperty(navigator, 'userAgent', { value: s, configurable: true })
  try {
    as('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)')
    expect([keyLabel('cmd', 'K'), keyLabel('alt', 'N')]).toEqual(['⌘K', '⌥N'])
    as('Mozilla/5.0 (X11; Linux x86_64)')
    expect([keyLabel('cmd', 'K'), keyLabel('alt', 'N')]).toEqual(['Ctrl K', 'Alt N'])
  } finally {
    if (ua) Object.defineProperty(navigator, 'userAgent', ua)
    else delete (navigator as any).userAgent
  }
})
