import { afterEach, expect, test } from 'bun:test'
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { AssistantTurn, PendingBubble, QueuedBubble, UserMessage, uploadName, WorkingIndicator } from './Messages.tsx'
import { ChatHost } from './agent.ts'
import { usePages, type Page } from '../../lib/pages.ts'
import { buildRows } from '../../lib/chat.ts'
import { item } from '../../test/fixtures.ts'

test("an upload's chip shows the name it was picked with", () => {
  expect(uploadName('/home/me/.inyon/uploads/2026-10-13/d09c14d4-62c/184807-76855d-inyon-test.txt')).toBe('inyon-test.txt')
  // uploads from before the id was added
  expect(uploadName('/home/me/.inyon/uploads/2026-01-02/x/101010-photo.jpg')).toBe('photo.jpg')
  // a name that only looks like the prefix keeps the rest
  expect(uploadName('/home/me/.inyon/uploads/2026-10-13/x/184807-76855d-123456-notes.md')).toBe('123456-notes.md')
})

test('a message keeps its box when it goes from sent, or queued, to confirmed', () => {
  const padding = (el: Element) => el.className.split(' ').filter((c) => /^p[xy]-/.test(c)).sort()
  const [row] = buildRows([item.user('run the tests')])
  if (row.type !== 'user') throw new Error('not a user row')
  const confirmed = render(<UserMessage row={row} />)
  const expected = padding(screen.getByText('run the tests').closest('.bg-bubble')!)
  confirmed.unmount()
  render(<PendingBubble p={{ id: 'p1', text: 'run the tests', status: 'sent', at: 0 }} onRetry={() => {}} onDismiss={() => {}} />)
  expect(padding(screen.getByText('run the tests').closest('.bg-bubble')!)).toEqual(expected)
  render(<QueuedBubble text="then lint" agent="claude" />)
  expect(padding(screen.getByText('then lint').closest('.border-dashed')!)).toEqual(expected)
})

test('the working line fades in', () => {
  const { container } = render(<WorkingIndicator activity="Pondering…" />)
  expect(container.firstElementChild!.className).toContain('animate-fade-in')
})

// Ghostex #252: chat lost the leading indent of a message's lines
test('a message keeps the indent of its lines', () => {
  const text = 'fix this:\n    if x:\n        y()'
  const [row] = buildRows([item.user(text)])
  if (row.type !== 'user') throw new Error('not a user row')
  const { container } = render(<UserMessage row={row} />)
  const bubble = container.querySelector('.bg-bubble')!
  expect(bubble.className).toContain('whitespace-pre-wrap')
  expect(bubble.textContent).toContain('\n    if x:\n        y()')
})

afterEach(() => usePages.setState({ pages: null }))

test('a reply that links to a page shows it as a card that opens it, once the reply is done', () => {
  const page: Page = { host: 'hub:h1/local', slug: 'my-plan', id: 'hub:h1/local/my-plan', name: 'My plan', project: '/home/me/code', by: 'claude', added: 1, folder: '/home/me/code/plans' }
  usePages.setState({ pages: [page] })
  const text = 'Added it. Open it in the app at /pages/local/my-plan, and /pages/local/gone is gone.'
  const [done] = buildRows([item.text(text), item.user('thanks')])
  const [open] = buildRows([item.text(text)])
  if (done.type !== 'assistant' || open.type !== 'assistant') throw new Error('not a reply')
  const at = (row: typeof done) => render(<MemoryRouter><ChatHost.Provider value="hub:h1/laptop"><AssistantTurn row={row} live={false} /></ChatHost.Provider></MemoryRouter>)
  const shown = at(done)
  const card = screen.getByRole('link', { name: /My plan/ })
  expect(card.getAttribute('href')).toBe('/pages/hub%3Ah1%2Flocal/my-plan')
  expect(screen.getAllByRole('link').filter((l) => l.getAttribute('href')?.startsWith('/pages/'))).toHaveLength(1)
  shown.unmount()
  if (open.open) {
    at(open)
    expect(screen.queryByRole('link', { name: /My plan/ })).toBeNull()
  }
})
