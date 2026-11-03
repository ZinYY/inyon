import { mock } from 'bun:test'
import type { ServerMsg } from '@inyon/protocol'
import * as realModule from '../lib/api.ts'
import * as realSocket from '../lib/socket.ts'

/**
 * Stand-ins for the network: `api` and `socket` replace the real ones in every test (see
 * setup.ts). `realApi` keeps the real request code for api.test.ts, which fakes `fetch` instead.
 */
export const realApi = { ...realModule.api }
export const realApiModule = { ...realModule }
/** The real socket module, for socket.test.ts (everything else gets the fake `socket` below). */
export const realSocketModule = { ...realSocket }

type Api = typeof realModule.api
type Fake<T> = { [K in keyof T]: T[K] extends (...a: infer A) => infer R ? ReturnType<typeof mock<(...a: A) => R>> : T[K] }

const fakeApi = {} as Fake<Api>
/** Every call resolves to `{}` unless a test says otherwise. */
export function resetApi() {
  for (const k of Object.keys(realApi) as (keyof Api)[]) (fakeApi as any)[k] = mock(async () => ({}))
}
resetApi()
export const api = fakeApi

const listeners = new Set<(m: ServerMsg) => void>()
const closedListeners = new Set<(key: string) => void>()
const statusListeners = new Set<(up: boolean) => void>()
export const socket = {
  up: false,
  start() {},
  stop() {},
  send: mock((_msg: unknown) => {}),
  on(fn: (m: ServerMsg) => void) {
    listeners.add(fn)
    return () => void listeners.delete(fn)
  },
  onChatClosed(fn: (key: string) => void) {
    closedListeners.add(fn)
    return () => void closedListeners.delete(fn)
  },
  onStatus(fn: (up: boolean) => void) {
    statusListeners.add(fn)
    return () => void statusListeners.delete(fn)
  },
  watching: () => false,
  subscribeOverview() {},
  subscribeChat: mock((_host: string, _pane: string) => () => {}),
}

/** The app stops watching a chat, as leaving its page does. */
export const closeChat = (key: string) => closedListeners.forEach((l) => l(key))

/** The socket connects (true) or drops (false), as when the hub restarts. */
export const setSocketUp = (up: boolean) => {
  socket.up = up
  statusListeners.forEach((l) => l(up))
}

/** Delivers a message from the hub, as the socket would. */
export const emit = (m: ServerMsg) => listeners.forEach((l) => l(m))
