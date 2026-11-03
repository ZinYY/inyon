import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'
import { execSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { VERSION } from '@inyon/protocol'
import { browserFiles, withPrecacheList } from './src/lib/precache.ts'
import { pageview } from './src/pageview/build.ts'

const HUB = process.env.INYON_HUB ?? process.env.TETHER_HUB ?? 'http://127.0.0.1:4777'

/** the commit a build came from, shown next to the version in Settings */
const commit = () => {
  if (process.env.GITHUB_SHA) return process.env.GITHUB_SHA.slice(0, 7)
  try {
    return execSync('git rev-parse --short HEAD', { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim()
  } catch {
    return ''
  }
}

/**
 * The app carries the Rust phone core (crates/inyon-phone, written to src/phone-core by
 * scripts/phone-core.ts, which `bun run build` runs first). Only the Android app and the browser
 * copy load it. VITE_INYON_PHONE_CORE=none leaves it out, for the copy a hub serves (it never
 * loads it).
 */
const cores: Record<string, string> = { rust: './src/phone-core/index.ts', none: './src/lib/no-phone-core.ts' }
const coreKind = process.env.VITE_INYON_PHONE_CORE ?? 'rust'
if (!cores[coreKind]) throw new Error(`VITE_INYON_PHONE_CORE is rust or none, not ${coreKind}`)
const rustCore = coreKind === 'rust'
const phoneCore = fileURLToPath(new URL(cores[coreKind], import.meta.url))
if (rustCore && !existsSync(phoneCore)) throw new Error('The phone core is missing: run `bun scripts/phone-core.ts` (or `bun run build`) first')

/** A page compiles wasm only when its CSP allows it: the Rust core's builds add that to script-src. */
const wasmCsp: Plugin = {
  name: 'inyon-wasm-csp',
  transformIndexHtml: (html) => (rustCore ? html.replace("script-src 'self'", "script-src 'self' 'wasm-unsafe-eval'") : html),
}

/** The dev server's hot reload needs an inline script, which index.html's CSP refuses: drop it there. */
const devWithoutCsp: Plugin = {
  name: 'inyon-dev-without-csp',
  apply: 'serve',
  transformIndexHtml: (html) => html.replace(/<meta\s+http-equiv="Content-Security-Policy"[^>]*>/, ''),
}

/** What this build is, for the web app to compare with what its host serves (lib/updates.ts). */
const versionFile: Plugin = {
  name: 'inyon-version-file',
  apply: 'build',
  generateBundle() {
    this.emitFile({ type: 'asset', fileName: 'version.json', source: JSON.stringify({ version: VERSION, commit: commit() }) })
  },
}

/** The service worker gets this build's hashed files to cache at install (public/sw.js has an empty list). */
const precache: Plugin = {
  name: 'inyon-precache',
  apply: 'build',
  generateBundle(_, bundle) {
    const sw = readFileSync(fileURLToPath(new URL('./public/sw.js', import.meta.url)), 'utf8')
    this.emitFile({ type: 'asset', fileName: 'sw.js', source: withPrecacheList(sw, browserFiles(bundle)) })
  },
}

export default defineConfig({
  plugins: [react(), tailwindcss(), devWithoutCsp, wasmCsp, versionFile, precache, pageview()],
  define: { __INYON_COMMIT__: JSON.stringify(commit()) },
  // the phone app runs the hub's core in-process (loaded only inside the app, see lib/phone.ts)
  resolve: { alias: { '@inyon/phone': phoneCore } },
  server: {
    port: 5173,
    host: '127.0.0.1',
    proxy: {
      '/api': HUB,
      '/ws': { target: HUB.replace(/^http/, 'ws'), ws: true },
    },
  },
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 1500,
    // React and the router in a chunk of their own: its hash outlives app changes, so an update
    // fetches the app's code again but not React's
    rolldownOptions: { output: { codeSplitting: { groups: [{ name: 'react', test: /node_modules[\\/](react|react-dom|scheduler|react-router)[\\/]/ }] } } },
  },
})
