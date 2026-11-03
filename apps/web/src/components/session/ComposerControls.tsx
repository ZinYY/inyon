import { useEffect, useState } from 'react'
import clsx from 'clsx'
import { AlertCircle, ChevronDown, Clock3, Gauge, Plug, Shield, Target } from 'lucide-react'
import { agentDef, type AgentKind, type AgentDef, type ChosenControls } from '@inyon/protocol'
import type { SessionModel } from './useSession.ts'
import { currentModelId } from './useSession.ts'
import { modelName } from '../../lib/format.ts'
import { Button, Field, Menu, Sheet, inputClass, type MenuItem } from '../ui/primitives.tsx'
import { ProvidersSheet } from '../ProvidersSheet.tsx'

export const EFFORT_LABEL: Record<string, string> = { default: 'Default', none: 'None', minimal: 'Minimal', low: 'Low', medium: 'Medium', high: 'High', xhigh: 'X-high', max: 'Max', ultra: 'Ultra' }

function Chip({ children, onClick, title, tone, open, innerRef }: { children: React.ReactNode; onClick?: () => void; title?: string; tone?: 'goal'; open?: boolean; innerRef?: React.Ref<HTMLButtonElement> }) {
  return (
    <button
      ref={innerRef}
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      onMouseDown={(e) => e.preventDefault()}
      className={clsx(
        'relative inline-flex h-8 max-w-[220px] shrink-0 items-center gap-1.5 rounded-full px-2.5 font-mono text-[12px] transition-colors after:absolute after:-inset-1',
        tone === 'goal' ? 'text-accent-text hover:bg-accent-soft' : 'text-fg-2 hover:bg-hover hover:text-fg',
        open && 'bg-hover text-fg',
      )}
    >
      {children}
    </button>
  )
}

export function EffortBars({ level, levels }: { level?: string; levels: string[] }) {
  const i = level ? levels.indexOf(level) : -1
  return (
    <span className="flex items-end gap-[2px]" aria-hidden>
      {[0, 1, 2, 3].map((n) => (
        <span key={n} className={clsx('w-[3px] rounded-[1px]', n <= Math.round((i / Math.max(1, levels.length - 1)) * 3) && i >= 0 ? 'bg-accent' : 'bg-line-strong')} style={{ height: 5 + n * 2.5 }} />
      ))}
    </span>
  )
}

/** The model a pick held for later names: its id, or the agent's own `/model` value for it. */
export function chosenModel(def: Pick<AgentDef, 'models' | 'set'>, value: string, effort?: string) {
  return def.models.find((m) => m.id === value || def.set.model?.(m.id, effort) === `/model ${value}`)
}

/** What a control's chip and menu show for a pick held until the agent is free (see the hub's
 * chosen.rs): the picked value marked as waiting, or why it didn't take. */
function waiting(c: ChosenControls | undefined, picked: string | undefined, label: string | undefined, agentName: string, current: string, drop: () => void) {
  if (picked === undefined || !label) return undefined
  if (c?.error) {
    return {
      mark: <AlertCircle className="size-3.5 shrink-0 text-danger" aria-label="Didn't switch" />,
      title: `Couldn't switch to ${label}: ${c.error}`,
      items: [{ key: 'chosen', separated: true, label: `Couldn't switch to ${label}`, hint: c.error, icon: <AlertCircle className="size-4" />, onSelect: drop } satisfies MenuItem],
    }
  }
  return {
    mark: <Clock3 className="size-3 shrink-0 opacity-70" aria-label="Waiting" />,
    title: `Switches to ${label} when ${agentName} is free`,
    items: [{ key: 'chosen', separated: true, label: "Don't switch", hint: `Keeps ${current}`, onSelect: drop } satisfies MenuItem],
  }
}

/** Model, effort, permission mode and goal, in the composer where you use them. */
export function ComposerControls({ s }: { s: SessionModel }) {
  const [goalOpen, setGoalOpen] = useState(false)
  const [providers, setProviders] = useState(false)
  const { def, status, info, agent, actions } = s
  // "/connect" typed in the box opens the providers sheet, as "/models" opens the model menu
  useEffect(() => {
    if (agent !== 'opencode') return
    const onOpen = (e: Event) => {
      if ((e as CustomEvent<string>).detail !== 'providers') return
      e.preventDefault()
      setTimeout(() => setProviders(true))
    }
    window.addEventListener('inyon:open-menu', onOpen)
    return () => window.removeEventListener('inyon:open-menu', onOpen)
  }, [agent])
  if (!agent || !def) return null
  const modelId = currentModelId(def, status, info?.model)
  // a catalog id (OpenCode's "provider/model") reads better as its label, without the provider prefix
  const listed = def.models.find((m) => m.id === modelId)?.label.split(' · ').pop()
  const modelLabel = listed ?? status?.model ?? (info?.model ? modelName(info.model) : 'Default model')
  const mode = s.screen?.mode ?? s.ref?.pane.agent?.mode ?? info?.mode
  // picks waiting for the agent to be free, or that didn't take
  const chosen = info?.chosen
  const name = agentDef(agent).name
  const effortLabel = status?.effort ? EFFORT_LABEL[status.effort] ?? status.effort : 'Default'
  const pickedModel = chosen?.model !== undefined ? chosenModel(def, chosen.model, status?.effort) : undefined
  const wModel = waiting(chosen, chosen?.model, pickedModel?.label.split(' · ').pop() ?? chosen?.model, name, modelLabel, actions.dropChosen)
  const wEffort = waiting(chosen, chosen?.effort, chosen?.effort && (EFFORT_LABEL[chosen.effort] ?? chosen.effort), name, effortLabel, actions.dropChosen)
  const wMode = waiting(chosen, chosen?.mode, chosen?.mode && modeLabel(chosen.mode, agent, def.modes), name, modeLabel(mode, agent, def.modes), actions.dropChosen)
  const pendingModel = !chosen?.error ? pickedModel?.id : undefined
  const pendingEffort = !chosen?.error ? chosen?.effort : undefined
  const pendingMode = !chosen?.error ? chosen?.mode : undefined
  const goal = info?.goal
  const goalOn = !!goal && !goal.met
  const modelItems: MenuItem[] = [
    ...def.models.map((m) => ({ key: m.id, label: m.label, hint: m.note, selected: m.id === (pendingModel ?? modelId), onSelect: () => actions.setModel(m.id) })),
    // OpenCode's list is what its connected providers offer; one more provider adds to it
    ...(agent === 'opencode' ? [{ key: 'connect', label: 'Connect a provider…', icon: <Plug className="size-4" />, separated: def.models.length > 0, onSelect: () => setProviders(true) }] : []),
  ]
  return (
    <>
      {modelItems.length > 0 && (
        <Menu
          name="model"
          header="Model"
          items={[...modelItems, ...(wModel?.items ?? [])]}
          trigger={({ toggle, ref, open }) => (
            <Chip innerRef={ref} onClick={toggle} open={open} title={wModel?.title ?? 'Model'}>
              {wModel?.mark}
              <span className="truncate">{wModel && !chosen?.error ? (pickedModel?.label.split(' · ').pop() ?? chosen?.model) : modelLabel}</span>
              <ChevronDown className="size-3 shrink-0 opacity-60" />
            </Chip>
          )}
        />
      )}
      {s.efforts.length > 0 && (
        <Menu
          name="effort"
          header="Reasoning effort"
          width={260}
          items={[
            // OpenCode runs a model without a variant unless one is picked
            ...(agent === 'opencode' ? [{ key: 'default', label: 'Default', selected: !status?.effort, onSelect: () => actions.setEffort('default') }] : []),
            ...s.efforts.map((e) => ({ key: e, label: EFFORT_LABEL[e] ?? e, selected: (pendingEffort ?? status?.effort) === e, onSelect: () => actions.setEffort(e) })),
            // Claude Code keeps its ultracode switch next to the levels; so does this menu
            ...(def.set.ultracode
              ? [{ key: 'ultracode', separated: true, label: `Ultracode: ${s.ultracode ? 'on' : 'off'}`, hint: 'A dynamic workflow for every task', icon: <Gauge className="size-4" />, selected: !!s.ultracode, onSelect: () => actions.ultracode(!s.ultracode) }]
              : []),
            ...(wEffort?.items ?? []),
          ]}
          trigger={({ toggle, ref, open }) => (
            <Chip innerRef={ref} onClick={toggle} open={open} title={wEffort?.title ?? `Reasoning effort${status?.effort ? `: ${effortLabel}` : ''}${s.ultracode ? ' · Ultracode on' : ''}`}>
              {wEffort?.mark}
              <EffortBars level={pendingEffort ?? status?.effort} levels={s.efforts} />
              <span>{pendingEffort ? EFFORT_LABEL[pendingEffort] ?? pendingEffort : effortLabel}</span>
              {s.ultracode && <Gauge className="ultracode-mark size-3.5 shrink-0" aria-label="Ultracode on" />}
            </Chip>
          )}
        />
      )}
      {def.modes.length > 0 && (
        <Menu
          name="mode"
          header={agent === 'opencode' ? 'Agent' : 'Permission mode'}
          width={220}
          items={[
            ...def.modes.map((m) => ({ key: m.id, label: m.label, selected: pendingMode ? pendingMode === m.id : modeMatches(mode, m.id, agent), onSelect: () => actions.setMode(m.id) })),
            ...(wMode?.items ?? []),
          ]}
          trigger={({ toggle, ref, open }) => (
            <Chip innerRef={ref} onClick={toggle} open={open} title={wMode?.title ?? `${agent === 'opencode' ? 'Agent' : 'Permission mode'}: ${modeLabel(mode, agent, def.modes)}`}>
              {wMode?.mark ?? <Shield className="size-3.5 shrink-0" />}
              <span className="hidden truncate @[460px]:inline">{modeLabel(pendingMode ?? mode, agent, def.modes)}</span>
            </Chip>
          )}
        />
      )}
      {def.set.goal && (
        <Chip tone={goalOn ? 'goal' : undefined} onClick={() => setGoalOpen(true)} title={goalOn ? `Goal: ${goal!.condition}` : 'Set a goal'}>
          <Target className="size-3.5 shrink-0" />
          {goalOn ? <span className="max-w-[140px] truncate">{goal!.condition}</span> : <span className="hidden @[460px]:inline">Goal</span>}
        </Chip>
      )}
      <GoalSheet key={String(goalOpen)} s={s} open={goalOpen} onClose={() => setGoalOpen(false)} />
      {agent === 'opencode' && s.ref && (
        <ProvidersSheet hostId={s.ref.host.config.id} hostName={s.ref.host.config.name} pane={s.ref.pane.id} open={providers} onClose={() => setProviders(false)} />
      )}
    </>
  )
}

export function GoalSheet({ s, open, onClose }: { s: SessionModel; open: boolean; onClose: () => void }) {
  const goal = s.info?.goal
  const active = !!goal && !goal.met
  const [text, setText] = useState(goal?.condition ?? '')
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Goal"
      icon={<Target className="size-4 text-fg-3" />}
      footer={
        <div className="flex items-center gap-2">
          {active && s.def?.set.clearGoal && (
            <Button variant="danger" onClick={() => (s.actions.clearGoal(), onClose())}>
              Turn off
            </Button>
          )}
          <Button className="ml-auto" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="accent" disabled={!text.trim()} onClick={() => (s.actions.setGoal(text.trim()), onClose())}>
            {active ? 'Update goal' : 'Set goal'}
          </Button>
        </div>
      }
    >
      <p className="mb-4 text-[13.5px] leading-relaxed text-fg-2">The agent checks this condition before it stops, and keeps going until it's met.</p>
      {goal && (
        <div className={clsx('mb-4 rounded-2xl border px-4 py-3 text-[13px]', goal.met ? 'border-line-strong bg-sunken' : 'border-line bg-sunken')}>
          <div className="font-medium">{goal.met ? 'Last goal was met' : 'Active goal'}</div>
          <div className="mt-0.5 line-clamp-3 text-fg-2">{goal.condition}</div>
          {goal.met && goal.reason && <div className="mt-1 line-clamp-2 text-[12px] text-fg-3">{goal.reason}</div>}
        </div>
      )}
      <Field label={active ? 'Change the goal' : 'New goal'}>
        <textarea rows={3} value={text} onChange={(e) => setText(e.target.value)} placeholder="All tests pass and the PR is open" className={clsx(inputClass, 'h-auto resize-none py-2.5 leading-relaxed')} />
      </Field>
    </Sheet>
  )
}

/** Codex names its modes itself (permission presets); the others share Claude's names. */
const OWN_MODES = new Set(['codex', 'copilot', 'opencode'])

/** `modes` is the agent's own list when the machine gave one (OpenCode's agents, custom ones too). */
export function modeLabel(mode?: string, agent?: AgentKind, modes?: { id: string; label: string }[]) {
  if (agent && OWN_MODES.has(agent)) {
    const list = modes ?? agentDef(agent).modes
    const id = mode || 'default'
    return list.find((m) => m.id === id)?.label ?? (agent === 'opencode' && mode ? mode.charAt(0).toUpperCase() + mode.slice(1) : mode ?? 'Default')
  }
  if (!mode || /^(manual|default|normal)$/.test(mode)) return 'Ask each time'
  if (/accept/.test(mode)) return 'Accept edits'
  if (/plan/.test(mode)) return 'Plan'
  if (/bypass/.test(mode)) return 'Bypass permissions'
  if (/auto/.test(mode)) return 'Auto'
  return mode
}

export function modeMatches(mode: string | undefined, id: string, agent?: AgentKind) {
  if (agent && OWN_MODES.has(agent)) return (mode || 'default') === id
  const l = modeLabel(mode)
  return (
    (id === 'default' && l === 'Ask each time') ||
    (id === 'acceptEdits' && l === 'Accept edits') ||
    (id === 'plan' && l === 'Plan') ||
    (id === 'auto' && l === 'Auto') ||
    (id === 'bypassPermissions' && l === 'Bypass permissions')
  )
}
