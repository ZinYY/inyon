import { expect, test } from 'bun:test'
import { browserFiles, withPrecacheList, type BuiltFile } from './precache.ts'

test('only the hashed files under assets/ go in the list, sorted, and the rest of the worker is untouched', () => {
  const sw = "const CACHE = 'x'\nconst ASSETS = []\nconst after = ASSETS.length\n"
  const out = withPrecacheList(sw, ['index.html', 'version.json', 'assets/z.js', 'assets/a.woff2', 'sw.js'])
  expect(out).toBe("const CACHE = 'x'\nconst ASSETS = [\"assets/a.woff2\",\"assets/z.js\"]\nconst after = ASSETS.length\n")
})

test('the chunks a browser can reach are listed; the phone app\'s in-process hub and its own imports are not', () => {
  const chunk = (fileName: string, o: Partial<BuiltFile> = {}): BuiltFile => ({ type: 'chunk', fileName, imports: [], dynamicImports: [], ...o })
  const bundle: Record<string, BuiltFile> = {
    'assets/index-1.js': chunk('assets/index-1.js', { isEntry: true, imports: ['assets/react-1.js', 'assets/core-1.js'], dynamicImports: ['assets/phone-1.js', 'assets/Messages-1.js'] }),
    'assets/react-1.js': chunk('assets/react-1.js'),
    // shared with the hub: still needed by the page
    'assets/core-1.js': chunk('assets/core-1.js'),
    'assets/Messages-1.js': chunk('assets/Messages-1.js', { dynamicImports: ['assets/rust-1.js'] }),
    'assets/rust-1.js': chunk('assets/rust-1.js'),
    'assets/phone-1.js': chunk('assets/phone-1.js', { facadeModuleId: '/repo/apps/web/src/phone-core/index.ts', imports: ['assets/registry-1.js', 'assets/core-1.js'] }),
    'assets/registry-1.js': chunk('assets/registry-1.js'),
    'assets/index-1.css': { type: 'asset', fileName: 'assets/index-1.css' },
    'version.json': { type: 'asset', fileName: 'version.json' },
  }
  expect(browserFiles(bundle).sort()).toEqual(['assets/Messages-1.js', 'assets/core-1.js', 'assets/index-1.css', 'assets/index-1.js', 'assets/react-1.js', 'assets/rust-1.js', 'version.json'])
})

test('the Rust phone core and its wasm stay out of the list too', () => {
  const chunk = (fileName: string, o: Partial<BuiltFile> = {}): BuiltFile => ({ type: 'chunk', fileName, imports: [], dynamicImports: [], ...o })
  const bundle: Record<string, BuiltFile> = {
    'assets/index-1.js': chunk('assets/index-1.js', { isEntry: true, dynamicImports: ['assets/phone-core-1.js'] }),
    'assets/phone-core-1.js': chunk('assets/phone-core-1.js', { facadeModuleId: '/repo/apps/web/src/phone-core/index.ts' }),
    'assets/inyon_phone_bg-BW5_vaB-.wasm': { type: 'asset', fileName: 'assets/inyon_phone_bg-BW5_vaB-.wasm' },
    'assets/index-1.css': { type: 'asset', fileName: 'assets/index-1.css' },
  }
  expect(browserFiles(bundle).sort()).toEqual(['assets/index-1.css', 'assets/index-1.js'])
})
