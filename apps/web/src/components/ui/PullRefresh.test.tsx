import { expect, mock, test } from 'bun:test'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { PullRefresh } from './PullRefresh.tsx'

// a finger on the page: down at y=100, moved to each y in turn, then lifted
function drag(el: Element, ...ys: number[]) {
  const touch = (y: number) => ({ clientY: y, identifier: 0, target: el }) as unknown as Touch
  const fire = (type: string, y?: number) =>
    el.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true, touches: y === undefined ? [] : [touch(y)] }))
  act(() => {
    fire('touchstart', 100)
    for (const y of ys) fire('touchmove', y)
    fire('touchend')
  })
}

function setup() {
  let done!: () => void
  const onRefresh = mock(() => new Promise<void>((r) => (done = r)))
  render(
    <PullRefresh onRefresh={onRefresh}>
      <p>Sessions</p>
    </PullRefresh>,
  )
  const page = screen.getByText('Sessions').parentElement!
  return { onRefresh, page, done: () => done() }
}

test('pulling down far enough from the top refreshes, and shows it until done', async () => {
  const { onRefresh, page, done } = setup()
  drag(page, 140, 200, 260)
  expect(onRefresh).toHaveBeenCalledTimes(1)
  expect(screen.getByRole('status', { name: 'Refreshing' })).toBeTruthy()
  // a second pull while it's still going does nothing
  drag(page, 200, 260)
  expect(onRefresh).toHaveBeenCalledTimes(1)
  await act(async () => {
    done()
    await new Promise((r) => setTimeout(r, 650))
  })
  expect(screen.queryByRole('status', { name: 'Refreshing' })).toBeNull()
})

test("a short pull, or one that starts scrolled down, doesn't", () => {
  const { onRefresh, page } = setup()
  drag(page, 130, 150)
  expect(onRefresh).not.toHaveBeenCalled()
  Object.defineProperty(page, 'scrollTop', { value: 300, configurable: true })
  drag(page, 200, 300)
  expect(onRefresh).not.toHaveBeenCalled()
})

test("the mark doesn't turn: its line pays out as you pull, then reels while it refreshes", async () => {
  const { page, done } = setup()
  const mark = () => document.querySelector('.mark-pull, .mark-reeling') as HTMLElement
  const touch = (y: number) => ({ clientY: y, identifier: 0, target: page }) as unknown as Touch
  const fire = (type: string, y?: number) => page.dispatchEvent(new TouchEvent(type, { bubbles: true, cancelable: true, touches: y === undefined ? [] : [touch(y)] }))
  act(() => {
    fire('touchstart', 100)
    fire('touchmove', 164) // 32px after damping: half way
  })
  expect(mark().className).toContain('mark-pull')
  expect(mark().style.getPropertyValue('--pull')).toBe('0.5')
  expect(mark().style.transform).toBe('')
  act(() => {
    fire('touchmove', 260)
    fire('touchend')
  })
  expect(mark().className).toContain('mark-reeling')
  await act(async () => {
    done()
    await new Promise((r) => setTimeout(r, 650))
  })
})

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
