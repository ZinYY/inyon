import { expect, test } from 'bun:test'
import { cleanDays } from './Machines.tsx'

const usage = { bytes: 0, files: 4, expired: 2, dir: '/home/me/.inyon/uploads' }

test('the clean button cleans the age the hub counted expired files with', () => {
  expect(cleanDays({ ...usage, days: 1 })).toBe(1)
  expect(cleanDays({ ...usage, days: 30 })).toBe(30)
})

test('a hub that keeps uploads forever, or predates the field, gets the old 7 days', () => {
  expect(cleanDays({ ...usage, days: 0 })).toBe(7)
  expect(cleanDays(usage)).toBe(7)
})

// the machine page stays mounted when you go to another machine; without a key the card kept the
// last machine's numbers, and Clean used its days on the new one
test('the uploads card starts fresh for each machine', async () => {
  const src = await Bun.file(new URL('./Machines.tsx', import.meta.url)).text()
  for (const use of src.match(/<UploadsCard\b[^>]*>/g) ?? []) expect(use).toContain('key={host.config.id}')
  expect(src).toMatch(/<UploadsCard\b/)
})
