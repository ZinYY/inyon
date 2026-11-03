import type { ServerMsg } from '@inyon/protocol'

/**
 * What the phone app gives the core (crates/inyon-phone, @inyon/phone in vite.config.ts), and
 * what it gets back.
 */

export interface PhoneStore {
  get<T>(key: string): T | undefined
  set(key: string, value: unknown): void
  del(key: string): void
  keys(prefix: string): string[]
}

interface Listener {
  remove(): Promise<void> | void
}

/** The native InyonSsh plugin (apps/mobile/android/.../ssh/InyonSshPlugin.java). */
export interface SshPlugin {
  probe(o: { host: string; port: number }): Promise<{ fingerprint: string; keyType: string }>
  connect(o: {
    id: string
    host: string
    port: number
    username: string
    hostKey: string
    password?: string
    privateKey?: string
    passphrase?: string
    ed25519Seed?: string
    ed25519Public?: string
  }): Promise<unknown>
  disconnect(o: { id: string }): Promise<unknown>
  /** stdin is base64 */
  exec(o: { id: string; command: string; stdin?: string; timeoutMs?: number }): Promise<{ code: number; stdout: string; stderr: string }>
  stream(o: { id: string; streamId: string; command: string; keepStdin?: boolean }): Promise<unknown>
  streamKill(o: { streamId: string }): Promise<unknown>
  pty(o: { id: string; ptyId: string; command: string; cols: number; rows: number }): Promise<unknown>
  ptyWrite(o: { ptyId: string; data: string }): Promise<unknown>
  ptyResize(o: { ptyId: string; cols: number; rows: number }): Promise<unknown>
  ptyKill(o: { ptyId: string }): Promise<unknown>
  setSecret(o: { key: string; value: string }): Promise<unknown>
  getSecret(o: { key: string }): Promise<{ value: string | null }>
  deleteSecret(o: { key: string }): Promise<unknown>
  addListener(event: 'streamLines', fn: (e: { streamId: string; lines: string[] }) => void): Promise<Listener> | Listener
  addListener(event: 'streamExit', fn: (e: { streamId: string; code: number }) => void): Promise<Listener> | Listener
  addListener(event: 'ptyData', fn: (e: { ptyId: string; data: string }) => void): Promise<Listener> | Listener
  addListener(event: 'ptyExit', fn: (e: { ptyId: string; code: number }) => void): Promise<Listener> | Listener
  addListener(event: 'closed', fn: (e: { id: string; reason?: string }) => void): Promise<Listener> | Listener
}

/** A connected screen of the app, the in-process stand-in for the hub's WebSocket. */
export interface PhoneConnection {
  message(raw: string): void
  close(): void
}

export interface PhoneRuntime {
  /** Answers an /api request the way the hub would. */
  fetch(req: Request): Promise<Response>
  connect(send: (msg: ServerMsg) => void): PhoneConnection
  /** The app is back in the foreground: check connections now, don't wait out a backoff. */
  resume(): void
  /** The app went to the background (true) or came back (false): poll less while hidden. */
  background(hidden: boolean): void
}

/** `name`: what a paired computer lists this phone as ("Nothing A142"); its model from the WebView otherwise. */
export type StartPhone = (o: { store: PhoneStore; ssh: SshPlugin; name?: string }) => Promise<PhoneRuntime>

/** The web app imports the runtime as `@inyon/phone` (a Vite alias to ./index.ts) and types it from here. */
export declare const startPhone: StartPhone
