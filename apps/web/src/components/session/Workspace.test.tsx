import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { FilesTab, WorkingTree, ReviewBar } from './Workspace.tsx'
import { api } from '../../test/fakes.ts'
import type { GitState } from '../../lib/api.ts'
import { renderSettled as render } from '../../test/render.tsx'
import { appendTo, onCompose } from '../../lib/compose.ts'
import { review, reviewMessage } from '../../lib/review.ts'
import { useStore } from '../../lib/store.ts'

const p = { host: 'box', pane: '%4' }
let composed: string[] = []
let off = () => {}
beforeEach(() => {
  composed = []
  off = onCompose('box', '%4', (t) => composed.push(t))
  review.clear('box:%4')
  useStore.setState({ toasts: [] })
})
afterEach(() => off())

describe('Files', () => {
  test('browses the folder, opens a file and mentions it', async () => {
    api.dir.mockImplementation(async (_h: string, _p: string, path: string) =>
      path === 'src'
        ? { cwd: '/srv/app', path, entries: [{ name: 'main.ts', dir: false, size: 120, mtime: 0 }] }
        : { cwd: '/srv/app', path, entries: [{ name: 'src', dir: true, size: 0, mtime: 0 }, { name: 'README.md', dir: false, size: 2048, mtime: 0 }] },
    )
    api.file.mockImplementation(async (_h: string, _p: string, path: string) => ({ kind: 'text', path, text: 'export const a = 1\n', size: 19, truncated: false }))
    await render(<FilesTab p={p} />)
    expect(screen.getByText('app')).toBeTruthy()
    expect(screen.getByText('2 KB')).toBeTruthy()
    await userEvent.click(screen.getByText('src'))
    expect(api.dir.mock.calls.at(-1)).toEqual(['box', '%4', 'src'])
    await userEvent.click(await screen.findByText('main.ts'))
    expect(api.file.mock.calls.at(-1)).toEqual(['box', '%4', 'src/main.ts'])
    expect((await screen.findByText(/export/)).closest('pre')).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: /Mention/ }))
    expect(composed).toEqual(['@src/main.ts'])
    await userEvent.click(screen.getByRole('button', { name: 'Back to the folder' }))
    expect(await screen.findByText('main.ts')).toBeTruthy()
  })

  test('a binary file says so instead of showing bytes', async () => {
    api.dir.mockImplementation(async () => ({ cwd: '/srv/app', path: '', entries: [{ name: 'a.bin', dir: false, size: 10, mtime: 0 }] }))
    api.file.mockImplementation(async () => ({ kind: 'binary', path: 'a.bin', size: 10 }))
    await render(<FilesTab p={p} />)
    await userEvent.click(screen.getByText('a.bin'))
    expect(await screen.findByText('Not a text file')).toBeTruthy()
  })
})

describe('Working tree', () => {
  const tree: GitState = { repo: true, remote: true, branch: 'main', upstream: 'origin/main', ahead: 1, behind: 0, files: [{ path: 'src/a.ts', status: 'M', add: 1, del: 1 }, { path: 'notes.md', status: '?', add: 0, del: 0 }] }
  const diff = { path: 'src/a.ts', binary: false, hunks: [{ oldStart: 1, oldLines: 2, newStart: 1, newLines: 2, lines: [' keep', '-old', '+new'] }] }

  test('outside a repository it says so', async () => {
    api.git.mockImplementation(async () => ({ repo: false, files: [] }))
    await render(<WorkingTree p={p} poke={0} />)
    expect(screen.getByText('Not a git repository')).toBeTruthy()
  })

  test('shows the branch and files, and comments on a line go into the message together', async () => {
    api.git.mockImplementation(async () => tree)
    api.gitDiff.mockImplementation(async () => diff)
    await render(
      <>
        <WorkingTree p={p} poke={0} />
        <ReviewBar p={p} />
      </>,
    )
    expect(screen.getByText('main')).toBeTruthy()
    expect(screen.getByText('U')).toBeTruthy()
    expect(api.gitDiff.mock.calls.at(-1)).toEqual(['box', '%4', 'src/a.ts'])
    // line 2 is both a removed line (old file) and an added one (new file)
    expect(screen.getByRole('button', { name: 'Comment on removed line 2' })).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Comment on line 2' }))
    await userEvent.type(screen.getByLabelText('Comment'), 'Keep the old name')
    await userEvent.click(screen.getByRole('button', { name: 'Save comment' }))
    expect(screen.getByText('1 comment')).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Add to message' }))
    expect(composed).toHaveLength(1)
    expect(composed[0]).toContain('src/a.ts:2')
    expect(screen.queryByText('1 comment')).toBeNull()
  })

  test('commits everything with your message', async () => {
    api.git.mockImplementation(async () => tree)
    api.gitDiff.mockImplementation(async () => diff)
    api.gitCommit.mockImplementation(async () => ({ commit: 'abc1234' }))
    await render(<WorkingTree p={p} poke={0} />)
    const commit = screen.getByRole('button', { name: 'Commit all' }) as HTMLButtonElement
    expect(commit.disabled).toBe(true)
    await userEvent.type(screen.getByLabelText('Commit message'), 'Rename a')
    await userEvent.click(commit)
    expect(api.gitCommit.mock.calls.at(-1)).toEqual(['box', '%4', 'Rename a'])
    expect(useStore.getState().toasts.map((t) => t.title)).toContain('Committed abc1234')
  })
})

test('review comments read as one message, file by file and in line order', () => {
  expect(
    reviewMessage([
      { file: 'b.ts', line: 9, removed: false, code: '  x()', text: 'Why?' },
      { file: 'a.ts', line: 4, removed: true, code: 'old()', text: 'Keep this' },
      { file: 'b.ts', line: 2, removed: false, code: '', text: 'Blank line here' },
    ]),
  ).toBe('Comments on your changes:\n\nb.ts:2\n> (empty line)\nBlank line here\n\nb.ts:9\n> x()\nWhy?\n\na.ts:4 (removed line)\n> old()\nKeep this')
})

test('added text goes after the draft, on its own paragraph when it has several lines', () => {
  expect(appendTo('', '@a.ts')).toBe('@a.ts')
  expect(appendTo('look at ', '@a.ts')).toBe('look at @a.ts')
  expect(appendTo('fix this', 'one\ntwo')).toBe('fix this\n\none\ntwo')
})

test('right-clicking a file offers to open it, mention it or copy its path', async () => {
  const { ContextMenuHost } = await import('../ui/ContextMenu.tsx')
  const { fireEvent } = await import('@testing-library/react')
  api.dir.mockImplementation(async () => ({ cwd: '/srv/app', path: '', entries: [{ name: 'main.ts', dir: false, size: 1, mtime: 0 }] }))
  await render(
    <>
      <FilesTab p={p} />
      <ContextMenuHost />
    </>,
  )
  fireEvent.contextMenu(screen.getByText('main.ts'), { clientX: 10, clientY: 10 })
  expect(screen.getByRole('menuitem', { name: /Open/ })).toBeTruthy()
  expect(screen.getByRole('menuitem', { name: /Copy path/ })).toBeTruthy()
  await userEvent.click(screen.getByRole('menuitem', { name: /Mention in message/ }))
  expect(composed).toEqual(['@main.ts'])
})

test("This chat with no edits points to the working tree, where edits made through commands show", async () => {
  const { ChangesTab } = await import('./Panels.tsx')
  try {
    localStorage.removeItem('inyon.changes')
  } catch {}
  api.git.mockImplementation(async () => ({ repo: true, branch: 'main', files: [{ path: 'test/cart.test.js', status: 'M', add: 6, del: 1 }] }))
  api.gitDiff.mockImplementation(async () => ({ path: 'test/cart.test.js', binary: false, hunks: [{ oldStart: 1, oldLines: 1, newStart: 1, newLines: 2, lines: [' a', '+b'] }] }))
  await render(<ChangesTab items={[]} p={p} />)
  expect(screen.getByText('No changes yet')).toBeTruthy()
  expect(screen.getByText(/Edits it makes through commands show under Working tree/)).toBeTruthy()
  await userEvent.click(screen.getByRole('button', { name: 'Show working tree' }))
  expect(await screen.findByText('cart.test.js', { exact: false })).toBeTruthy()
  try {
    localStorage.removeItem('inyon.changes')
  } catch {}
})
