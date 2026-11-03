// Preloaded by bun test (bunfig.toml): a browser DOM, and the network swapped for fakes.
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { afterEach, mock } from 'bun:test'

GlobalRegistrator.register({ url: 'http://localhost:5173/' })
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

// the rest after the DOM exists: these modules touch `document` and `window` as they load
const fakes = await import('./fakes.ts')
mock.module(new URL('../lib/api.ts', import.meta.url).pathname, () => ({ ...fakes.realApiModule, api: fakes.api }))
mock.module(new URL('../lib/socket.ts', import.meta.url).pathname, () => ({ socket: fakes.socket }))

const { cleanup } = await import('@testing-library/react')
const { useStore, forgetEnded } = await import('../lib/store.ts')
const initial = useStore.getState()

afterEach(() => {
  cleanup()
  fakes.resetApi()
  fakes.socket.send.mockClear()
  fakes.socket.subscribeChat.mockClear()
  fakes.socket.up = false
  useStore.setState(initial, true)
  forgetEnded()
  localStorage.clear()
  sessionStorage.clear()
})
