import { afterEach, expect, test } from 'bun:test'
import { appHref, routeOf } from './base.ts'
import { forgetPairFragment, pairFragment, takePairFragment } from './native.ts'

test('on a hub, routes are the addresses', () => {
  expect(appHref('/machines', '/')).toBe('/machines')
  expect(routeOf('/m/box/p/3', '/')).toBe('/m/box/p/3')
})

test('on GitHub Pages, addresses carry the repository path and routes do not', () => {
  const base = '/inyon-releases/'
  expect(appHref('/machines', base)).toBe('/inyon-releases/machines')
  expect(appHref('/', base)).toBe('/inyon-releases/')
  expect(routeOf('/inyon-releases/m/box/p/3', base)).toBe('/m/box/p/3')
  expect(routeOf('/inyon-releases/', base)).toBe('/')
  expect(routeOf('/inyon-releases', base)).toBe('/')
  expect(routeOf('/inyon-releases-old/x', base)).toBe('/inyon-releases-old/x')
})

afterEach(() => {
  forgetPairFragment()
  history.replaceState(null, '', '/')
})

test('a pairing code in the fragment is taken out of the address and handed to the pairing sheet', () => {
  history.replaceState(null, '', '/#pair?m=abc&s=secret&n=studio')
  takePairFragment()
  expect(pairFragment()).toBe('inyon://pair?m=abc&s=secret&n=studio')
  // nothing of it stays where a reload or the history could send it on
  expect(location.hash).toBe('')
  expect(location.pathname).toBe('/machines')
})

test('any other fragment is left alone', () => {
  history.replaceState(null, '', '/settings#notifications')
  takePairFragment()
  expect(pairFragment()).toBeUndefined()
  expect(location.hash).toBe('#notifications')
})
