import { expect, test } from 'bun:test'
import { render, waitFor } from '@testing-library/react'
import { Markdown } from './Markdown.tsx'
import { isLoaded, languageOf, loadLanguage } from './highlight.ts'

const srcs = (text: string) => [...render(<Markdown text={text} />).container.querySelectorAll('img')].map((i) => i.getAttribute('src'))

test('agent output shows images from this site only', () => {
  expect(srcs('![a](/api/hosts/x/file.png)')).toEqual(['/api/hosts/x/file.png'])
  // "//host/…" is another site, not a path here
  for (const url of ['//evil.example/x.png?d=secret', 'https://evil.example/x.png']) expect(srcs(`![a](${url})`)).toEqual([])
  expect(render(<Markdown text="![leak](//evil.example/x.png)" />).container.textContent).toContain('leak')
})

// the grammars left the chat chunk: a block shows plain at once and coloured when its grammar arrives
test('a code block is highlighted once its language has loaded', async () => {
  expect(isLoaded('rust')).toBe(false)
  const { container } = render(<Markdown text={'```rust\nfn main() {}\n```'} />)
  const plain = container.querySelector('pre > code')!
  expect(plain.className).toBe('language-rust')
  expect(plain.querySelector('span')).toBeNull()
  await waitFor(() => expect(container.querySelector('pre > code.hljs')).not.toBeNull())
  const code = container.querySelector('pre > code')!
  // the markup rehype-highlight made, which styles.css's .hljs rules expect
  expect(code.className).toBe('hljs language-rust')
  expect(code.querySelector('span.hljs-keyword')?.textContent).toBe('fn')
  expect(code.textContent).toBe('fn main() {}\n')
  expect(container.querySelector('.group\\/code span')?.textContent).toBe('rust')
})

test('a loaded language highlights on the first render, and aliases map to it', async () => {
  await loadLanguage('typescript')
  const { container } = render(<Markdown text={'```ts\nconst a = 1\n```'} />)
  expect(container.querySelector('pre > code')!.className).toBe('hljs language-ts')
  expect(container.querySelector('span.hljs-keyword')?.textContent).toBe('const')
  expect(languageOf('language-TSX')).toBe('typescript')
  expect(languageOf('language-yml')).toBe('yaml')
  expect(languageOf(undefined)).toBeUndefined()
})

test('an unknown language and inline code stay plain', async () => {
  expect(await loadLanguage('brainfuck')).toBe(false)
  const { container } = render(<Markdown text={'```brainfuck\n+++\n```\n\nand `inline`'} />)
  await Promise.resolve()
  expect(container.querySelector('pre > code')!.className).toBe('language-brainfuck')
  expect(container.querySelectorAll('.hljs').length).toBe(0)
  const inline = [...container.querySelectorAll('code')].find((c) => c.textContent === 'inline')!
  expect(inline.className).toBe('')
})

test('code text is escaped, not rendered as markup', async () => {
  await loadLanguage('xml')
  const { container } = render(<Markdown text={'```html\n<img src=x onerror=alert(1)>\n```'} />)
  expect(container.querySelector('pre img')).toBeNull()
  expect(container.querySelector('pre > code')!.textContent).toBe('<img src=x onerror=alert(1)>\n')
})

// Ghostex #252: a reply's code keeps its indent
test('a code block keeps the indent of its lines', () => {
  const { container } = render(<Markdown text={'```\ndef f():\n    return 1\n```'} />)
  expect(container.querySelector('pre > code')!.textContent).toBe('def f():\n    return 1\n')
})
