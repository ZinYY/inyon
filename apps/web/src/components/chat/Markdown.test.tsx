import { expect, test } from 'bun:test'
import { render, waitFor } from '@testing-library/react'
import { Markdown } from './Markdown.tsx'
import { isLoaded, languageOf, loadLanguage } from './highlight.ts'
import { mathDelimiters } from './mathDelimiters.ts'

const srcs = (text: string) => [...render(<Markdown text={text} />).container.querySelectorAll('img')].map((i) => i.getAttribute('src'))

test('bold punctuation next to CJK prose keeps its emphasis without inserting spaces', () => {
  for (const [source, bold] of [
    ['结论：**今天适合散步。**明天再看天气。', '今天适合散步。'],
    ['选择**“海边步道”**然后喝咖啡。', '“海边步道”'],
    ['推荐**路线（约一小时）**之后休息。', '路线（约一小时）'],
    ['**海辺を歩こう。**その後は休憩。', '海辺を歩こう。'],
    ['**산책(한 시간)**을 추천합니다.', '산책(한 시간)'],
  ]) {
    const { container } = render(<Markdown text={source!} />)
    expect(container.querySelector('strong')?.textContent).toBe(bold!)
    expect(container.textContent).toBe(source!.replaceAll('**', ''))
  }
})

test('streaming a bold sentence and its adjacent continuation does not lose emphasis', () => {
  const { container, rerender } = render(<Markdown text="推荐：**海边散步。" />)
  expect(container.textContent).toBe('推荐：**海边散步。')
  for (const suffix of ['**', '**然后', '**然后喝咖啡。']) {
    rerender(<Markdown text={'推荐：**海边散步。' + suffix} />)
    expect(container.querySelectorAll('strong')).toHaveLength(1)
    expect(container.querySelector('strong')?.textContent).toBe('海边散步。')
    expect(container.textContent).not.toContain('*')
  }
})

test('CJK emphasis preserves code, escaped markers, links and nested inline formatting', () => {
  const literal = '**散步。**之后'
  const { container } = render(<Markdown text={[
    '`' + literal + '`', '```text\n' + literal + '\n```',
    String.raw`\*\*散步。\*\*之后`,
    '推荐**[步道](https://example.com/)和`coffee`。**之后休息。',
    '也可**散步和*休息*。**明天再来。',
    'English **bold.** Next. snake_case and 2 * 3 * 4.',
  ].join('\n\n')} />)
  expect(container.querySelector('p > code')?.textContent).toBe(literal)
  expect(container.querySelector('pre > code')?.textContent).toBe(literal + '\n')
  expect([...container.querySelectorAll('p')].some(p => p.textContent === literal)).toBe(true)
  expect(container.querySelectorAll('strong')).toHaveLength(3)
  expect(container.querySelector('strong a')?.getAttribute('href')).toBe('https://example.com/')
  expect(container.querySelector('strong code')?.textContent).toBe('coffee')
  expect(container.querySelector('strong em')?.textContent).toBe('休息')
})

test('CJK bold works in table cells alongside lazy math rendering', async () => {
  const { container } = render(<Markdown text={'| Plan | Note |\n| --- | --- |\n| **散步。**之后休息 | **距离（$d=vt$）**可以计算 |'} />)
  await waitFor(() => expect(container.querySelector('.katex')).not.toBeNull())
  expect(container.querySelectorAll('table strong')).toHaveLength(2)
  expect(container.querySelector('table strong .katex')).not.toBeNull()
  expect(container.querySelector('td')?.textContent).toBe('散步。之后休息')
})

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

test('TeX delimiters normalize outside inline, fenced and indented code only', () => {
  expect(mathDelimiters(String.raw`Inline \(x^2\).`)).toBe('Inline $x^2$.')
  expect(mathDelimiters(String.raw`\[\frac{a}{b}\]`)).toBe('\n$$\n\\frac{a}{b}\n$$\n')
  for (const code of ['`\\(x\\)`', '`` \\(x\\) ` literal ``', '```tex\n\\[x\\]\n```', '~~~tex\n\\(x\\)\n~~~', '```tex\n\\(x\\)', '    \\(x\\)\n']) {
    expect(mathDelimiters(code)).toBe(code)
  }
})

test('shared chat renders inline math, display math and GFM tables', async () => {
  const { container } = render(<Markdown text={String.raw`Inline \(x^2 + y^2 = z^2\), and $E=mc^2$.

\[
\sum_{i=1}^{n} i = \frac{n(n+1)}{2}
\]

| Quantity | Formula |
| --- | --- |
| Area | $\pi r^2$ |
| Sum | $a+b$ |`} />)
  await waitFor(() => expect(container.querySelectorAll('.katex')).toHaveLength(5), { timeout: 3000 })
  expect(container.querySelectorAll('.katex-display')).toHaveLength(1)
  expect(container.querySelectorAll('table tbody tr')).toHaveLength(2)
  expect(container.querySelectorAll('math')).toHaveLength(5)
})

test('a drawn formula without raw TeX retains fraction alignment', () => {
  const drawing = '      25\nd = 3 ── = 1.25\n      60'
  const { container } = render(<Markdown text={'```terminal-math\n' + drawing + '\n```'} />)
  expect(container.querySelector('[role="math"]')?.textContent).toBe(drawing)
  expect(container.textContent).not.toContain('terminal-math')
})

test('invalid and untrusted TeX remain contained without external images or executable links', async () => {
  const { container } = render(<Markdown text={String.raw`$\includegraphics{https://evil.example/leak}$

$\href{javascript:alert(1)}{bad}$

$\unsupported{x}$

Still readable.`} />)
  await waitFor(() => expect(container.querySelector('.katex')).not.toBeNull())
  expect(container.querySelector('img')).toBeNull()
  expect(container.querySelector('a[href^="javascript:"]')).toBeNull()
  expect(container.textContent).toContain('Still readable.')
})
