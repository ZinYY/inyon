import { describe, expect, test } from 'bun:test'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { RenameSheet } from './RenameSheet.tsx'
import { api } from '../../test/fakes.ts'
import { flush } from '../../test/render.tsx'

const noop = () => {}
const sheet = (p: { open?: boolean; pane?: string; current?: string } = {}) => (
  <RenameSheet host="box" pane={p.pane ?? '%1'} current={p.current ?? 'Login fix'} agent open={p.open ?? true} onClose={noop} />
)
const input = () => screen.getByRole('textbox') as HTMLInputElement

describe('RenameSheet keeps what you type while live data flows', () => {
  test('a refresh, even one with a new title, does not undo typing', async () => {
    const user = userEvent.setup()
    const { rerender } = render(sheet())
    await user.clear(input())
    await user.type(input(), 'Sign')
    rerender(sheet())
    // the agent retitled its terminal meanwhile
    rerender(sheet({ current: 'Refactor auth' }))
    await user.type(input(), 'up flow')
    expect(input().value).toBe('Signup flow')
  })

  test("reopening shows the session's current name", async () => {
    const user = userEvent.setup()
    const { rerender } = render(sheet())
    await user.type(input(), ' (draft)')
    rerender(sheet({ open: false }))
    rerender(sheet({ current: 'Renamed elsewhere' }))
    expect(input().value).toBe('Renamed elsewhere')
  })

  test('switching to another session shows its name', async () => {
    const user = userEvent.setup()
    const { rerender } = render(sheet())
    await user.type(input(), 'x')
    rerender(sheet({ pane: '%2', current: 'Other' }))
    expect(input().value).toBe('Other')
  })

  test('saving sends the typed name; automatic sends an empty one', async () => {
    const user = userEvent.setup()
    render(sheet())
    await user.clear(input())
    await user.type(input(), 'New name')
    await user.click(screen.getByText('Save'))
    await flush()
    expect(api.rename.mock.calls[0]).toEqual(['box', '%1', 'New name'])
    await user.click(screen.getByText('Use automatic name'))
    await flush()
    expect(api.rename.mock.calls[1]).toEqual(['box', '%1', ''])
  })
})
