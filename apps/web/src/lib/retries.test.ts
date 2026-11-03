import { expect, test } from 'bun:test'
import { ApiError, withRetries } from './api.ts'

test('tries again after a dropped connection or a busy server, then succeeds', async () => {
  let calls = 0
  const r = await withRetries(async () => {
    calls++
    if (calls === 1) throw new TypeError('Failed to fetch')
    if (calls === 2) throw new ApiError(502, 'link dropped')
    return 'saved'
  }, [1, 1, 1])
  expect(r).toBe('saved')
  expect(calls).toBe(3)
})

test("doesn't retry a refusal, and gives up after the last wait", async () => {
  let calls = 0
  await expect(withRetries(async () => (calls++, Promise.reject(new ApiError(413, 'too large'))), [1, 1])).rejects.toThrow('too large')
  expect(calls).toBe(1)
  calls = 0
  await expect(withRetries(async () => (calls++, Promise.reject(new ApiError(503, 'busy'))), [1, 1])).rejects.toThrow('busy')
  expect(calls).toBe(3)
})
