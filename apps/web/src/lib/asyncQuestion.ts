import type { ChatItem } from '@inyon/protocol'

export function pendingTextQuestion(items: ChatItem[]) {
  const answered = new Map<string, Set<string> | null>()
  for (const item of items) {
    if (item.kind !== 'tool_result') continue
    const previous = answered.get(item.toolUseId)
    answered.set(item.toolUseId, !item.meta?.answers || previous === null ? null : new Set([...previous ?? [], ...Object.keys(item.meta.answers)]))
  }
  for (const item of [...items].reverse()) {
    if (item.kind === 'user') break
    if (item.kind !== 'assistant' || item.block.type !== 'tool_use') continue
    const call = item.block
    if (call.name !== 'AskUserQuestion' || call.input?.async !== true || answered.get(call.id) === null) continue
    const questions: any[] = call.input.questions ?? []
    const index = questions.findIndex((q: any) => q.question && !(q.options?.length) && !answered.get(call.id)?.has(q.question))
    if (index >= 0) return { call: call.id, index, question: String(questions[index].question) }
  }
}
