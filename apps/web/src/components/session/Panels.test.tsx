import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { SidePanel, type PanelTab } from './Panels.tsx'
import { useSession } from './useSession.ts'
import { useStore } from '../../lib/store.ts'
import { agentPane, host } from '../../test/fixtures.ts'
import { usePages, type Page } from '../../lib/pages.ts'

function Panel({ closing }: { closing?: boolean }) {
  const s = useSession('box', '%1')
  const [tab, setTab] = useState<PanelTab>('changes')
  return <SidePanel s={s} items={[]} tab={tab} setTab={setTab} onTask={() => {}} onSubagent={() => {}} closing={closing} />
}
const panel = (closing?: boolean) => render(<MemoryRouter><Panel closing={closing} /></MemoryRouter>)

describe('the side panel', () => {
  // happy-dom has no layout: the tabs say where they are, 80px apart and 70px wide
  const left = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetLeft')
  const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetWidth')
  beforeEach(() => {
    useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude')] })] })
    const index = function (this: HTMLElement) {
      return [...(this.parentElement?.querySelectorAll('[role=tab]') ?? [])].indexOf(this)
    }
    Object.defineProperty(HTMLElement.prototype, 'offsetLeft', { configurable: true, get() { const i = index.call(this); return i < 0 ? 0 : 10 + i * 80 } })
    Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get() { return index.call(this) < 0 ? 0 : 70 } })
  })
  afterEach(() => {
    for (const [name, d] of [['offsetLeft', left], ['offsetWidth', width]] as const) {
      if (d) Object.defineProperty(HTMLElement.prototype, name, d)
      else delete (HTMLElement.prototype as any)[name]
    }
  })

  test('one underline slides under the chosen tab, and the body comes in fresh', async () => {
    const user = userEvent.setup()
    const { container } = panel()
    const line = () => container.querySelector('[data-part=underline]') as HTMLElement
    const body = () => container.querySelector('.overflow-y-auto') as HTMLElement
    expect(container.querySelectorAll('[data-part=underline]')).toHaveLength(1)
    expect(line().style.transform).toBe('translateX(10px) scaleX(0.7)')
    // drawn in place at first, so it doesn't slide in from the edge
    expect(line().className).not.toContain('transition-transform')
    const was = body()
    await user.click(screen.getByRole('tab', { name: 'Activity' }))
    expect(line().style.transform).toBe('translateX(170px) scaleX(0.7)')
    expect(line().className).toContain('transition-transform')
    expect(body()).not.toBe(was)
    expect(body().className).toContain('animate-fade-in')
    // no tab carries a border of its own any more: the line is the one mark
    for (const tab of screen.getAllByRole('tab')) expect(tab.className).not.toMatch(/border-b/)
  })

  test('comes in from its edge, and fades while it is closing', () => {
    expect(panel().container.querySelector('aside')!.className).toContain('animate-panel-in')
    expect(panel(true).container.querySelector('aside')!.className).toContain('animate-fade-out')
  })
})

describe("the session's pages", () => {
  const page = (slug: string, more: Partial<Page> = {}): Page => ({ host: 'box', slug, id: `box/${slug}`, name: `Plan ${slug}`, project: '/home/me/code', by: 'claude', added: 1, folder: `/home/me/code/private/plans/${slug}`, ...more })
  beforeEach(() => useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'claude')] })] }))
  afterEach(() => usePages.setState({ pages: null }))

  test('a Pages tab with the pages from its project, each a link to its viewer', async () => {
    usePages.setState({ pages: [page('robot', { progress: { done: 2, total: 3, now: 'Wiring' } }), page('elsewhere', { project: '/home/me/other', folder: '/home/me/other/p' })] })
    const user = userEvent.setup()
    panel()
    await user.click(screen.getByRole('tab', { name: /Pages/ }))
    const link = screen.getByRole('link', { name: /Plan robot/ })
    expect(link.getAttribute('href')).toBe('/pages/box/robot')
    expect(link.textContent).toContain('Wiring')
    expect(link.textContent).toContain('2/3')
    expect(screen.queryByText('Plan elsewhere')).toBeNull()
  })

  test('no Pages tab for a session with none', () => {
    usePages.setState({ pages: [page('far', { host: 'other' })] })
    panel()
    expect(screen.queryByRole('tab', { name: /Pages/ })).toBeNull()
  })
})
