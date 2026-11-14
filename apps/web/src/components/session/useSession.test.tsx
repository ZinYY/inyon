import { expect, test } from 'bun:test'
import { act, renderHook } from '@testing-library/react'
import { AGENTS, chatKey, type AgentStatus } from '@inyon/protocol'
import { currentModelId, openCodeStatus, useSession } from './useSession.ts'
import { useStore } from '../../lib/store.ts'
import { agentPane, chatInfo, host } from '../../test/fixtures.ts'
import { flush } from '../../test/render.tsx'
import { api, emit } from '../../test/fakes.ts'

test('reloading chat rebuilds the reader without sending an agent command', async () => {
  const { result } = renderHook(() => useSession('box', '%1'))
  await result.current.actions.reloadChat()
  expect(api.refreshConversation).toHaveBeenCalledWith('box', '%1', expect.objectContaining({ allowWorking: true, allowPrompt: true }))
  expect(api.send).not.toHaveBeenCalled()
  expect(api.restart).not.toHaveBeenCalled()
})

test('Claude shows confirmed controls before another turn, without carrying them across resume', async () => {
  const live: AgentStatus = { model: 'Opus 5.5', effort: 'high', updatedAt: 1000 }
  const confirmed: AgentStatus = { model: 'Sonnet 5.5', modelId: 'sonnet', effort: 'low', updatedAt: 1000 }
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude', { agent: { kind: 'claude', pid: 1, state: 'idle', sessionId: 'one', status: live } })] })] })
  // Use the same incoming chat.info path as the WebSocket, including its conversation identity.
  emit({ t: 'chat.info', key: chatKey('box', '%1'), info: chatInfo({ sessionId: 'one', status: confirmed }) })
  const { result, unmount } = renderHook(() => useSession('box', '%1'))
  await flush()
  expect(result.current.status).toMatchObject({ model: 'Sonnet 5.5', effort: 'low' })
  act(() => emit({ t: 'chat.info', key: chatKey('box', '%1'), info: chatInfo({ sessionId: 'other', status: confirmed }) }))
  await flush()
  expect(result.current.status).toMatchObject({ model: 'Opus 5.5', effort: 'high' })
  unmount()
})

test("OpenCode's footer decides the model and variant; its record keeps the numbers; the footer's fill in", () => {
  const record: AgentStatus = { model: 'opencode/big-pickle', modelId: 'opencode/big-pickle', contextUsed: 15844, costUsd: 0.5, updatedAt: 10 }
  const live: AgentStatus = { model: 'Space Bunny Free', modelId: 'opencode/space-bunny-free', effort: 'high', contextSize: 1048576, contextPct: 2, contextUsed: 20000, costUsd: 0.9, updatedAt: 20 }
  expect(openCodeStatus(record, live)).toEqual({ model: 'Space Bunny Free', modelId: 'opencode/space-bunny-free', effort: 'high', contextSize: 1048576, contextUsed: 15844, costUsd: 0.5, updatedAt: 20 })
  // nothing read yet from the record: the sidebar's numbers stand in
  expect(openCodeStatus(undefined, live)).toMatchObject({ contextUsed: 20000, contextPct: 2, costUsd: 0.9 })
  // the footer hidden and nothing kept: the record alone
  expect(openCodeStatus(record, { updatedAt: 0 })).toMatchObject({ model: 'opencode/big-pickle', contextUsed: 15844 })
  expect(openCodeStatus(undefined, undefined)).toBeUndefined()
})

test('an exact model id wins, and a name never stands in for a longer one', () => {
  const def = { ...AGENTS.opencode, models: [
    { id: 'openai/gpt-5', label: 'OpenAI · GPT-5' },
    { id: 'openai/gpt-5.1-codex', label: 'OpenAI · GPT-5.1 Codex' },
    { id: 'openrouter/openai/gpt-5', label: 'OpenRouter · GPT-5' },
  ] }
  expect(currentModelId(def, { modelId: 'openai/gpt-5', updatedAt: 0 })).toBe('openai/gpt-5')
  expect(currentModelId(def, { modelId: 'openai/gpt-5.1-codex', updatedAt: 0 })).toBe('openai/gpt-5.1-codex')
  expect(currentModelId(def, { model: 'GPT-5.1 Codex', updatedAt: 0 })).toBe('openai/gpt-5.1-codex')
  expect(currentModelId(def, { model: 'OpenRouter · GPT-5', updatedAt: 0 })).toBe('openrouter/openai/gpt-5')
  expect(currentModelId(def, undefined, 'openai/gpt-5.1-codex')).toBe('openai/gpt-5.1-codex')
  expect(currentModelId(AGENTS.claude, { model: 'Opus 5.5', updatedAt: 0 })).toBe('opus')
})

const effortOf = async (status: Omit<AgentStatus, 'updatedAt'>) => {
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude', { agent: { kind: 'claude', pid: 1, state: 'idle', status: { ...status, updatedAt: 1 } } })] })] })
  const { result } = renderHook(() => useSession('box', '%1'))
  await flush()
  return result.current.status?.effort
}

test("missing effort stays unknown; only an explicit default uses the model catalogue", async () => {
  expect(await effortOf({ model: 'Opus 5.5' })).toBeUndefined()
  expect(await effortOf({ model: 'Sonnet 5.5' })).toBeUndefined()
  expect(await effortOf({ model: 'Opus 5.5', effort: 'default' })).toBe('high')
  expect(await effortOf({ model: 'Sonnet 5.5', effort: 'default' })).toBe('medium')
  expect(await effortOf({ model: 'Opus 5.5', effort: 'xhigh' })).toBe('xhigh')
  expect(await effortOf({ model: 'Opus 5.5', effort: 'low' })).toBe('low')
  // a model whose default isn't known stays unknown
  expect(await effortOf({ model: 'Haiku 5.5' })).toBeUndefined()
})
