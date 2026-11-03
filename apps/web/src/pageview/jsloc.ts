/**
 * A page's scripts read and set `location` (Blueprint's screens go to the next one with
 * `location.href = 'list.html' + location.search`). In the viewer every document is an
 * about:srcdoc, and a browser lets nothing stand in for `window.location`, so the names are
 * rewritten in the script's text to the shim's stand-in, which knows the page's own address (see
 * shim.ts). This is a tokenizer, not a parser: it skips strings, template text, comments and
 * regular expressions, and rewrites `location` used as a name, and `window.location`,
 * `self.location`, `globalThis.location` and `document.location`. Anything else that happens to be
 * called location (`a.location`, `{ location: 1 }`, `let location`) is left alone.
 */

/** What `location` becomes. The shim defines it on the window and on the document. */
export const LOCATION = '__inyonLocation'

/** The names whose `.location` is the document's address. */
const OWNERS = new Set(['window', 'self', 'globalThis', 'document'])

/** Words after which a slash starts a regular expression rather than dividing. */
const BEFORE_REGEX = new Set(['return', 'typeof', 'instanceof', 'in', 'of', 'new', 'delete', 'void', 'throw', 'case', 'do', 'else', 'yield', 'await'])

/** Words after which `location` is being declared, not read. */
const DECLARES = new Set(['var', 'let', 'const', 'function', 'class'])

type Kind = 'id' | 'num' | 'str' | 'regex' | 'punc'
interface Token {
  kind: Kind
  text: string
}

const isSpace = (c: string) => c === ' ' || c === '\n' || c === '\t' || c === '\r' || c === '\f' || c === '\v' || c === ' ' || c === ' ' || c === ' ' || c === '﻿'
const isIdStart = (c: string) => /[A-Za-z_$\u0080-￿]/.test(c)
const isIdPart = (c: string) => /[\w$\u0080-￿]/.test(c)

/** The script with its uses of `location` pointed at the shim. Text it can't read is left as it is. */
export function rewriteLocation(src: string): string {
  if (!src.includes('location')) return src
  const n = src.length
  // the last three tokens, newest last, for `window . location` and what may come before a slash
  const recent: Token[] = []
  const push = (kind: Kind, text: string) => {
    recent.push({ kind, text })
    if (recent.length > 3) recent.shift()
  }
  const back = (k: number): Token | undefined => recent[recent.length - k]
  // the brace depth at which each open `${` of a template literal goes back to template text
  const templates: number[] = []
  let depth = 0
  let out = ''
  let copied = 0
  let i = 0

  /** Skips template text from just after its opening backtick (or a `}` closing a substitution). */
  const template = () => {
    while (i < n) {
      const c = src[i]!
      if (c === '\\') i += 2
      else if (c === '`') {
        i++
        push('str', '`')
        return
      } else if (c === '$' && src[i + 1] === '{') {
        i += 2
        templates.push(depth)
        depth++
        push('punc', '${')
        return
      } else i++
    }
  }

  const regexAllowed = () => {
    const p = back(1)
    if (!p) return true
    if (p.kind === 'num' || p.kind === 'str' || p.kind === 'regex') return false
    if (p.kind === 'id') return BEFORE_REGEX.has(p.text)
    return p.text !== ')' && p.text !== ']'
  }

  while (i < n) {
    const c = src[i]!
    if (isSpace(c)) {
      i++
      continue
    }
    if (c === '/' && src[i + 1] === '/') {
      while (i < n && src[i] !== '\n' && src[i] !== '\r') i++
      continue
    }
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2)
      i = end < 0 ? n : end + 2
      continue
    }
    if (c === '"' || c === "'") {
      i++
      while (i < n && src[i] !== c && src[i] !== '\n') i += src[i] === '\\' ? 2 : 1
      i++
      push('str', c)
      continue
    }
    if (c === '`') {
      i++
      template()
      continue
    }
    if (c === '}' && templates.length && templates[templates.length - 1] === depth - 1) {
      templates.pop()
      depth--
      i++
      template()
      continue
    }
    if (c === '/' && regexAllowed()) {
      let j = i + 1
      let inClass = false
      for (; j < n; j++) {
        const d = src[j]!
        if (d === '\n' || d === '\r') break
        if (d === '\\') j++
        else if (d === '[') inClass = true
        else if (d === ']') inClass = false
        else if (d === '/' && !inClass) break
      }
      if (j < n && src[j] === '/') {
        i = j + 1
        while (i < n && isIdPart(src[i]!)) i++
        push('regex', '/')
        continue
      }
      // not a regular expression after all: a lone slash
      i++
      push('punc', '/')
      continue
    }
    if (isIdStart(c)) {
      const start = i
      while (i < n && isIdPart(src[i]!)) i++
      const word = src.slice(start, i)
      if (word === 'location' && rewrites(src, i, back)) {
        out += src.slice(copied, start) + LOCATION
        copied = i
      }
      push('id', word)
      continue
    }
    if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(src[i + 1] ?? ''))) {
      i++
      while (i < n && /[\w.]/.test(src[i]!)) i++
      push('num', '0')
      continue
    }
    if (c === '?' && src[i + 1] === '.' && !/[0-9]/.test(src[i + 2] ?? '')) {
      i += 2
      push('punc', '?.')
      continue
    }
    if (c === '{') depth++
    else if (c === '}') depth = Math.max(0, depth - 1)
    i++
    push('punc', c)
  }
  return copied ? out + src.slice(copied) : src
}

/** Whether `location`, just read and ending at `end`, is the document's address. */
function rewrites(src: string, end: number, back: (k: number) => Token | undefined): boolean {
  const p = back(1)
  if (p?.kind === 'punc' && (p.text === '.' || p.text === '?.')) {
    // window.location, but not a.window.location
    const owner = back(2)
    const before = back(3)
    return owner?.kind === 'id' && OWNERS.has(owner.text) && !(before?.kind === 'punc' && (before.text === '.' || before.text === '?.'))
  }
  if (p?.kind === 'id' && DECLARES.has(p.text)) return false
  let j = end
  while (j < src.length && isSpace(src[j]!)) j++
  // an object's key ({ location: 1 }, { a, location: 2 }); `c ? location : x` is a read
  if (src[j] === ':' && p?.kind === 'punc' && (p.text === '{' || p.text === ',')) return false
  return true
}
