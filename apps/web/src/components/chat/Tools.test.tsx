import { describe, expect, test } from 'bun:test'
import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { ToolRow } from './Tools.tsx'
import { AssistantTurn, CommandRow } from './Messages.tsx'
import { buildRows } from '../../lib/chat.ts'
import { item } from '../../test/fixtures.ts'

const call = { id: 'b1', name: 'Bash', input: { command: 'ls' }, result: { kind: 'tool_result', seq: 2, uuid: 'r', ts: '', toolUseId: 'b1', isError: false, text: 'total 0' } as any }

describe('a tool row', () => {
  test('opens its body on the first tap, folds it shut on the next and keeps it for the one after', async () => {
    const user = userEvent.setup()
    const { container } = render(<ToolRow call={call} live={false} />)
    const fold = () => container.querySelector('.expand') as HTMLElement | null
    // closed from the start: nothing is mounted, and no border has to appear later
    expect(fold()).toBeNull()
    expect(container.firstElementChild!.className).toContain('border-transparent')
    await user.click(screen.getByRole('button'))
    expect(fold()!.dataset.open).toBe('true')
    expect(fold()!.textContent).toContain('total 0')
    expect(container.firstElementChild!.className).toContain('border-line')
    // the row itself (the open body has a copy button of its own)
    await user.click(screen.getAllByRole('button')[0])
    // still there, so it can fold shut instead of vanishing; inert, so it can't be tabbed into
    expect(fold()!.dataset.open).toBe('false')
    expect(fold()!.textContent).toContain('total 0')
    expect(fold()!.firstElementChild!.hasAttribute('inert')).toBe(true)
  })

  test('a step that joins a reply already on screen comes in rising; the ones drawn with it hold still', () => {
    const turn = (items: Parameters<typeof buildRows>[0]) => {
      const [row] = buildRows(items)
      if (row.type !== 'assistant') throw new Error('not a turn')
      return <AssistantTurn row={row} live />
    }
    const first = [item.tool('t1', 'Bash', { command: 'ls' })]
    const { container, rerender } = render(turn(first))
    const rows = () => [...container.querySelectorAll('.rounded-xl.border')] as HTMLElement[]
    expect(rows()).toHaveLength(1)
    expect(rows()[0].className).not.toContain('animate-row-in')
    act(() => rerender(turn([...first, item.tool('t2', 'Read', { file_path: '/a' })])))
    expect(rows()).toHaveLength(2)
    expect(rows()[0].className).not.toContain('animate-row-in')
    expect(rows()[1].className).toContain('animate-row-in')
  })
})

describe('a command row', () => {
  test('unfolds its output the same way', async () => {
    const user = userEvent.setup()
    const [row] = buildRows([item.command('compact', '', 'Compacted the conversation')])
    if (row.type !== 'command') throw new Error('not a command')
    const { container } = render(<CommandRow row={row} />)
    expect(container.querySelector('.expand')).toBeNull()
    await user.click(screen.getByRole('button'))
    const fold = container.querySelector('.expand') as HTMLElement
    expect(fold.dataset.open).toBe('true')
    expect(fold.className).toContain('w-full')
    expect(screen.getByText('Compacted the conversation')).toBeTruthy()
  })
})
