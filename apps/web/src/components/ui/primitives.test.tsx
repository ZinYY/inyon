import { afterEach, describe, expect, test } from 'bun:test'
import { useState } from 'react'
import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Menu, Segmented, Sheet } from './primitives.tsx'

const viewport = (width: number, height: number) => act(() => {
  ;(window as any).happyDOM.setViewport({ width, height })
  window.dispatchEvent(new Event('resize'))
})

describe('Menu in a short viewport', () => {
  // happy-dom has no layout: the trigger says where it is and the menu how tall its rows are
  const rows = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight')
  afterEach(() => {
    if (rows) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', rows)
    else delete (HTMLElement.prototype as any).scrollHeight
    ;(window as any).happyDOM.setViewport({ width: 1024, height: 768 })
  })

  async function openAt(top: number, height = 32) {
    Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get() { return this.getAttribute('role') === 'menu' ? 224 : 0 } })
    const items = ['Opus', 'Sonnet', 'Haiku', 'Fable'].map((k) => ({ key: k, label: k, onSelect() {} }))
    render(<Menu items={items} trigger={({ toggle, ref }) => <button ref={ref} onClick={toggle}>Model</button>} />)
    const trigger = screen.getByText('Model')
    trigger.getBoundingClientRect = () => ({ top, bottom: top + height, left: 80, right: 160, width: 80, height, x: 80, y: top, toJSON() {} }) as DOMRect
    await userEvent.setup().click(trigger)
    return screen.getByRole('menu')
  }

  test('a phone on its side: opens above the composer instead of below the screen', async () => {
    viewport(568, 320)
    const menu = await openAt(250)
    // above the trigger (top 250, 6px gap), never taller than the room up there
    expect(menu.style.top).toBe('')
    expect(menu.style.bottom).toBe(`${320 - 244}px`)
    expect(parseFloat(menu.style.maxHeight)).toBeLessThanOrEqual(244 - 8)
    // and it rises out of the button rather than dropping onto it
    expect(menu.className).toContain('animate-pop-up')
    expect(menu.className).toContain('origin-bottom-left')
  })

  test('no room for all of it on either side: takes the bigger side and scrolls', async () => {
    viewport(568, 320)
    const menu = await openAt(140)
    // 134px below against 126px above
    expect(menu.style.top).toBe('178px')
    expect(menu.style.maxHeight).toBe('134px')
    expect(menu.classList.contains('animate-pop')).toBe(true)
    expect(menu.classList.contains('origin-top-left')).toBe(true)
  })

  test('moves when the viewport changes while it is open', async () => {
    viewport(412, 839)
    const menu = await openAt(250)
    expect(menu.style.top).toBe('288px')
    viewport(568, 320)
    expect(menu.style.top).toBe('')
    expect(menu.style.bottom).toBe('76px')
  })
})

describe('Segmented', () => {
  test('an option that is only an icon still has a name, and the group says what it chooses', () => {
    render(<Segmented label="View" value="chat" onChange={() => {}} options={[{ v: 'chat', name: 'Chat', label: <svg /> }, { v: 'terminal', name: 'Terminal', label: <svg /> }]} />)
    expect(screen.getByRole('radiogroup', { name: 'View' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'Chat' }).getAttribute('aria-checked')).toBe('true')
    expect(screen.getByRole('radio', { name: 'Terminal' }).getAttribute('aria-checked')).toBe('false')
  })
})

describe('Sheet', () => {
  let behind = 0
  function Page() {
    const [open, setOpen] = useState(false)
    return (
      <>
        <button onClick={() => setOpen(true)}>Open</button>
        <button onClick={() => behind++}>Behind</button>
        <Sheet open={open} onClose={() => setOpen(false)} title="Rename" footer={<button>Save</button>}>
          <input aria-label="Name" />
        </Sheet>
      </>
    )
  }

  test('is named by its title, takes focus, keeps Tab inside and gives focus back', async () => {
    behind = 0
    const user = userEvent.setup()
    render(<Page />)
    await user.click(screen.getByText('Open'))
    const dialog = screen.getByRole('dialog', { name: 'Rename' })
    expect(dialog.contains(document.activeElement)).toBe(true)

    // Close, Name, Save, then round again; never the page behind
    const seen: string[] = []
    for (let i = 0; i < 4; i++) {
      await user.tab()
      seen.push(document.activeElement?.getAttribute('aria-label') ?? document.activeElement?.textContent ?? '')
    }
    expect(seen).toEqual(['Close', 'Name', 'Save', 'Close'])
    await user.tab({ shift: true })
    expect(document.activeElement?.textContent).toBe('Save')
    await user.keyboard('{Enter}')
    expect(behind).toBe(0)

    await user.keyboard('{Escape}')
    await act(() => Bun.sleep(250))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement?.textContent).toBe('Open')
  })

  test('Escape closes only the sheet on top', async () => {
    function Stacked() {
      const [outer, setOuter] = useState(true)
      const [inner, setInner] = useState(true)
      return (
        <>
          <Sheet open={outer} onClose={() => setOuter(false)} title="Files">
            <p>list</p>
          </Sheet>
          <Sheet open={inner} onClose={() => setInner(false)} title="Preview">
            <p>file</p>
          </Sheet>
        </>
      )
    }
    const user = userEvent.setup()
    render(<Stacked />)
    await act(() => Bun.sleep(10))
    await user.keyboard('{Escape}')
    await act(() => Bun.sleep(250))
    expect(screen.queryByRole('dialog', { name: 'Preview' })).toBeNull()
    expect(screen.getByRole('dialog', { name: 'Files' })).toBeTruthy()
    await user.keyboard('{Escape}')
    await act(() => Bun.sleep(250))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  test("a field's own autoFocus wins, and focus still goes back to the opener", async () => {
    function Auto() {
      const [open, setOpen] = useState(false)
      return (
        <>
          <button onClick={() => setOpen(true)}>Open</button>
          {open && (
            <Sheet open onClose={() => setOpen(false)} title="Name it">
              <input aria-label="Name" autoFocus />
            </Sheet>
          )}
        </>
      )
    }
    const user = userEvent.setup()
    render(<Auto />)
    await user.click(screen.getByText('Open'))
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Name')
    await user.click(screen.getByLabelText('Close'))
    expect(document.activeElement?.textContent).toBe('Open')
  })
})
