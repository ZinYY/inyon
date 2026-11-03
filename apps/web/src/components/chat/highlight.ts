import hljs from 'highlight.js/lib/core'
import type { LanguageFn } from 'highlight.js'

type Loader = () => Promise<{ default: LanguageFn }>

/** The languages agents print most. Each is fetched the first time a block in it shows; others stay plain. */
const LANGUAGES: Record<string, Loader> = {
  bash: () => import('highlight.js/lib/languages/bash'),
  css: () => import('highlight.js/lib/languages/css'),
  diff: () => import('highlight.js/lib/languages/diff'),
  go: () => import('highlight.js/lib/languages/go'),
  java: () => import('highlight.js/lib/languages/java'),
  javascript: () => import('highlight.js/lib/languages/javascript'),
  json: () => import('highlight.js/lib/languages/json'),
  kotlin: () => import('highlight.js/lib/languages/kotlin'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
  python: () => import('highlight.js/lib/languages/python'),
  rust: () => import('highlight.js/lib/languages/rust'),
  shell: () => import('highlight.js/lib/languages/shell'),
  sql: () => import('highlight.js/lib/languages/sql'),
  swift: () => import('highlight.js/lib/languages/swift'),
  typescript: () => import('highlight.js/lib/languages/typescript'),
  xml: () => import('highlight.js/lib/languages/xml'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
}

const ALIASES: Record<string, string> = {
  sh: 'bash', zsh: 'bash', console: 'shell',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', ts: 'typescript', tsx: 'typescript',
  html: 'xml', svg: 'xml', yml: 'yaml', py: 'python',
}

/** The language a markdown code block names, as highlight.js knows it; undefined when it names none. */
export function languageOf(className?: string): string | undefined {
  const name = className?.match(/language-([\w+#-]+)/)?.[1]?.toLowerCase()
  return name ? ALIASES[name] ?? name : undefined
}

export const isLoaded = (lang: string) => !!hljs.getLanguage(lang)

const loading = new Map<string, Promise<boolean>>()

/** Resolves true once the language is registered, false when it is not one we ship (or didn't arrive). */
export function loadLanguage(lang: string): Promise<boolean> {
  if (isLoaded(lang)) return Promise.resolve(true)
  const load = LANGUAGES[lang]
  if (!load) return Promise.resolve(false)
  let pending = loading.get(lang)
  if (!pending) {
    pending = load().then(
      (m) => {
        hljs.registerLanguage(lang, m.default)
        return true
      },
      () => {
        // offline, most likely: the next block in this language asks again
        loading.delete(lang)
        return false
      },
    )
    loading.set(lang, pending)
  }
  return pending
}

/** Highlighted markup for code in a loaded language: escaped text in `hljs-*` spans. */
export const highlight = (lang: string, code: string) => hljs.highlight(code, { language: lang }).value
