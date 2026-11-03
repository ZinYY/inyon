import { beforeEach, expect, test } from 'bun:test'
import type { HostState } from '@inyon/protocol'
import { connLog, why } from './connlog.ts'
import { followHosts, installConnWatch } from './connwatch.ts'
import { useStore } from './store.ts'

const host = (id: string, status: HostState['status'], error?: string, kind = 'link') => ({ config: { id, name: id, kind }, status, error, sessions: [], agents: [] }) as unknown as HostState

beforeEach(() => {
  for (const s of connLog.all()) connLog.forget(s.id)
})

test('a computer that goes from online to connecting dropped, with its own words as the reason', () => {
  const seen = new Map()
  followHosts([host('pc', 'connecting')], seen)
  // starting up is not a drop
  expect(connLog.get('pc')).toBeUndefined()
  followHosts([host('pc', 'online')], seen)
  expect(connLog.get('pc')).toMatchObject({ kind: 'computer', up: true })
  followHosts([host('pc', 'connecting')], seen)
  expect(connLog.lastDrop('pc')).toMatchObject({ cause: 'closed' })
  followHosts([host('pc', 'connecting', "Connecting to pc… (Couldn't find the computer online.)")], seen)
  expect(connLog.lastDrop('pc')).toMatchObject({ tries: 1, last: "Connecting to pc… (Couldn't find the computer online.)" })
  followHosts([host('pc', 'online')], seen)
  expect(connLog.lastDrop('pc')!.back).toBeNumber()
})

test("a hub's machine that the hub lost, and one first seen unpaired", () => {
  const seen = new Map()
  followHosts([host('srv', 'online', undefined, 'node')], seen)
  followHosts([host('srv', 'error', 'Machine offline', 'node')], seen)
  expect(connLog.get('srv')?.kind).toBe('machine')
  expect(why(connLog.lastDrop('srv')!)).toBe('Machine offline')
  followHosts([host('pc', 'error', 'This computer is no longer paired with this phone')], seen)
  expect(connLog.lastDrop('pc')).toMatchObject({ cause: 'signed-out' })
  // the hub itself out of reach is the hub's drop, not each machine's
  followHosts([host('hub:h1/a', 'connecting', 'Connecting to studio…', 'node')], seen)
  expect(connLog.get('hub:h1/a')).toBeUndefined()
})

test("its hub going away isn't a drop for each machine, and a reason of its own after is", () => {
  const seen = new Map()
  followHosts([host('hub:h1/a', 'online', undefined, 'node')], seen)
  followHosts([host('hub:h1/a', 'connecting', 'Reconnecting to studio', 'node')], seen)
  expect(connLog.lastDrop('hub:h1/a')).toBeUndefined()
  // first seen while its hub was out of reach, then the hub says why it isn't there
  followHosts([host('hub:h1/b', 'connecting', 'Connecting to studio…', 'node')], seen)
  followHosts([host('hub:h1/b', 'error', 'Machine offline', 'node')], seen)
  expect(why(connLog.lastDrop('hub:h1/b')!)).toBe('Machine offline')
})

test("the hub's first answer counts even when it matches the kept copy", () => {
  installConnWatch()
  const hosts = [host('pc', 'online')]
  useStore.setState({ hosts, hostsStale: true })
  expect(connLog.get('pc')).toBeUndefined()
  // same list, now confirmed
  useStore.setState({ hostsStale: false })
  expect(connLog.get('pc')?.up).toBe(true)
  useStore.setState({ hosts: [host('pc', 'connecting')] })
  expect(connLog.lastDrop('pc')).toMatchObject({ cause: 'closed' })
})
