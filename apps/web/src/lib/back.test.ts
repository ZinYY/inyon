import { expect, test } from 'bun:test'
import { renderHook } from '@testing-library/react'
import { backHandled, useBackCloses } from './back.ts'

test('back closes the newest open layer first, then lets the page go back', () => {
  const closed: string[] = []
  const sheet = renderHook(({ open }) => useBackCloses(open, () => closed.push('sheet')), { initialProps: { open: true } })
  const picker = renderHook(({ open }) => useBackCloses(open, () => closed.push('picker')), { initialProps: { open: true } })
  expect(backHandled()).toBe(true)
  expect(closed).toEqual(['picker'])
  picker.rerender({ open: false })
  expect(backHandled()).toBe(true)
  expect(closed).toEqual(['picker', 'sheet'])
  sheet.rerender({ open: false })
  expect(backHandled()).toBe(false)
})
