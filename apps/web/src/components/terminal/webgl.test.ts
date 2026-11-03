import { expect, test } from 'bun:test'
import type { Terminal } from '@xterm/xterm'
import { attachWebgl } from './webgl.ts'
import { getRenderer, setRenderer } from '../../lib/renderer.ts'

/** A terminal that activates addons the way xterm does, and a WebGL addon that may refuse. */
function fakes(webgl = true) {
  const loaded: unknown[] = []
  const term = { loadAddon: (a: any) => { loaded.push(a); a.activate(term) } } as unknown as Terminal
  let lose = () => {}
  const disposed: unknown[] = []
  class WebglAddon {
    onContextLoss(cb: () => void) { lose = cb; return { dispose() {} } }
    activate() { if (!webgl) throw new Error('WebglAddon: WebGL2 not supported') }
    dispose() { disposed.push(this) }
  }
  return { term, loaded, disposed, loseContext: () => lose(), load: async () => ({ WebglAddon: WebglAddon as any }) }
}

test('the terminal draws with WebGL when the browser has it', async () => {
  setRenderer('dom')
  const f = fakes()
  await attachWebgl(f.term, f.load).done
  expect(f.loaded.length).toBe(1)
  expect(getRenderer()).toBe('webgl')
})

test('without WebGL the default renderer stays', async () => {
  setRenderer('dom')
  const f = fakes(false)
  await attachWebgl(f.term, f.load).done
  expect(getRenderer()).toBe('dom')
})

test('a lost context hands drawing back to the default renderer', async () => {
  const f = fakes()
  await attachWebgl(f.term, f.load).done
  expect(getRenderer()).toBe('webgl')
  f.loseContext()
  expect(f.disposed.length).toBe(1)
  expect(getRenderer()).toBe('dom')
})

test('a terminal closed before the addon arrived never gets it', async () => {
  setRenderer('dom')
  const f = fakes()
  const w = attachWebgl(f.term, f.load)
  w.dispose()
  await w.done
  expect(f.loaded).toEqual([])
  expect(getRenderer()).toBe('dom')
})

test('when the addon cannot be fetched the default renderer stays', async () => {
  setRenderer('dom')
  const f = fakes()
  await attachWebgl(f.term, () => Promise.reject(new Error('offline'))).done
  expect(f.loaded).toEqual([])
  expect(getRenderer()).toBe('dom')
})

const canvasOf = (width: number, clientWidth: number) => ({ querySelectorAll: () => [{ classList: { contains: () => false }, width, clientWidth }] }) as unknown as HTMLElement

test('a WebGL canvas drawn at CSS size on a 2x screen hands drawing back, and the terminal refits', async () => {
  const f = fakes()
  ;(f.term as any).element = canvasOf(698, 698)
  const was = globalThis.devicePixelRatio
  ;(globalThis as any).devicePixelRatio = 2
  let changed = 0
  try {
    await attachWebgl(f.term, f.load, () => changed++).done
  } finally {
    ;(globalThis as any).devicePixelRatio = was
  }
  expect(f.disposed.length).toBe(1)
  expect(getRenderer()).toBe('dom')
  expect(changed).toBe(2) // once for WebGL, once for the way back
})

test('the scale check passes a sharp canvas, and anything on a 1x screen', async () => {
  const { atScreenScale } = await import('./webgl.ts')
  expect(atScreenScale({ element: canvasOf(1396, 698) }, 2)).toBe(true)
  expect(atScreenScale({ element: canvasOf(1950, 698) }, 2.75)).toBe(true)
  expect(atScreenScale({ element: canvasOf(698, 698) }, 2)).toBe(false)
  expect(atScreenScale({ element: canvasOf(698, 698) }, 1)).toBe(true)
  expect(atScreenScale({ element: undefined }, 2)).toBe(true)
})
