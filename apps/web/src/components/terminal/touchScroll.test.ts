import { afterEach, expect, test } from 'bun:test'
import { touchScroll } from './touchScroll.ts'
import type { IModes } from '@xterm/xterm'

// a terminal 400px tall with 20 rows: 20px a row, so one wheel step every 40px of finger travel
function setup(mouseTrackingMode: IModes['mouseTrackingMode'] = 'vt200') {
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
  const terminal = {
    rows: 20,
    modes: { mouseTrackingMode },
    buffer: { active: { type: 'normal' as 'normal' | 'alternate', viewportY: 100, baseY: 100 } },
    scrollLines(lines: number) {
      const b = this.buffer.active
      b.viewportY = Math.max(0, Math.min(b.baseY, b.viewportY + lines))
    },
  }
  const stop = touchScroll(host, terminal)
  return { host, screen, wheels, reached, terminal, stop }
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

test('PSmux normal-buffer history moves both ways without sending wheel reports or input', () => {
  const { screen, wheels, reached, terminal, stop } = setup('none')
  touch(screen, 'touchstart', 100, 100)
  touch(screen, 'touchmove', 100, 190)
  expect(terminal.buffer.active.viewportY).toBe(96)
  touch(screen, 'touchmove', 100, 200)
  expect(terminal.buffer.active.viewportY).toBe(95)
  touch(screen, 'touchmove', 100, 140)
  expect(terminal.buffer.active.viewportY).toBe(98)
  expect(wheels).toEqual([])
  expect(reached).toEqual([])
  stop()
})

test('an empty normal buffer never turns a drag into command-history arrow keys', () => {
  const { screen, wheels, terminal, stop } = setup('none')
  terminal.buffer.active.baseY = terminal.buffer.active.viewportY = 0
  touch(screen, 'touchstart', 100, 100)
  touch(screen, 'touchmove', 100, 300)
  expect(terminal.buffer.active.viewportY).toBe(0)
  expect(wheels).toEqual([])
  stop()
})

test('switching into a mouse-aware or alternate-screen program routes subsequent drags to it', () => {
  const { screen, wheels, terminal, stop } = setup('none')
  const drag = () => {
    touch(screen, 'touchstart', 100, 100)
    touch(screen, 'touchmove', 100, 180)
  }
  drag()
  expect(terminal.buffer.active.viewportY).toBe(96)
  terminal.modes.mouseTrackingMode = 'vt200'
  drag()
  expect(wheels).toEqual([-1, -1])
  expect(terminal.buffer.active.viewportY).toBe(96)
  terminal.modes.mouseTrackingMode = 'none'
  terminal.buffer.active.type = 'alternate'
  drag()
  expect(wheels).toEqual([-1, -1, -1, -1])
  terminal.buffer.active.type = 'normal'
  drag()
  expect(terminal.buffer.active.viewportY).toBe(92)
  expect(wheels).toHaveLength(4)
  stop()
})
