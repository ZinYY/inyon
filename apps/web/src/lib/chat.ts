import type { ChatItem } from '@inyon/protocol'

type ItemOf<K extends ChatItem['kind']> = Extract<ChatItem, { kind: K }>
export type UserItem = ItemOf<'user'>
export type ToolResult = ItemOf<'tool_result'>

export interface ToolCall {
  id: string
  name: string
  input: any
  result?: ToolResult
  /** when the call was made */
  ts?: string
}

export type Part =
  | { type: 'text'; key: string; text: string }
  | { type: 'thinking'; key: string; text: string }
  | { type: 'tools'; key: string; calls: ToolCall[] }
  | { type: 'special'; key: string; call: ToolCall } // AskUserQuestion, ExitPlanMode, TodoWrite
  | { type: 'notice'; key: string; item: ItemOf<'notice'> }

export type Row =
  | { type: 'user'; key: string; item: UserItem }
  | { type: 'assistant'; key: string; parts: Part[]; ts: string; durationMs?: number; open: boolean }
  | { type: 'command'; key: string; name: string; args: string; output?: string }
  | { type: 'notice'; key: string; item: ItemOf<'notice'> }
  | { type: 'goal'; key: string; item: ItemOf<'goal'> }

const SPECIAL = new Set(['AskUserQuestion', 'ExitPlanMode', 'TodoWrite'])
/**
 * Slash commands that only change a setting (the app's own controls send these). The header shows
 * the setting itself, so they aren't messages in the conversation. A goal shows as its own row.
 */
const SETTINGS = new Set(['model', 'models', 'effort', 'fast', 'permissions', 'approvals', 'mode', 'agent', 'agents', 'output-style', 'goal'])

/** Groups a flat transcript into conversation rows: user bubbles and assistant turns. */
export function buildRows(items: ChatItem[]): Row[] {
  const rows: Row[] = []
  const results = new Map<string, ToolResult>()
  for (const i of items) if (i.kind === 'tool_result') results.set(i.toolUseId, i)

  let turn: Extract<Row, { type: 'assistant' }> | null = null
  /** the last command was a setting, so the output after it is hidden too */
  let hidden = false
  const closeTurn = () => {
    if (turn) turn.open = false
    turn = null
  }
  const ensureTurn = (i: ChatItem) => {
    if (!turn) {
      turn = { type: 'assistant', key: `a${i.seq}`, parts: [], ts: i.ts, open: true }
      rows.push(turn)
    }
    return turn
  }

  for (const i of items) {
    if (i.kind !== 'command') hidden = false
    switch (i.kind) {
      case 'user':
        closeTurn()
        rows.push({ type: 'user', key: `u${i.seq}`, item: i })
        break
      case 'assistant': {
        const t = ensureTurn(i)
        const b = i.block
        if (b.type === 'text') t.parts.push({ type: 'text', key: i.uuid, text: b.text })
        else if (b.type === 'thinking') t.parts.push({ type: 'thinking', key: i.uuid, text: b.text })
        else {
          const call: ToolCall = { id: b.id, name: b.name, input: b.input, result: results.get(b.id), ts: i.ts }
          if (SPECIAL.has(b.name)) t.parts.push({ type: 'special', key: i.uuid, call })
          else {
            const last = t.parts[t.parts.length - 1]
            if (last?.type === 'tools') last.calls.push(call)
            else t.parts.push({ type: 'tools', key: i.uuid, calls: [call] })
          }
        }
        break
      }
      case 'turn_end':
        if (turn) (turn as Extract<Row, { type: 'assistant' }>).durationMs = i.durationMs
        closeTurn()
        break
      case 'command': {
        const prev = rows[rows.length - 1]
        // a setting's own output ("Set model to …") goes with it
        if (!i.name && hidden) break
        hidden = SETTINGS.has(i.name.replace(/^\/+/, '').toLowerCase())
        if (hidden) break
        if (!i.name && prev?.type === 'command' && prev.output === undefined) {
          prev.output = i.output
          break
        }
        closeTurn()
        rows.push({ type: 'command', key: `c${i.seq}`, name: i.name, args: i.args, output: i.output })
        break
      }
      case 'goal':
        // keep it in order: replies after it start a new turn rather than joining the one above
        closeTurn()
        rows.push({ type: 'goal', key: `g${i.seq}`, item: i })
        break
      case 'notice':
        if (turn && i.subtype === 'interrupted') {
          ;(turn as Extract<Row, { type: 'assistant' }>).parts.push({ type: 'notice', key: i.uuid, item: i })
          closeTurn()
        } else if (turn && i.subtype === 'task') {
          // a background job finishing mid-turn doesn't end the turn
          ;(turn as Extract<Row, { type: 'assistant' }>).parts.push({ type: 'notice', key: `n${i.seq}`, item: i })
        } else {
          closeTurn()
          rows.push({ type: 'notice', key: `n${i.seq}`, item: i })
        }
        break
    }
  }
  return rows
}
