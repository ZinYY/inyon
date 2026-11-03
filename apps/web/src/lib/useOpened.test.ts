import { describe, expect, mock, test } from 'bun:test'
import { renderHook } from '@testing-library/react'
import { useOpened } from './useOpened.ts'

const setup = (open: boolean, key?: string) => {
  const reset: () => void = mock(() => {})
  const hook = renderHook((p: { open: boolean; key?: string; reset: () => void }) => useOpened(p.open, p.key, p.reset), { initialProps: { open, key, reset } })
  return { reset, ...hook }
}

describe('useOpened', () => {
  test('resets once on open, not on every render after', () => {
    const { reset, rerender } = setup(true, 'a')
    expect(reset).toHaveBeenCalledTimes(1)
    rerender({ open: true, key: 'a', reset })
    rerender({ open: true, key: 'a', reset: () => {} })
    expect(reset).toHaveBeenCalledTimes(1)
  })

  test('does nothing while closed', () => {
    const { reset, rerender } = setup(false, 'a')
    rerender({ open: false, key: 'b', reset })
    expect(reset).not.toHaveBeenCalled()
  })

  test('reopening the same item resets again', () => {
    const { reset, rerender } = setup(true, 'a')
    rerender({ open: false, key: 'a', reset })
    rerender({ open: true, key: 'a', reset })
    expect(reset).toHaveBeenCalledTimes(2)
  })

  test('switching item while open resets', () => {
    const { reset, rerender } = setup(true, 'a')
    rerender({ open: true, key: 'b', reset })
    expect(reset).toHaveBeenCalledTimes(2)
  })

  test('no key (adding, not editing) counts as one item', () => {
    const { reset, rerender } = setup(true)
    rerender({ open: true, key: undefined, reset })
    expect(reset).toHaveBeenCalledTimes(1)
    rerender({ open: true, key: 'a', reset })
    expect(reset).toHaveBeenCalledTimes(2)
  })

  test('calls the latest reset, so it sees current props', () => {
    const first = mock(() => {})
    const latest = mock(() => {})
    const { rerender } = renderHook((p: { open: boolean; reset: () => void }) => useOpened(p.open, 'a', p.reset), { initialProps: { open: false, reset: first } })
    rerender({ open: true, reset: latest })
    expect(first).not.toHaveBeenCalled()
    expect(latest).toHaveBeenCalledTimes(1)
  })
})
