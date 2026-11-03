import { expect, test } from 'bun:test'
import { act, render } from '@testing-library/react'
import { AnimatedList, LEAVE_MS } from './AnimatedList.tsx'

const items = (...keys: string[]) => keys.map((key) => ({ key, node: <span>{key}</span> }))
const sleep = (ms: number) => act(() => new Promise((r) => setTimeout(r, ms)))

test('a new item arrives rising; one that was there does not', () => {
  const { container, rerender } = render(<AnimatedList items={items('a')} />)
  expect(container.querySelector('[data-key="a"]')!.className).toContain('animate-slide-up')
  rerender(<AnimatedList items={items('a', 'b')} />)
  expect(container.querySelector('[data-key="a"]')!.className).not.toContain('animate-slide-up')
  expect(container.querySelector('[data-key="b"]')!.className).toContain('animate-slide-up')
})

test('an item that goes fades first, then leaves', async () => {
  const { container, rerender } = render(<AnimatedList items={items('a', 'b')} />)
  rerender(<AnimatedList items={items('b')} />)
  const a = container.querySelector('[data-key="a"]')!
  expect(a.className).toContain('leave')
  expect(a.textContent).toBe('a')
  await sleep(LEAVE_MS + 20)
  expect(container.querySelector('[data-key="a"]')).toBeNull()
  expect(container.querySelector('[data-key="b"]')).not.toBeNull()
})

test('an item that goes fades where it was, not in the last cell', async () => {
  const keys = (c: HTMLElement) => [...c.querySelectorAll('[data-key]')].map((el) => el.getAttribute('data-key'))
  const { container, rerender } = render(<AnimatedList items={items('a', 'b', 'c', 'd')} />)
  rerender(<AnimatedList items={items('a', 'd')} />)
  expect(keys(container)).toEqual(['a', 'b', 'c', 'd'])
  expect(container.querySelector('[data-key="b"]')!.className).toContain('leave')
  expect(container.querySelector('[data-key="c"]')!.className).toContain('leave')
  // one added meanwhile goes after the ones still there
  rerender(<AnimatedList items={items('a', 'd', 'e')} />)
  expect(keys(container)).toEqual(['a', 'b', 'c', 'd', 'e'])
  await sleep(LEAVE_MS + 20)
  expect(keys(container)).toEqual(['a', 'd', 'e'])
})

test('the first batch comes in one row after another; later arrivals come at once', () => {
  const { container, rerender } = render(<AnimatedList items={items('a', 'b', 'c')} />)
  const row = (k: string) => container.querySelector(`[data-key="${k}"]`) as HTMLElement
  for (const [k, i] of [['a', 0], ['b', 1], ['c', 2]] as const) {
    expect(row(k).className).toContain('staggered')
    expect(row(k).style.getPropertyValue('--i')).toBe(String(i))
  }
  // a refresh with the same items leaves the rows as they are, mid-rise or not
  rerender(<AnimatedList items={items('a', 'b', 'c')} />)
  expect(row('a').className).toContain('animate-slide-up')
  rerender(<AnimatedList items={items('a', 'b', 'c', 'd')} />)
  expect(row('d').className).toContain('animate-slide-up')
  expect(row('d').className).not.toContain('staggered')
  expect(row('d').style.getPropertyValue('--i')).toBe('')
  expect(row('a').className).not.toContain('animate-slide-up')
})

test('a list can name the way its items arrive', () => {
  const { container } = render(<AnimatedList items={items('a')} enter="animate-toast-in" />)
  expect(container.querySelector('[data-key="a"]')!.className).toContain('animate-toast-in')
  expect(container.querySelector('[data-key="a"]')!.className).not.toContain('animate-slide-up')
})

test('an item that comes straight back is not shown twice', async () => {
  const { container, rerender } = render(<AnimatedList items={items('a', 'b')} />)
  rerender(<AnimatedList items={items('b')} />)
  rerender(<AnimatedList items={items('a', 'b')} />)
  expect(container.querySelectorAll('[data-key="a"]')).toHaveLength(1)
  expect(container.querySelector('[data-key="a"]')!.className).not.toContain('leave')
  await sleep(LEAVE_MS + 20)
  expect(container.querySelectorAll('[data-key="a"]')).toHaveLength(1)
})

test('items that reflow because the list changed width do not glide; a real move does', () => {
  const real = HTMLElement.prototype.getBoundingClientRect
  let listWidth = 800
  const at: Record<string, number> = { a: 0, b: 400 }
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const key = this.dataset.key
    const left = key ? at[key] ?? 0 : 0
    return { left, top: 0, width: key ? 300 : listWidth, height: 100, right: left + 300, bottom: 100, x: left, y: 0, toJSON() {} } as DOMRect
  }
  try {
    const { container, rerender } = render(<AnimatedList items={items('a', 'b')} />)
    const b = () => container.querySelector('[data-key="b"]') as HTMLElement
    // the window settles narrower: b drops under a
    listWidth = 400
    at.b = 0
    rerender(<AnimatedList items={items('a', 'b')} />)
    expect(b().style.transform).toBe('')
    // same width, b moves because the order changed: it glides from where it was
    at.b = 200
    rerender(<AnimatedList items={items('a', 'b')} />)
    expect(b().style.transform).toBe('translate(-200px, 0px)')
  } finally {
    HTMLElement.prototype.getBoundingClientRect = real
  }
})
