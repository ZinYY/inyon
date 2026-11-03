import { afterEach, expect, mock, test } from 'bun:test'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ContextMenuHost, contextMenu, usePageMenu } from './ContextMenu.tsx'

afterEach(cleanup)

function setup(opts: Parameters<typeof contextMenu>[1] = {}) {
  const rename = mock(() => {})
  const remove = mock(() => {})
  render(
    <>
      <div data-testid="row" {...contextMenu(() => [{ key: 'rename', label: 'Rename…', onSelect: rename }, false, { key: 'remove', label: 'Remove…', danger: true, separated: true, onSelect: remove }], opts)}>
        <span>studio</span>
        <input aria-label="Name" />
      </div>
      <div data-testid="other" {...contextMenu(() => [{ key: 'open', label: 'Open', onSelect: () => {} }])}>other</div>
      <ContextMenuHost />
    </>,
  )
  return { rename, remove, row: screen.getByTestId('row') }
}

const rightClick = (el: Element, init: MouseEventInit = {}) => fireEvent.contextMenu(el, { clientX: 120, clientY: 80, ...init })
const items = () => screen.queryAllByRole('menuitem').map((b) => b.textContent)

test('a right-click opens the menu where the pointer is, and picking an item runs it and closes it', () => {
  const { rename, row } = setup()
  fireEvent.pointerDown(row, { pointerType: 'mouse' })
  expect(rightClick(screen.getByText('studio'))).toBe(false) // the browser's menu stays shut
  expect(items()).toEqual(['Rename…', 'Remove…'])
  const menu = screen.getByRole('menu')
  expect([menu.style.left, menu.style.top]).toEqual(['120px', '82px'])
  fireEvent.click(screen.getByRole('menuitem', { name: 'Rename…' }))
  expect(rename).toHaveBeenCalledTimes(1)
  expect(screen.queryByRole('menu')).toBeNull()
})

test('Esc closes it, and the keys move through it', () => {
  const { remove, row } = setup()
  rightClick(row)
  fireEvent.keyDown(window, { key: 'ArrowDown' })
  fireEvent.keyDown(window, { key: 'Enter' })
  expect(remove).toHaveBeenCalledTimes(1)
  rightClick(row)
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(screen.queryByRole('menu')).toBeNull()
})

test("a text box gets Cut, Copy, Paste and Select all instead of the row's menu, and Shift keeps the browser's", () => {
  const { row } = setup()
  expect(rightClick(screen.getByLabelText('Name'))).toBe(false)
  expect(items().map((t) => t?.replace(/(Ctrl|⌘) ?[A-Z]$/, '').trim())).toEqual(['Cut', 'Copy', 'Paste', 'Select all'])
  // nothing selected: nothing to cut or copy
  expect(screen.getByRole('menuitem', { name: /^Cut/ }).hasAttribute('disabled')).toBe(true)
  fireEvent.keyDown(window, { key: 'Escape' })
  expect(rightClick(row, { shiftKey: true })).toBe(true)
  expect(screen.queryByRole('menu')).toBeNull()
})

test('a link gets Open and Copy link', () => {
  render(
    <>
      <a href="https://example.com/docs">docs</a>
      <ContextMenuHost />
    </>,
  )
  fireEvent.pointerDown(screen.getByText('docs'), { pointerType: 'mouse' })
  expect(rightClick(screen.getByText('docs'))).toBe(false)
  expect(items()).toEqual(['Open link', 'Copy link'])
})

test("a right-click on nothing in particular gets the page's menu, not the browser's", () => {
  const search = mock(() => {})
  function Shell() {
    usePageMenu(() => [{ key: 'search', label: 'Search…', keys: 'Ctrl K', onSelect: search }])
    return <p>empty space</p>
  }
  render(
    <>
      <Shell />
      <ContextMenuHost />
    </>,
  )
  fireEvent.pointerDown(screen.getByText('empty space'), { pointerType: 'mouse' })
  expect(rightClick(screen.getByText('empty space'))).toBe(false)
  expect(items()).toContain('Reload')
  expect(items()).toContain('Search…Ctrl K')
  fireEvent.click(screen.getByRole('menuitem', { name: /Search/ }))
  expect(search).toHaveBeenCalledTimes(1)
})

test("a long press on a touch screen keeps doing what it did, unless the menu asks for it", () => {
  const { row } = setup()
  fireEvent.pointerDown(row, { pointerType: 'touch' })
  rightClick(row)
  expect(screen.queryByRole('menu')).toBeNull()
  cleanup()
  const second = setup({ touch: true })
  fireEvent.pointerDown(second.row, { pointerType: 'touch' })
  rightClick(second.row)
  expect(items()).toEqual(['Rename…', 'Remove…'])
  fireEvent.pointerDown(second.row, { pointerType: 'mouse' })
})

test('selected text inside puts Copy first', () => {
  const { row } = setup()
  const range = document.createRange()
  range.selectNodeContents(screen.getByText('studio'))
  window.getSelection()!.removeAllRanges()
  window.getSelection()!.addRange(range)
  rightClick(row)
  expect(items()).toEqual(['Copy', 'Rename…', 'Remove…'])
  window.getSelection()!.removeAllRanges()
})

test('a right-click elsewhere while one is open closes it, and opens the other one', async () => {
  const { row } = setup()
  rightClick(row)
  const other = screen.getByTestId('other')
  const was = document.elementFromPoint
  document.elementFromPoint = () => other
  try {
    // the click lands on the layer that catches clicks outside the menu
    await act(async () => {
      fireEvent.contextMenu(document.querySelector('.fixed.inset-0')!, { clientX: 300, clientY: 200 })
      await new Promise((r) => requestAnimationFrame(() => r(undefined)))
    })
    expect(items()).toEqual(['Open'])
  } finally {
    document.elementFromPoint = was
  }
})
