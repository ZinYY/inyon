import { expect, test } from 'bun:test'
import { copyOldKeys } from './legacy.ts'

test('keys saved under the old name are copied once, never over a newer value', () => {
  localStorage.clear()
  localStorage.setItem('tether.settings.v2', '{"theme":"dark"}')
  localStorage.setItem('tether.keepAlive', '1')
  localStorage.setItem('inyon.keepAlive', '0')
  localStorage.setItem('other', 'x')
  copyOldKeys()
  expect(localStorage.getItem('inyon.settings.v2')).toBe('{"theme":"dark"}')
  expect(localStorage.getItem('inyon.keepAlive')).toBe('0')
  expect(localStorage.getItem('inyon.other')).toBeNull()
})
