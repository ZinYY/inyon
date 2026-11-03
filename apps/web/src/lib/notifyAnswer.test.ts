import { afterEach, beforeEach, expect, test } from 'bun:test'
import { ANSWER_TTL_MS, answerFromNotification } from './notifyAnswer.ts'
import { useStore } from './store.ts'
import { api } from '../test/fakes.ts'
import { agentPane, host } from '../test/fixtures.ts'

const posted: any[] = []
const cleared: string[] = []
const asks = (labels: string[]) => host('box', { panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state: 'waiting', options: labels.map((label, i) => ({ n: i + 1, label })) } })] })
const tap = (option: number, label: string) => answerFromNotification({ host: 'box', pane: '%3', option, label, tag: 'box:%3' })

beforeEach(() => {
  posted.length = 0
  cleared.length = 0
  ;(window as any).Capacitor = {
    isNativePlatform: () => true,
    getPlatform: () => 'android',
    Plugins: { InyonDevice: { postNotification: async (o: any) => void posted.push(o), clearNotification: async (o: any) => void cleared.push(o.tag) } },
  }
  useStore.setState({ hosts: [asks(['Yes', 'No'])] })
})
afterEach(() => delete (window as any).Capacitor)

test('an answer tapped on a notification goes to the agent, and the notification goes', async () => {
  await tap(1, 'Yes')
  expect(api.answer).toHaveBeenLastCalledWith('box', '%3', 1)
  expect(cleared).toEqual(['box:%3'])
  expect(posted).toEqual([])
})

test('a late tap never lands on the next question', async () => {
  const calls = api.answer.mock.calls.length
  // the next question has a different answer at 1
  useStore.setState({ hosts: [asks(['Allow once', 'Deny'])] })
  await tap(1, 'Yes')
  // or the session isn't asking at all
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%3', 'claude', { agent: { kind: 'claude', pid: 1, state: 'working' } })] })] })
  await tap(1, 'Yes')
  expect(api.answer.mock.calls.length).toBe(calls)
  expect(posted.map((p) => p.title)).toEqual(['That question changed', 'That question changed'])
  expect(posted[0]).toMatchObject({ href: '/m/box/p/3', tag: 'box:%3' })
})

test('a tap that waited too long for a frozen app is dropped, not sent late', async () => {
  const calls = api.answer.mock.calls.length
  const at = 1_000_000
  await answerFromNotification({ host: 'box', pane: '%3', option: 1, label: 'Yes', tag: 'box:%3', at }, at + ANSWER_TTL_MS + 1)
  expect(api.answer.mock.calls.length).toBe(calls)
  // the phone has said it wasn't sent: nothing posted or cleared over that
  expect(posted).toEqual([])
  expect(cleared).toEqual([])
  // one in time goes
  await answerFromNotification({ host: 'box', pane: '%3', option: 1, label: 'Yes', tag: 'box:%3', at }, at + 2000)
  expect(api.answer).toHaveBeenLastCalledWith('box', '%3', 1)
})

test("an answer the hub refuses says why, on the same notification", async () => {
  api.answer.mockImplementationOnce(async () => {
    throw new Error('The machine is offline')
  })
  await tap(2, 'No')
  expect(posted).toMatchObject([{ title: "Couldn't send your answer", body: 'The machine is offline', tag: 'box:%3', kind: 'waiting' }])
  expect(cleared).toEqual([])
})
