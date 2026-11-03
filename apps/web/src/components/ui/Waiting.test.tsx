import { describe, expect, test } from 'bun:test'
import { render, screen } from '@testing-library/react'
import { Waiting } from './primitives.tsx'

const blocks = (el: HTMLElement) => el.querySelectorAll('.skeleton')

describe('Waiting', () => {
  test('says what it waits for, in one line a screen reader announces', () => {
    const { container } = render(<Waiting label="Connecting to build-box" />)
    const status = screen.getByRole('status')
    expect(status.textContent).toBe('Connecting to build-box')
    // the label is the only text: the blocks are empty
    expect(container.querySelector('p')!.className).toContain('text-[12.5px]')
    expect(container.querySelector('p')!.className).toContain('text-fg-3')
    expect(container.querySelector('.animate-spin')).toBeNull()
  })

  test('each shape is made of skeleton blocks, which the stylesheet stops under reduced motion', () => {
    const lines = render(<Waiting label="Loading older messages" shape="lines" />)
    expect(blocks(lines.container)).toHaveLength(3)
    expect([...blocks(lines.container)].map((b) => (b as HTMLElement).style.width)).toEqual(['60%', '90%', '75%'])
    lines.unmount()

    const cards = render(<Waiting label="Opening Inyon" shape="cards" />)
    expect(blocks(cards.container)).toHaveLength(4)
    expect(blocks(cards.container)[0].className).toContain('rounded-3xl')
    cards.unmount()

    const rows = render(<Waiting label="Reading the session" shape="rows" />)
    expect(blocks(rows.container)).toHaveLength(3)
    expect(blocks(rows.container)[0].className).toContain('rounded-2xl')
    rows.unmount()

    const panel = render(<Waiting label="Reading this phone's key" shape="panel" />)
    expect(blocks(panel.container)).toHaveLength(1)
  })

  test('a count fits the skeleton to the slot', () => {
    const { container } = render(<Waiting label="Loading hub settings" shape="rows" count={1} className="px-4" />)
    expect(blocks(container)).toHaveLength(1)
    expect(screen.getByRole('status').className).toContain('px-4')
  })
})
