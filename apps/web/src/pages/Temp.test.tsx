import { beforeEach, describe, expect, test } from 'bun:test'
import { act, fireEvent, screen, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { TempPage, deleteQuestion, resetThumbs } from './Temp.tsx'
import { MachinePage } from './Machines.tsx'
import { machineMenu } from '../components/menus.tsx'
import { ConfirmHost } from '../components/Confirm.tsx'
import { ContextMenuHost } from '../components/ui/ContextMenu.tsx'
import { useStore } from '../lib/store.ts'
import { ApiError, type TempItem, type TempListing } from '../lib/api.ts'
import { api } from '../test/fakes.ts'
import { host } from '../test/fixtures.ts'
import { flush, renderSettled as render } from '../test/render.tsx'

const NOW = Date.now()
const DAY = 86_400_000
const item = (name: string, more: Partial<TempItem> = {}): TempItem => ({
  id: `claude-tmp:${name}`,
  root: 'claude-tmp',
  path: `/tmp/claude-1000/-home-me-code-app/${name}`,
  name,
  kind: 'dir',
  size: 1024 * 1024,
  files: 3,
  modified: NOW - 10 * DAY,
  recent: false,
  images: [],
  ...more,
})
const A = item('app · 11111111', { size: 2 * 1024 ** 3 })
const B = item('app · 22222222', { modified: NOW - 2 * DAY })
const C = item('app · 33333333', { modified: NOW - 60_000, recent: true, size: 512 * 1024 })
const PASTE = item('0a49e8c1.txt', { root: 'claude-paste', kind: 'file', path: '/home/me/.claude/paste-cache/0a49e8c1.txt', size: 2048, files: 1 })
const LISTING: TempListing = {
  v: 1,
  host: 'box',
  roots: [
    { id: 'claude-tmp', agent: 'claude', label: 'Claude Code temp files', path: '/tmp/claude-1000', size: A.size + B.size + C.size, items: 3, exists: true },
    { id: 'claude-paste', agent: 'claude', label: 'Claude Code pasted text', path: '/home/me/.claude/paste-cache', size: 2048, items: 1, exists: true },
    { id: 'gemini-tmp', agent: 'gemini', label: 'Gemini CLI temp files', path: '/home/me/.gemini/tmp', size: 0, items: 0, exists: false },
  ],
  items: [A, B, C, PASTE],
}

const at = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="temp" element={<TempPage />} />
        <Route path="temp/:host" element={<TempPage />} />
        <Route path="machines/:host" element={<MachinePage />} />
      </Routes>
      <ConfirmHost />
      <ContextMenuHost />
    </MemoryRouter>,
  )

const checkbox = (name: string) => screen.getByRole('checkbox', { name: `Select ${name}` })
const checked = () => screen.queryAllByRole('checkbox').filter((c) => c.getAttribute('aria-checked') === 'true').map((c) => c.getAttribute('aria-label')!.replace(/^Select /, ''))

beforeEach(() => {
  resetThumbs()
  useStore.setState({ hosts: [host('box', { config: { id: 'box', kind: 'local', name: 'studio' } })], hostsLoaded: true })
  api.tempList.mockImplementation(async () => structuredClone(LISTING))
})

describe('the list', () => {
  test('groups items by folder with each folder total; empty folders are left out', async () => {
    await at('/temp/box')
    expect(api.tempList).toHaveBeenCalledWith('box')
    const claude = screen.getByRole('region', { name: 'Claude Code temp files' })
    expect(within(claude).getByText('2.0 GB · 3')).toBeTruthy()
    expect(within(claude).getByText('app · 11111111')).toBeTruthy()
    expect(within(screen.getByRole('region', { name: 'Claude Code pasted text' })).getByText('0a49e8c1.txt')).toBeTruthy()
    expect(screen.queryByText('Gemini CLI temp files')).toBeNull()
    // the recent one says so
    expect(screen.getAllByText('recent')).toHaveLength(1)
  })

  test('shift-click selects the range from the last row clicked', async () => {
    await at('/temp/box')
    fireEvent.click(checkbox(A.name))
    fireEvent.click(checkbox(PASTE.name), { shiftKey: true })
    expect(checked()).toEqual([A.name, B.name, C.name, PASTE.name])
    // a plain click takes one off again
    fireEvent.click(checkbox(B.name))
    expect(checked()).toEqual([A.name, C.name, PASTE.name])
  })

  test('"Older than a week" picks only old items no agent is using', async () => {
    await at('/temp/box')
    fireEvent.click(screen.getByRole('button', { name: 'Older than a week' }))
    expect(checked()).toEqual([A.name, PASTE.name])
    expect(within(screen.getByRole('region', { name: 'Selection' })).getByText(/2 selected/)).toBeTruthy()
  })

  test('deleting asks with the size freed and a warning about recent items, then lists again', async () => {
    api.tempDelete.mockImplementation(async () => ({ removed: [A.path, C.path], freed: A.size + C.size, failed: [] }))
    await at('/temp/box')
    fireEvent.click(checkbox(A.name))
    fireEvent.click(checkbox(C.name))
    fireEvent.click(within(screen.getByRole('region', { name: 'Selection' })).getByRole('button', { name: /Delete/ }))
    await flush()
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('Delete 2 items?')).toBeTruthy()
    expect(within(dialog).getByText("Frees 2.0 GB on studio. This can't be undone. 1 of them changed in the last 30 minutes, so an agent may still be using them.")).toBeTruthy()
    expect(api.tempDelete).not.toHaveBeenCalled()
    await act(async () => fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' })))
    await flush()
    expect(api.tempDelete).toHaveBeenCalledWith('box', [A.path, C.path])
    expect(api.tempList).toHaveBeenCalledTimes(2)
    expect(useStore.getState().toasts.some((t) => t.title === 'Freed 2.0 GB')).toBe(true)
  })

  test('the question for one item names it; no warning when nothing is recent', () => {
    expect(deleteQuestion([B], 'studio')).toEqual({ title: `Delete ${B.name}?`, body: "Frees 1.0 MB on studio. This can't be undone.", yes: 'Delete', danger: true })
    expect(deleteQuestion([C], 'studio').body).toContain('It changed in the last 30 minutes')
  })

  test('a machine whose inyon is too old says so where the list would be', async () => {
    api.tempList.mockImplementation(async () => {
      throw new ApiError(409, 'Update Inyon on studio to see its temp files')
    })
    await at('/temp/box')
    expect(screen.getByText('Update Inyon on studio to see its temp files')).toBeTruthy()
    expect(screen.getByRole('button', { name: /Try again/ })).toBeTruthy()
  })

  test('nothing to show', async () => {
    api.tempList.mockImplementation(async () => ({ ...LISTING, items: [] }))
    await at('/temp/box')
    expect(screen.getByText('Nothing here')).toBeTruthy()
  })

  test('an offline machine is not asked', async () => {
    useStore.setState({ hosts: [host('box', { status: 'error' })] })
    await at('/temp/box')
    expect(screen.getByText('Not connected')).toBeTruthy()
    expect(api.tempList).not.toHaveBeenCalled()
  })
})

describe('an item and its files', () => {
  const open = async () => {
    api.tempFiles.mockImplementation(async () => ({
      path: A.path,
      truncated: false,
      files: [
        { path: `${A.path}/scratchpad/plan.html`, rel: 'scratchpad/plan.html', size: 120, modified: NOW - 5000, kind: 'text' as const },
        { path: `${A.path}/images/shot.png`, rel: 'images/shot.png', size: 4000, modified: NOW - 9000, kind: 'image' as const },
      ],
    }))
    await at('/temp/box')
    fireEvent.click(screen.getByText(A.name))
    await flush()
    expect(api.tempFiles).toHaveBeenCalledWith('box', A.path)
  }

  test('HTML previews as text: nothing in it becomes an element', async () => {
    api.tempPreview.mockImplementation(async () => ({ path: `${A.path}/scratchpad/plan.html`, mime: 'text/html', size: 120, text: '<h1 id="planted">Hi</h1><script>window.x=1</script>', truncated: false }))
    await open()
    fireEvent.click(screen.getByText('scratchpad/plan.html'))
    await flush()
    expect(api.tempPreview).toHaveBeenCalledWith('box', `${A.path}/scratchpad/plan.html`)
    expect(screen.getByText('<h1 id="planted">Hi</h1><script>window.x=1</script>').tagName).toBe('PRE')
    expect(document.getElementById('planted')).toBeNull()
  })

  test('an image previews from a data: URL', async () => {
    api.tempPreview.mockImplementation(async (_h: string, _p: string, max?: number) => (max ? {} : { path: `${A.path}/images/shot.png`, mime: 'image/png', size: 4000, base64: 'iVBORw0KGgo=', truncated: false }) as any)
    await open()
    fireEvent.click(screen.getByText('images/shot.png'))
    await flush()
    const img = screen.getByRole('img', { name: 'images/shot.png' }) as HTMLImageElement
    expect(img.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=')
  })

  test('a long text says it was cut', async () => {
    api.tempPreview.mockImplementation(async () => ({ path: 'x', mime: 'text/plain', size: 900_000, text: 'abc', truncated: true }))
    await open()
    fireEvent.click(screen.getByText('scratchpad/plan.html'))
    await flush()
    expect(screen.getByText(/Truncated: showing the first 256 KB/)).toBeTruthy()
  })
})

test('a slow answer for an item opened before never fills in the one open now', async () => {
  let answerA: (v: unknown) => void = () => {}
  const file = (p: string) => ({ path: `${p}/x.txt`, rel: `${p.slice(-8)}.txt`, size: 1, modified: NOW, kind: 'text' as const })
  api.tempFiles.mockImplementation(((_h: string, p: string) => (p === A.path ? new Promise((r) => (answerA = r)) : Promise.resolve({ path: p, truncated: false, files: [file(p)] }))) as any)
  await at('/temp/box')
  fireEvent.click(screen.getByText(A.name))
  await flush()
  // closed before A answers, then B opened
  fireEvent.keyDown(window, { key: 'Escape' })
  await flush()
  fireEvent.click(screen.getByText(B.name))
  await flush()
  expect(screen.getByText('22222222.txt')).toBeTruthy()
  await act(async () => answerA({ path: A.path, truncated: false, files: [file(A.path)] }))
  await flush()
  expect(screen.queryByText('11111111.txt')).toBeNull()
  expect(screen.getByText('22222222.txt')).toBeTruthy()
})

test("a refresh overtaken by the next delete's never puts the deleted file back", async () => {
  const f1 = { path: `${A.path}/one.txt`, rel: 'one.txt', size: 1, modified: NOW - DAY, kind: 'text' as const }
  const f2 = { path: `${A.path}/two.txt`, rel: 'two.txt', size: 1, modified: NOW - DAY, kind: 'text' as const }
  let slow: (v: unknown) => void = () => {}
  let calls = 0
  api.tempFiles.mockImplementation((async () => {
    calls++
    if (calls === 1) return { path: A.path, truncated: false, files: [f1, f2] }
    // the refresh after the first delete answers last, with the second file still there
    if (calls === 2) return new Promise((r) => (slow = r))
    return { path: A.path, truncated: false, files: [] }
  }) as any)
  api.tempDelete.mockImplementation((async (_h: string, paths: string[]) => ({ removed: paths, freed: 1, failed: [] })) as any)
  await at('/temp/box')
  fireEvent.click(screen.getByText(A.name))
  await flush()
  const remove = async (name: string) => {
    fireEvent.contextMenu(screen.getByText(name), { clientX: 10, clientY: 10 })
    await flush()
    fireEvent.click(screen.getByRole('menuitem', { name: /Delete/ }))
    await flush()
    await act(async () => fireEvent.click(within(screen.getAllByRole('dialog').at(-1)!).getByRole('button', { name: 'Delete' })))
    await flush()
  }
  await remove('one.txt')
  await remove('two.txt')
  expect(screen.getByText('This folder is empty.')).toBeTruthy()
  await act(async () => slow({ path: A.path, truncated: false, files: [f2] }))
  await flush()
  expect(screen.queryByText('two.txt')).toBeNull()
  expect(screen.getByText('This folder is empty.')).toBeTruthy()
})

describe('where it is reached', () => {
  test('the machine page has a Temp files tab that lists them', async () => {
    await at('/machines/box')
    fireEvent.click(screen.getByRole('radio', { name: 'Temp files' }))
    await flush()
    expect(api.tempList).toHaveBeenCalledWith('box')
    expect(screen.getByText(A.name)).toBeTruthy()
  })

  test("a machine's menu opens its temp files", () => {
    const went: string[] = []
    const items = machineMenu(useStore.getState().hosts[0]!, ((to: string) => went.push(to)) as any)
    const temp = items.find((i) => i.key === 'temp')!
    expect(temp.label).toBe('Temp files')
    temp.onSelect()
    expect(went).toEqual(['/temp/box'])
  })

  test('/temp picks the first machine that is online', async () => {
    useStore.setState({ hosts: [host('off', { status: 'error' }), host('box', { config: { id: 'box', kind: 'local', name: 'studio' } })] })
    await at('/temp')
    expect(api.tempList).toHaveBeenCalledWith('box')
    expect(screen.getByRole('combobox', { name: 'Machine' }).textContent).toContain('studio')
  })
})
