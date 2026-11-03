import { afterEach, beforeEach, expect, mock, test } from 'bun:test'
import { act, render, screen } from '@testing-library/react'
import { Intro, introDue } from './Intro.tsx'

beforeEach(() => sessionStorage.removeItem('inyon.intro.seen'))
afterEach(() => {
  document.documentElement.classList.remove('intro-running')
  document.querySelectorAll('[data-intro-target]').forEach((el) => el.remove())
})

const sleep = (ms: number) => act(() => new Promise((r) => setTimeout(r, ms)))

/** A header mark on the page, where the intro lands. */
function header() {
  const el = document.createElement('div')
  el.setAttribute('data-intro-target', 'true')
  el.getBoundingClientRect = () =>
    ({
      left: 16,
      top: 13,
      width: 26,
      height: 26,
      right: 42,
      bottom: 39,
      x: 16,
      y: 13,
      toJSON() {},
    }) as DOMRect
  document.body.appendChild(el)
  return el
}

test('the intro is due once per app session', () => {
  expect(introDue()).toBe(true)
  expect(introDue()).toBe(false)
})

test('while it runs the header mark is hidden; when it is done it is gone', async () => {
  header()
  const done = mock(() => {})
  render(<Intro onDone={done} />)
  expect(document.documentElement.classList.contains('intro-running')).toBe(true)
  expect(screen.getByTestId('intro')).toBeTruthy()
  await sleep(1800)
  expect(screen.queryByTestId('intro')).toBeNull()
  expect(document.documentElement.classList.contains('intro-running')).toBe(false)
  expect(done).toHaveBeenCalledTimes(1)
})

test('with the page still loading it holds the finished mark until the header is there', async () => {
  render(<Intro />)
  await sleep(1400)
  // the drawing is done, there is nothing to land on yet: it waits
  expect(screen.getByTestId('intro').className).not.toContain('intro-off')
  const mark = screen.getByTestId('intro').querySelector('.intro-mark') as HTMLElement
  mark.getBoundingClientRect = () =>
    ({
      left: 115,
      top: 342,
      width: 160,
      height: 160,
      right: 275,
      bottom: 502,
      x: 115,
      y: 342,
      toJSON() {},
    }) as DOMRect
  header()
  await sleep(150)
  expect(screen.getByTestId('intro').className).toContain('intro-off')
  expect(mark.style.transform).toBe(`translate(${16 + 13 - (115 + 80)}px, ${13 + 13 - (342 + 80)}px) scale(${26 / 160})`)
  await sleep(800)
  expect(screen.queryByTestId('intro')).toBeNull()
})

test('with no header mark at all it fades out in place once the hold is over', async () => {
  const done = mock(() => {})
  render(<Intro onDone={done} />)
  await sleep(1800)
  expect(screen.getByTestId('intro').className).not.toContain('intro-off')
  await sleep(1300)
  expect(screen.queryByTestId('intro')).toBeNull()
  expect(done).toHaveBeenCalledTimes(1)
})

test('a tap skips to the hand-off and it finishes quickly', async () => {
  const done = mock(() => {})
  render(<Intro onDone={done} />)
  act(() => {
    screen.getByTestId('intro').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
  })
  // the hand-off starts on the next frame
  await sleep(50)
  expect(screen.getByTestId('intro').className).toContain('intro-off')
  await sleep(800)
  expect(screen.queryByTestId('intro')).toBeNull()
  expect(done).toHaveBeenCalledTimes(1)
})

test('with a header mark on screen the flight ends exactly on it', async () => {
  header()
  render(<Intro />)
  const mark = screen.getByTestId('intro').querySelector('.intro-mark') as HTMLElement
  mark.getBoundingClientRect = () =>
    ({
      left: 115,
      top: 342,
      width: 160,
      height: 160,
      right: 275,
      bottom: 502,
      x: 115,
      y: 342,
      toJSON() {},
    }) as DOMRect
  act(() => {
    screen.getByTestId('intro').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
  })
  await sleep(50)
  // from the centre of the mark to the centre of the header's, scaled to its size
  expect(mark.style.transform).toBe(`translate(${16 + 13 - (115 + 80)}px, ${13 + 13 - (342 + 80)}px) scale(${26 / 160})`)
})
