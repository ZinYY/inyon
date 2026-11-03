// The phone's side, in the browser: the Rust link compiled to wasm, against the Bun machine.
import init, { benchHandshakes, connect, identityId } from './pkg/s4_wasm.js'

const out = { checks: {} }
const check = (name, ok, detail) => (out.checks[name] = { ok: !!ok, ...(detail === undefined ? {} : { detail }) })

async function run() {
  const ms = (t) => +(performance.now() - t).toFixed(1)
  const t0 = performance.now()
  const bytes = await (await fetch('./pkg/s4_wasm_bg.wasm')).arrayBuffer()
  out.wasmFetchMs = ms(t0)
  // a second fetch: the first pays for the page's first request to the test server
  const t0b = performance.now()
  await (await fetch('./pkg/s4_wasm_bg.wasm')).arrayBuffer()
  out.wasmRefetchMs = ms(t0b)
  const tc = performance.now()
  await init({ module_or_path: bytes })
  out.wasmCompileAndStartMs = ms(tc)
  const tj = performance.now()
  const ts = await import('./pkg/ts-phone.js')
  out.tsBundleLoadMs = ms(tj)

  const cfg = await (await fetch('/config.json')).json()
  const seed = new Uint8Array(cfg.phoneSeed)
  const phone = identityId(seed)
  const url = `ws://${location.host}/link`

  const t1 = performance.now()
  const link = await connect(url, cfg.machineId, seed, JSON.stringify({ phone, name: 'spike' }))
  out.handshakeMs = +(performance.now() - t1).toFixed(1)
  const welcome = JSON.parse(link.welcome)
  check('welcome from the TypeScript machine', welcome.ok && welcome.from === 'bun' && welcome.machine === cfg.machineId, welcome)

  const queue = []
  let waiting
  link.setOnMessage((json) => {
    const m = JSON.parse(json)
    if (waiting) {
      const w = waiting
      waiting = undefined
      w(m)
    } else queue.push(m)
  })
  const next = () => (queue.length ? Promise.resolve(queue.shift()) : new Promise((r) => (waiting = r)))
  const ask = (m) => (link.send(JSON.stringify(m)), next())

  // the phone signs the handshake hash with its ed25519 key; the machine checks it with the id
  const proof = await ask({ t: 'proof', sig: [...link.signBinding()] })
  const binding = [...link.binding].map((b) => b.toString(16).padStart(2, '0')).join('')
  check('ed25519 signature over the binding verifies in TS', proof.ok)
  check('handshake hash is the same on both ends', proof.binding === binding)

  const t2 = performance.now()
  for (let i = 0; i < 200; i++) {
    const r = await ask({ t: 'echo', i })
    if (r.i !== i) throw new Error('echo out of order')
  }
  out.echoRoundTripMs = +((performance.now() - t2) / 200).toFixed(3)

  // the same handshake from the TypeScript link, for comparison
  const tt = performance.now()
  const tsLink = await ts.tsConnect(url, cfg.machineId, seed, { phone, name: 'ts' })
  out.tsHandshakeMs = ms(tt)
  tsLink.channel.close()

  // crypto alone: 50 handshakes + 64 KB each way, in wasm and in TypeScript
  benchHandshakes(2)
  ts.tsBench(2)
  const tb = performance.now()
  check('wasm bench handshakes', benchHandshakes(50) === 50)
  out.wasmHandshakeAnd64KbMs = +((performance.now() - tb) / 50).toFixed(2)
  const tb2 = performance.now()
  check('TS bench handshakes', ts.tsBench(50) === 50)
  out.tsHandshakeAnd64KbMs = +((performance.now() - tb2) / 50).toFixed(2)

  const text = 'ø→🙂'.repeat(50000)
  const t3 = performance.now()
  const echoed = await ask({ t: 'echo', text })
  out.bigEchoMs = +(performance.now() - t3).toFixed(1)
  check('450 KB of multi-byte text survives the chunking both ways', echoed.text === text, { bytes: new TextEncoder().encode(text).length })

  const big = await ask({ t: 'big', n: 100000 })
  check('a chunked message from TS splits inside a character and joins', big.data.length === 100000 && big.data === 'é'.repeat(100000))
  link.close()

  // the wrong machine key: the TS machine can't read message 1 and hangs up
  try {
    await connect(url, cfg.strangerId, seed, JSON.stringify({ phone }))
    check('a wrong machine key is refused', false)
  } catch (e) {
    check('a wrong machine key is refused', true, String(e.message ?? e))
  }
  // a hello that names another phone: refused by the TS check, told why, encrypted
  try {
    await connect(url, cfg.machineId, seed, JSON.stringify({ phone: cfg.strangerId }))
    check('a key that is not the named phone is refused', false)
  } catch (e) {
    check('a key that is not the named phone is refused', /belong/.test(String(e.message ?? e)), String(e.message ?? e))
  }
}

run()
  .catch((e) => (out.error = String(e?.stack ?? e)))
  .finally(() => {
    out.ok = !out.error && Object.values(out.checks).every((c) => c.ok)
    document.getElementById('out').textContent = JSON.stringify(out, null, 2)
    window.s4 = out
  })
