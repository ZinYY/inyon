import { expect, test } from 'bun:test'
import { renderHook } from '@testing-library/react'
import { AGENTS, type AgentStatus } from '@inyon/protocol'
import { currentModelId, openCodeStatus, useSession } from './useSession.ts'
import { useStore } from '../../lib/store.ts'
import { agentPane, host } from '../../test/fixtures.ts'
import { flush } from '../../test/render.tsx'

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

test("Claude's effort is the model's own level until one is picked, not \"Default\"", async () => {
  // Claude Code's status line names a level only once you choose one
  expect(await effortOf({ model: 'Opus 5.5' })).toBe('high')
  expect(await effortOf({ model: 'Sonnet 5.5' })).toBe('medium')
  expect(await effortOf({ model: 'Opus 5.5', effort: 'low' })).toBe('low')
  // a model whose default isn't known stays unknown
  expect(await effortOf({ model: 'Haiku 5.5' })).toBeUndefined()
})
