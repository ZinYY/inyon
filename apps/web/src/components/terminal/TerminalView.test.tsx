import { expect, test } from 'bun:test'
import { fireEvent, render, screen } from '@testing-library/react'
import { TerminalView } from './TerminalView.tsx'
import { useStore } from '../../lib/store.ts'
import { socket } from '../../test/fakes.ts'

test('the Shift button stays pressed after toolbar keys until clicked again', () => {
  // No terminal connection is needed to exercise the actual touch key bar.
  useStore.setState({ connected: false })
  render(<TerminalView host="box" pane="%1" />)
  const shift = screen.getByRole('button', { name: 'Shift' })
  fireEvent.click(shift)
  fireEvent.click(screen.getByRole('button', { name: 'tab' }))
  fireEvent.click(screen.getByRole('button', { name: 'Up' }))
  expect(shift.getAttribute('aria-pressed')).toBe('true')
  expect(socket.send.mock.calls.map(([m]) => (m as any).d)).toEqual(['\x1b[Z', '\x1b[1;2A'])
  fireEvent.click(shift)
  fireEvent.click(screen.getByRole('button', { name: 'tab' }))
  expect(shift.getAttribute('aria-pressed')).toBe('false')
  expect((socket.send.mock.calls.at(-1)![0] as any).d).toBe('\t')
})
