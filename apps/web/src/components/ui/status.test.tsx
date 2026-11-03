import { expect, test } from 'bun:test'
import { render } from '@testing-library/react'
import { AGENTS } from '@inyon/protocol'
import { AgentSpinner, StateDot, StatePill, backgroundText, failureText, shownState } from './status.tsx'

test('the spinner steps through its frames in CSS, so it never re-renders to animate', () => {
  const { container, rerender } = render(<AgentSpinner kind="claude" active />)
  const strip = container.querySelector('span > span') as HTMLElement
  expect(strip.children).toHaveLength(AGENTS.claude.spinner.length)
  expect(strip.style.animation).toContain(`steps(${AGENTS.claude.spinner.length})`)
  rerender(<AgentSpinner kind="claude" />)
  expect((container.querySelector('span > span') as HTMLElement).style.animation).toBe('')
})

test('an idle agent with work left running shows as Background, with a dot that moves and is not the idle grey', () => {
  expect(shownState({ state: 'idle', background: '1 agent' })).toBe('background')
  expect(shownState({ state: 'idle' })).toBe('idle')
  expect(shownState({ state: 'working', background: '1 agent' })).toBe('working')
  const dot = (state: Parameters<typeof StateDot>[0]['state']) => render(<StateDot state={state} />).container.firstElementChild!.className
  expect(dot('background')).toContain('pulse-soft')
  expect(dot('background')).not.toContain('bg-fg-3')
  expect(dot('idle')).not.toContain('pulse-soft')
})

test('a state chip changes colour in place and brings in the new word with a fade', () => {
  const { container, rerender } = render(<StatePill state="working" />)
  const pill = container.firstElementChild as HTMLElement
  const word = () => pill.lastElementChild as HTMLElement
  expect(pill.className).toContain('transition-colors')
  expect(pill.firstElementChild!.className).toContain('transition-colors')
  const was = word()
  rerender(<StatePill state="waiting" />)
  expect(container.firstElementChild).toBe(pill)
  expect(word()).not.toBe(was)
  expect(word().textContent).toBe('Needs you')
  expect(word().className).toContain('animate-fade-in')
})

test('background work reads as a sentence, alone or as a list', () => {
  expect(backgroundText('1 shell')).toBe('1 shell running in the background')
  expect(backgroundText('1 background task')).toBe('1 task running in the background')
  expect(backgroundText('2 agents')).toBe('2 agents running in the background')
  expect(backgroundText('1 shell, 1 agent')).toBe('1 shell and 1 agent running in the background')
  expect(backgroundText('1 shell, 2 monitors, 1 agent')).toBe('1 shell, 2 monitors and 1 agent running in the background')
})

test('a turn that stopped on an error says why, and when a full rate limit resets in local time', () => {
  const now = new Date(2026, 9, 24, 12, 0).getTime()
  const at = (h: number) => Math.round(now / 1000 + h * 3600)
  const time = (h: number) => new Date(at(h) * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  expect(failureText({ error: 'authentication_failed' }, now)).toBe('Stopped: signed out')
  expect(failureText({ error: 'something_new' }, now)).toBe('Stopped: an error')
  expect(failureText({ error: 'rate_limit', resetsAt: at(2) }, now)).toBe(`Stopped: rate limit · resets ${time(2)}`)
  // days away: the day too
  const day = new Date(at(50) * 1000).toLocaleDateString([], { weekday: 'short' })
  expect(failureText({ error: 'rate_limit', resetsAt: at(50) }, now)).toBe(`Stopped: rate limit · resets ${day} ${time(50)}`)
  // a reset that has passed isn't shown
  expect(failureText({ error: 'rate_limit', resetsAt: at(-1) }, now)).toBe('Stopped: rate limit')
})
