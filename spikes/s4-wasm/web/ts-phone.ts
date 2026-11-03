// The same phone side in TypeScript (packages/link, as the React app runs it today), bundled
// for the browser so the page can time both.
import { connect, identityFromSeed, Initiator, noiseKeyOf, Responder, wsWire } from '../../../packages/link/src/index.ts'

const enc = new TextEncoder()

export async function tsConnect(url: string, machineId: string, seed: Uint8Array, hello: unknown) {
  const id = identityFromSeed(seed)
  return connect(wsWire(new WebSocket(url)), { noise: id.noise, machineKey: noiseKeyOf(machineId), hello })
}

/** n in-memory handshakes plus a 64 KB message each way, like benchHandshakes in Rust. */
export function tsBench(n: number) {
  const phone = identityFromSeed(new Uint8Array(32).fill(1))
  const machine = identityFromSeed(new Uint8Array(32).fill(2))
  const body = enc.encode(JSON.stringify({ data: 'x'.repeat(64 * 1024) }))
  let ok = 0
  for (let k = 0; k < n; k++) {
    const i = new Initiator(phone.noise, machine.noise.publicKey, enc.encode('tether-link 1'))
    const r = new Responder(machine.noise, enc.encode('tether-link 1'))
    r.readMessage1(i.writeMessage1(enc.encode('{}')))
    const { message, transport: rt } = r.writeMessage2(enc.encode('{"ok":true}'))
    const { transport: it } = i.readMessage2(message)
    let got = 0
    for (let at = 0; at < body.length; at += 60000) {
      const piece = body.subarray(at, at + 60000)
      got += rt.recv.decrypt(new Uint8Array(0), it.send.encrypt(new Uint8Array(0), piece)).length
    }
    if (got === body.length) ok++
  }
  return ok
}
