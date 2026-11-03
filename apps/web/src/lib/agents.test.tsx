import { expect, test } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { screen } from '@testing-library/react'
import { AGENT_CATALOG, AGENTS, agentDef, knownAgent } from '@inyon/protocol'
import { AgentLogo } from './skin.tsx'
import { wsUrl } from './hub.ts'
import { hubWsUrl } from './hubs.ts'
import { startingNote } from './starting.ts'
import { renderSettled as render } from '../test/render.tsx'

test('an agent added after this app was built shows as a plain terminal agent under its own name', () => {
  const def = agentDef('newagent')
  expect(def.name).toBe('Newagent')
  expect(def.chat).toBe(false)
  expect(def.models).toEqual([])
  expect(def.skin.light.accent).toMatch(/^#/)
  // the same entry every time, so a screen doesn't redraw for a new object
  expect(agentDef('newagent')).toBe(def)
  expect(knownAgent('newagent')).toBe(false)
  expect(agentDef('claude')).toBe(AGENTS.claude)
})

test('an agent this app has no mark for gets the plain tile, not a broken picture', async () => {
  await render(<AgentLogo kind={'newagent' as never} size={24} />)
  expect(document.querySelector('img')).toBeNull()
  await render(<AgentLogo kind="pi" size={24} />)
  expect(screen.getByAltText('Pi').getAttribute('src')).toContain('agents/pi.svg')
})

test('every agent in the catalog has its mark', () => {
  for (const kind of Object.keys(AGENTS)) expect(statSync(join(import.meta.dir, `../../public/agents/${kind}.svg`)).size).toBeGreaterThan(0)
})

test('the app tells a hub which agents it knows, on both of its sockets', () => {
  expect(wsUrl()).toEndWith(`/ws?catalog=${AGENT_CATALOG}`)
  expect(hubWsUrl({ id: 'h', name: 'Hub', url: 'https://hub.example' } as never, 't')).toBe(`wss://hub.example/ws?ticket=t&catalog=${AGENT_CATALOG}`)
})

test('nothing looks an agent up in the catalog directly, so a newer kind never breaks a screen', () => {
  const offenders: string[] = []
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name)
      if (statSync(path).isDirectory()) walk(path)
      else if (/\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) && /\bAGENTS\[/.test(readFileSync(path, 'utf8'))) offenders.push(path)
    }
  }
  walk(join(import.meta.dir, '..'))
  expect(offenders).toEqual([])
})

test('starting an agent from its terminal says where it opens: the chat view, or this terminal for a terminal-first one', () => {
  expect(startingNote('claude')).toContain('chat view')
  expect(startingNote('pi')).toBe('It opens here in the terminal.')
  expect(startingNote('newagent')).toBe('It opens here in the terminal.')
})
