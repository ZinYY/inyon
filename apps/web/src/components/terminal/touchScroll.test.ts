import { afterEach, expect, test } from 'bun:test'
import { touchScroll } from './touchScroll.ts'

// a terminal 400px tall with 20 rows: 20px a row, so one wheel step every 40px of finger travel
function setup() {
  const host = document.createElement('div')
  Object.defineProperty(host, 'clientHeight', { value: 400 })
  const screen = document.createElement('div')
  screen.className = 'xterm-screen'
  host.appendChild(screen)
  document.body.appendChild(host)
  const wheels: number[] = []
  screen.addEventListener('wheel', (e) => wheels.push((e as WheelEvent).deltaY))
  // what the terminal's own touch handling would see
  const reached: string[] = []
  screen.addEventListener('touchmove', () => reached.push('move'))
  const stop = touchScroll(host, () => 20)
  return { host, screen, wheels, reached, stop }
}

const touch = (el: HTMLElement, type: string, x: number, y: number) => {
  const e = new TouchEvent(type, { bubbles: true, cancelable: true, touches: type === 'touchend' ? [] : ([{ clientX: x, clientY: y }] as any) })
  el.dispatchEvent(e)
  return e
}

afterEach(() => (document.body.innerHTML = ''))

test('dragging down scrolls up into the history, one wheel step per two rows', () => {
  const { screen, wheels, reached, stop } = setup()
  touch(screen, 'touchstart', 100, 100)
  const move = touch(screen, 'touchmove', 100, 190)
  expect(move.defaultPrevented).toBe(true)
  // 90px: two whole steps, the rest carried over to the next move
  expect(wheels).toEqual([-1, -1])
  touch(screen, 'touchmove', 100, 205)
  expect(wheels).toEqual([-1, -1])
  touch(screen, 'touchmove', 100, 220)
  expect(wheels).toEqual([-1, -1, -1])
  // the terminal's own touch scrolling never sees the drag
  expect(reached).toEqual([])
  stop()
})

test('dragging up scrolls back down toward the newest lines', () => {
  const { screen, wheels, stop } = setup()
  touch(screen, 'touchstart', 100, 300)
  touch(screen, 'touchmove', 100, 215)
  expect(wheels).toEqual([1, 1])
  stop()
})

test('a tap or a sideways drag is left alone', () => {
  const { screen, wheels, reached, stop } = setup()
  touch(screen, 'touchstart', 100, 100)
  const tap = touch(screen, 'touchmove', 102, 103)
  expect(tap.defaultPrevented).toBe(false)
  touch(screen, 'touchend', 102, 103)
  touch(screen, 'touchstart', 100, 100)
  touch(screen, 'touchmove', 180, 112)
  expect(wheels).toEqual([])
  expect(reached).toEqual(['move', 'move'])
  stop()
})

test('after it stops listening, drags do nothing', () => {
  const { screen, wheels, stop } = setup()
  stop()
  touch(screen, 'touchstart', 100, 100)
  touch(screen, 'touchmove', 100, 300)
  expect(wheels).toEqual([])
})
