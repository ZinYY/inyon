// bun scripts/phone-core.ts [--debug]: builds the Rust phone core (crates/inyon-phone) as wasm and
// writes it with its JavaScript side to src/phone-core/, where the app's build picks it up as
// @inyon/phone (vite.config.ts). `bun run build` runs it first. Needs the wasm32-unknown-unknown
// target and the wasm-bindgen program at the version crates/inyon-phone pins
// (`cargo install wasm-bindgen-cli`).
import { $ } from 'bun'
import { gzipSync } from 'node:zlib'
import { join } from 'node:path'

// a build without the Rust core needs none of this (vite.config.ts)
if ((process.env.VITE_INYON_PHONE_CORE ?? 'rust') !== 'rust') process.exit(0)

const root = join(import.meta.dir, '../../..')
const out = join(import.meta.dir, '../src/phone-core')
const debug = process.argv.includes('--debug')
const profile = debug ? 'debug' : 'release'

const pinned = /wasm-bindgen = "=([\d.]+)"/.exec(await Bun.file(join(root, 'crates/inyon-phone/Cargo.toml')).text())?.[1]
const have = (await $`wasm-bindgen --version`.nothrow().quiet().text()).trim().split(' ')[1]
if (!have) throw new Error(`wasm-bindgen isn't installed: cargo install wasm-bindgen-cli --version ${pinned} --locked`)
if (have !== pinned) throw new Error(`wasm-bindgen ${have} is installed, the crate needs ${pinned}: cargo install wasm-bindgen-cli --version ${pinned} --locked`)

await $`cargo build ${debug ? [] : ['--release']} --target wasm32-unknown-unknown -p inyon-phone`.cwd(root)
const target = process.env.CARGO_TARGET_DIR ?? join(root, 'target')
await $`rm -rf ${out}`
await $`wasm-bindgen --target web --out-dir ${out} --out-name inyon_phone ${join(target, 'wasm32-unknown-unknown', profile, 'inyon_phone.wasm')}`
// what the app imports as @inyon/phone in such a build: the module, loaded on first use
await Bun.write(
  join(out, 'index.ts'),
  `// Written by scripts/phone-core.ts: the Rust phone core as @inyon/phone (see vite.config.ts).
import init, { startPhone as start } from './inyon_phone.js'
import type { StartPhone } from '../lib/phone-types.ts'

export const startPhone: StartPhone = async (o) => {
  await init()
  return (await start(o)) as never
}
`,
)
const wasm = await Bun.file(join(out, 'inyon_phone_bg.wasm')).bytes()
console.log(`phone core: ${(wasm.length / 1e6).toFixed(2)} MB, ${(gzipSync(wasm, { level: 9 }).length / 1e6).toFixed(2)} MB gzipped`)
