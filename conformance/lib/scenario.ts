import crypto from 'node:crypto'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { VERSION } from '@inyon/protocol'
import { Context } from './context.ts'
import { Hub, REPO, sha256 } from './hub.ts'
import { Machine } from './machine.ts'
import { FakeNode } from './node.ts'
import { Normalizer } from './normalize.ts'

export interface Scenario {
  /** the golden file's name */
  name: string
  /** what it covers, one line */
  title: string
  /** builds the fake machine (panes, records); the scenario then has a paired machine "box" */
  machine?: (m: Machine) => void | Promise<void>
  /** records to put in the hub's data folder before it starts */
  seed?: (dataDir: string) => void
  run(ctx: Context): Promise<void>
}

/** The machine's id and name on the hub. */
export const BOX = 'box'

/** What inyon on a machine says first on /node (crates/inyon-engine/src/connect.rs). */
export const HELLO = { type: 'hello', version: VERSION, os: 'linux x64', hostname: 'box', platform: 'linux', mux: 'tmux' }

export interface RunOptions {
  command?: string
  keep?: boolean
  log?: (line: string) => void
}

/** Runs one scenario against a fresh hub; returns its transcript as normalized JSON lines. */
export async function runScenario(s: Scenario, o: RunOptions = {}): Promise<string[]> {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'inyon-conformance-')))
  const norm = new Normalizer()
  norm.learn(root, 'tmp')
  // Claude names a folder's transcripts after its path with every other character a dash
  norm.learn(root.replace(/[^a-zA-Z0-9]/g, '-'), 'tmp-slug')
  // the hub names its own machine after the computer: a plain word, on some computers
  norm.learnExact(os.hostname(), 'hostname')
  // the checkout the hub runs from (the update check names it)
  norm.learn(REPO, 'repo')
  norm.learn(VERSION, 'version')
  let machine: Machine | undefined
  let node: FakeNode | undefined
  const token = crypto.randomBytes(32).toString('base64url')
  norm.learn(token, 'node-token')
  if (s.machine) {
    machine = new Machine(path.join(root, 'machine'))
    await machine.start()
    await s.machine(machine)
  }
  const hub = new Hub(root, {
    command: o.command,
    log: o.log,
    seed: (dir) => {
      if (machine) fs.writeFileSync(path.join(dir, 'hosts.json'), JSON.stringify([{ id: BOX, name: 'Box', kind: 'node', tokenHash: sha256(token), pairedAt: Date.now() }]))
      s.seed?.(dir)
    },
  })
  try {
    await hub.start()
    norm.learn(hub.base, 'hub')
    norm.learn(hub.base.replace(/^http:\/\//, ''), 'hub-address')
    // the hub's address inside a link's query string (the phone pairing QR)
    norm.learn(encodeURIComponent(hub.base), 'hub-encoded')
    if (machine) node = new FakeNode(machine, HELLO)
    const ctx = new Context(hub, norm, machine, node, token)
    await s.run(ctx)
    await ctx.end()
    return ctx.entries.map((e) => JSON.stringify(norm.value(e)))
  } catch (e) {
    throw new Error(`${s.name}: ${(e as Error).stack ?? e}\n--- hub output ---\n${hub.tail()}`)
  } finally {
    node?.close()
    await hub.stop()
    machine?.stop()
    if (!o.keep) fs.rmSync(root, { recursive: true, force: true })
  }
}
