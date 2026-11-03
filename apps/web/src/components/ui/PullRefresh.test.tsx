import { expect, test } from 'bun:test'
import { act, fireEvent, render } from '@testing-library/react'
import { PullRefresh } from './PullRefresh.tsx'

// Reproduce Chrome clamping a restored offset while the route's first layout is short.
function clampedLayout(check: (layout: { grow: () => void; paint: () => void }) => void) {
  const descriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollTop')
  const request = globalThis.requestAnimationFrame
  const cancel = globalThis.cancelAnimationFrame
  const offsets = new WeakMap<HTMLElement, number>()
  const frames = new Map<number, FrameRequestCallback>()
  let id = 0
  let max = 200
  Object.defineProperty(HTMLElement.prototype, 'scrollTop', {
    configurable: true,
    get() { return offsets.get(this) ?? 0 },
    set(value: number) { offsets.set(this, Math.max(0, Math.min(value, max))) },
  })
  globalThis.requestAnimationFrame = (fn) => { frames.set(++id, fn); return id }
  globalThis.cancelAnimationFrame = (id) => { frames.delete(id) }
  try {
    check({
      grow: () => { max = 3000 },
      paint: () => act(() => {
        const pending = [...frames.values()]
        frames.clear()
        pending.forEach((fn) => fn(performance.now()))
      }),
    })
  } finally {
    if (descriptor) Object.defineProperty(HTMLElement.prototype, 'scrollTop', descriptor)
    else delete (HTMLElement.prototype as any).scrollTop
    globalThis.requestAnimationFrame = request
    globalThis.cancelAnimationFrame = cancel
  }
}

const key = 'inyon:scroll:overview'
const page = () => render(<PullRefresh onRefresh={async () => {}} scrollKey="overview"><p>Overview content</p></PullRefresh>)

test('a short initial layout cannot overwrite the saved position before the page grows', () => {
  clampedLayout(({ grow, paint }) => {
    sessionStorage.setItem(key, '1257')
    const view = page()
    const box = view.container.querySelector('.overscroll-y-contain') as HTMLElement
    expect(box.scrollTop).toBe(200)
    fireEvent.scroll(box)
    paint()
    expect(sessionStorage.getItem(key)).toBe('1257')
    grow()
    paint()
    paint()
    expect(box.scrollTop).toBe(1257)
    expect(sessionStorage.getItem(key)).toBe('1257')
    view.unmount()
  })
})

test('manual scrolling cancels pending restoration instead of pulling the user back', () => {
  clampedLayout(({ grow, paint }) => {
    sessionStorage.setItem(key, '1257')
    const view = page()
    const box = view.container.querySelector('.overscroll-y-contain') as HTMLElement
    fireEvent.wheel(box)
    box.scrollTop = 120
    fireEvent.scroll(box)
    grow()
    paint()
    expect(box.scrollTop).toBe(120)
    expect(sessionStorage.getItem(key)).toBe('120')
    view.unmount()
  })
})
