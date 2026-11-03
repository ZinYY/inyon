import { expect, test } from 'bun:test'
import { useState } from 'react'
import { fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Select } from './Select.tsx'
import { renderSettled as render } from '../../test/render.tsx'

const OPTIONS = [
  { value: '', label: 'Default' },
  { value: 'opus', label: 'Opus' },
  { value: 'haiku', label: 'Haiku' },
]

function Pick({ onEscapeOutside }: { onEscapeOutside?: () => void }) {
  const [v, setV] = useState('')
  return (
    <div onKeyDown={(e) => e.key === 'Escape' && onEscapeOutside?.()}>
      <Select title="Model" value={v} onChange={setV} options={OPTIONS} />
      <output>{v || 'none'}</output>
    </div>
  )
}

test('draws its own list instead of the system picker, and picks with a tap', async () => {
  const user = userEvent.setup()
  await render(<Pick />)
  expect(document.querySelector('select')).toBeNull()
  await user.click(screen.getByRole('combobox', { name: 'Model' }))
  expect(screen.getByRole('listbox')).toBeTruthy()
  // the tick on the chosen row only fades, it never shrinks away
  expect(screen.getByRole('option', { name: 'Opus' }).querySelector('svg')!.getAttribute('class')).not.toMatch(/scale-50/)
  await user.click(screen.getByRole('option', { name: 'Opus' }))
  // the phone sheet slides away before it reports the choice
  await new Promise((r) => setTimeout(r, 250))
  expect(screen.getByText('opus')).toBeTruthy()
  expect(screen.queryByRole('listbox')).toBeNull()
})

test('by a mouse the list opens out of the field, all at once; on a phone the sheet brings its rows in one after another', async () => {
  const user = userEvent.setup()
  const first = await render(<Pick />)
  await user.click(screen.getByRole('combobox', { name: 'Model' }))
  const list = screen.getByRole('listbox')
  expect(list.className).not.toContain('stagger')
  expect(list.parentElement!.className).toContain('animate-pop')
  expect(list.parentElement!.className).toContain('origin-top')
  await user.keyboard('{Escape}')
  first.unmount()
  ;(window as any).happyDOM.setViewport({ width: 390, height: 844 })
  try {
    await render(<Pick />)
    await user.click(screen.getByRole('combobox', { name: 'Model' }))
    expect(screen.getByRole('listbox').className).toContain('stagger')
    expect(screen.getByRole('dialog')).toBeTruthy()
  } finally {
    ;(window as any).happyDOM.setViewport({ width: 1024, height: 768 })
  }
})

test('works from the keyboard: arrows move, Enter picks, Escape closes only the list', async () => {
  const user = userEvent.setup()
  let outside = 0
  window.addEventListener('keydown', (e) => e.key === 'Escape' && outside++)
  await render(<Pick />)
  const box = screen.getByRole('combobox', { name: 'Model' })
  box.focus()
  await user.keyboard('{ArrowDown}')
  expect(screen.getByRole('listbox')).toBeTruthy()
  await user.keyboard('{ArrowDown}{ArrowDown}{Enter}')
  expect(screen.getByText('haiku')).toBeTruthy()
  await user.keyboard('{ArrowDown}')
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(screen.queryByRole('listbox')).toBeNull()
  // a sheet underneath listens for Escape on window too; it must not close with the list
  expect(outside).toBe(0)
})
