import { describe, expect, test } from 'bun:test'
import { act, fireEvent, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { AGENTS } from '@inyon/protocol'
import { Composer } from './Composer.tsx'
import { ComposerControls } from '../session/ComposerControls.tsx'
import type { SessionModel } from '../session/useSession.ts'
import { useStore } from '../../lib/store.ts'
import { api, emit, socket } from '../../test/fakes.ts'
import { chatInfo, fresh, host } from '../../test/fixtures.ts'
import { flush, renderSettled as render } from '../../test/render.tsx'

// drafts outlive the component (per pane, in module state), so each test uses its own pane
let n = 0
const nextPane = () => `%${++n}`
const box = () => screen.getByRole('textbox') as HTMLTextAreaElement
const sendButton = () => screen.getByLabelText(/^(Send|Queue follow-up|Run command|Stop)$/) as HTMLButtonElement
const draft = (pane: string, text: string, at: number, source: 'app' | 'terminal' = 'terminal') => act(() => emit({ t: 'chat.draft', key: `box:${pane}`, text, source, at }))

describe('Composer keeps what you type while live data flows', () => {
  test('screen, info and machine refreshes do not touch the text', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const key = `box:${pane}`
    const { rerender } = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), 'Fix the')
    act(() => {
      emit({ t: 'chat.screen', key, screen: { working: true, width: 80, activity: 'Thinking…' } })
      emit({ t: 'chat.info', key, info: chatInfo() })
      emit({ t: 'overview', hosts: [host('box')] })
      emit({ t: 'overview', hosts: fresh([host('box')]) })
    })
    rerender(<Composer host="box" pane={pane} agent="claude" working controls={<span>new controls</span>} />)
    await user.type(box(), ' login bug')
    expect(box().value).toBe('Fix the login bug')
  })

  test("another device's draft does not replace what you're typing", async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), 'mine')
    draft(pane, 'theirs', 1)
    await user.type(box(), ' still')
    expect(box().value).toBe('mine still')
  })

  test("the terminal's draft is adopted when you aren't editing", async () => {
    const pane = nextPane()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    draft(pane, 'typed in tmux', 1)
    expect(box().value).toBe('typed in tmux')
    expect(screen.getByText('From the terminal')).toBeTruthy()
  })

  test("a newer terminal draft replaces the old one after you come back from the terminal view", async () => {
    const pane = nextPane()
    const first = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    draft(pane, 'second desk draft', 1)
    expect(box().value).toBe('second desk draft')
    // the terminal view replaces the chat; meanwhile the desk keeps typing
    first.unmount()
    draft(pane, 'third draft from the terminal tab', 2)
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    expect(box().value).toBe('third draft from the terminal tab')
    expect(screen.getByText('From the terminal')).toBeTruthy()
  })

  test("what you typed on the phone survives leaving the chat when nothing newer came in", async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const first = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), 'my own words')
    first.unmount()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    expect(box().value).toBe('my own words')
  })

  test('typing is shared with the hub after a pause, once', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    // earlier tests' timers still push their drafts (on purpose: leaving the page keeps it)
    const mine = () => socket.send.mock.calls.filter(([m]) => (m as { pane?: string }).pane === pane)
    await user.type(box(), 'hello')
    expect(mine()).toEqual([])
    await act(() => Bun.sleep(500))
    expect(mine()).toEqual([[{ t: 'draft', host: 'box', pane, text: 'hello' }]])
    // the hub saying back the same text changes nothing
    draft(pane, 'hello', 2, 'app')
    await user.type(box(), '!')
    expect(box().value).toBe('hello!')
  })

  test('leaving the app mid-sentence sends the draft at once, not after the pause', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    const mine = () => socket.send.mock.calls.filter(([m]) => (m as { pane?: string }).pane === pane)
    await user.type(box(), 'half')
    expect(mine()).toEqual([])
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    try {
      act(() => void document.dispatchEvent(new Event('visibilitychange')))
    } finally {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
    expect(mine()).toEqual([[{ t: 'draft', host: 'box', pane, text: 'half' }]])
    // and only once: the timer it replaced doesn't send it again
    await act(() => Bun.sleep(500))
    expect(mine()).toHaveLength(1)
  })

  test('the text comes back after leaving the page and returning', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const { unmount } = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), 'half a thought')
    unmount()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    expect(box().value).toBe('half a thought')
  })
})

describe('Composer send button', () => {
  test('Send, Queue follow-up, Run command and Stop', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const { rerender } = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    expect(sendButton().getAttribute('aria-label')).toBe('Send')
    expect(sendButton().disabled).toBe(true)
    await user.type(box(), 'hi')
    expect(sendButton().disabled).toBe(false)
    rerender(<Composer host="box" pane={pane} agent="claude" working />)
    expect(sendButton().getAttribute('aria-label')).toBe('Queue follow-up')
    const button = sendButton()
    const icon = button.firstElementChild!
    await user.clear(box())
    expect(sendButton().getAttribute('aria-label')).toBe('Stop')
    // the same button turns into Stop: its colour fades over and only the icon is swapped, with a fade
    expect(sendButton()).toBe(button)
    expect(button.firstElementChild).not.toBe(icon)
    expect(button.firstElementChild!.className).toContain('animate-fade-in')
    await user.type(box(), '!ls -la')
    expect(sendButton().getAttribute('aria-label')).toBe('Run command')
    expect(sendButton()).toBe(button)
    expect(screen.getByText(/Shell command/)).toBeTruthy()
  })

  test('while Codex works, a message is queued unless you switch to steering it in', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const { rerender } = await render(<Composer host="box" pane={pane} agent="codex" working={false} />)
    // idle: no choice to make
    expect(screen.queryByRole('switch')).toBeNull()
    rerender(<Composer host="box" pane={pane} agent="codex" working />)
    const toggle = screen.getByRole('switch') as HTMLButtonElement
    expect(toggle.getAttribute('aria-checked')).toBe('false')
    expect(toggle.textContent).toBe('Queue')
    await user.type(box(), 'also check the tests')
    expect(sendButton().getAttribute('aria-label')).toBe('Queue follow-up')
    await user.click(toggle)
    expect(toggle.getAttribute('aria-checked')).toBe('true')
    expect(toggle.textContent).toBe('Steer now')
    expect(screen.getByLabelText('Steer now')).toBeTruthy()
    expect(box().placeholder).toBe('Steer Codex now…')
    await user.click(screen.getByLabelText('Steer now'))
    await flush()
    const [call] = api.send.mock.calls
    expect(call.slice(0, 3)).toEqual(['box', pane, 'also check the tests'])
    expect(call[4]).toBe(true)
    // a shell command runs as itself whichever way; other agents never see the switch
    await user.type(box(), '!ls')
    expect(screen.queryByRole('switch')).toBeNull()
    rerender(<Composer host="box" pane={nextPane()} agent="claude" working />)
    expect(screen.queryByRole('switch')).toBeNull()
  })

  test("agents without shell commands send '!' text as a message", async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={nextPane()} agent="gemini" working={false} />)
    await user.type(box(), '!ls')
    expect(sendButton().getAttribute('aria-label')).toBe('Send')
  })

  test('sending clears the box, shows a pending bubble and posts the text', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), '  ship it  ')
    await user.click(sendButton())
    await flush()
    expect(box().value).toBe('')
    const [p] = useStore.getState().chats[`box:${pane}`].pending
    expect(p).toMatchObject({ text: 'ship it', status: 'sent' })
    expect(api.send.mock.calls).toEqual([['box', pane, 'ship it', p.id]])
  })

  test("an Android keyboard putting the sent message back doesn't bring it back as a draft", async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    const { unmount } = await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    const mine = () => socket.send.mock.calls.filter(([m]) => (m as { pane?: string }).pane === pane)
    await user.type(box(), 'change session then come back')
    await user.click(sendButton())
    await flush()
    // the keyboard was still composing: it commits its text (the whole message, or its last word) into the emptied box
    act(() => void fireEvent.change(box(), { target: { value: 'change session then come back' } }))
    act(() => void fireEvent.change(box(), { target: { value: 'back' } }))
    expect(box().value).toBe('')
    // leaving the app right after doesn't send the old text either
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    try {
      act(() => void document.dispatchEvent(new Event('visibilitychange')))
    } finally {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    }
    await act(() => Bun.sleep(500))
    expect(mine().filter(([m]) => (m as { text: string }).text)).toEqual([])
    // switching to another session and coming back: the box is still empty
    unmount()
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    expect(box().value).toBe('')
    // typing a new message works as usual
    await user.type(box(), 'next')
    expect(box().value).toBe('next')
  })

  test('a failed send keeps the bubble with the reason', async () => {
    const user = userEvent.setup()
    const pane = nextPane()
    api.send.mockImplementation(async () => {
      throw new Error('pane is gone')
    })
    await render(<Composer host="box" pane={pane} agent="claude" working={false} />)
    await user.type(box(), 'hello{Enter}')
    await flush()
    expect(useStore.getState().chats[`box:${pane}`].pending[0]).toMatchObject({ status: 'failed', error: 'pane is gone' })
  })

  test('disabled while the machine is unreachable', async () => {
    const user = userEvent.setup()
    await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} disabled />)
    await user.type(box(), 'hi')
    expect(box().value).toBe('')
    expect(sendButton().disabled).toBe(true)
  })
})

describe('a file that failed to upload', () => {
  const attach = (container: HTMLElement, ...names: string[]) =>
    act(() => void fireEvent.change(container.querySelector('input[type=file]')!, { target: { files: names.map((n) => new File([n], n, { type: 'text/plain' })) } }))
  const sent = () => api.send.mock.calls.map((c) => c[2])

  test('holds the message until it is tried again, then goes with it', async () => {
    const user = userEvent.setup()
    let down = true
    api.upload.mockImplementation(async (_h: string, _p: string | undefined, f: File) => {
      if (f.name === 'spec.txt' && down) throw new Error('disk full')
      return { name: f.name, path: `/up/${f.name}`, size: f.size, type: f.type }
    })
    const { container } = await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
    attach(container, 'spec.txt', 'notes.txt')
    await flush()
    expect(screen.getByText('disk full')).toBeTruthy()
    await user.type(box(), 'use the attached spec')
    expect(sendButton().disabled).toBe(true)
    await user.type(box(), '{Enter}')
    await flush()
    expect(sent()).toEqual([])
    expect(box().value).toBe('use the attached spec')
    expect(screen.getByLabelText('Remove spec.txt')).toBeTruthy()

    down = false
    await user.click(screen.getByLabelText('Retry spec.txt'))
    await flush()
    expect(sendButton().disabled).toBe(false)
    await user.click(sendButton())
    await flush()
    expect(sent()).toEqual(['use the attached spec\n\nAttached files:\n/up/spec.txt\n/up/notes.txt'])
    expect(screen.queryByLabelText('Remove spec.txt')).toBeNull()
    expect(screen.queryByLabelText('Remove notes.txt')).toBeNull()
  })

  test('removing it lets the rest go', async () => {
    const user = userEvent.setup()
    api.upload.mockImplementation(async (_h: string, _p: string | undefined, f: File) => {
      if (f.name === 'spec.txt') throw new Error('disk full')
      return { name: f.name, path: `/up/${f.name}`, size: f.size, type: f.type }
    })
    const { container } = await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
    attach(container, 'spec.txt', 'notes.txt')
    await flush()
    await user.type(box(), 'read these')
    expect(sendButton().disabled).toBe(true)
    await user.click(screen.getByLabelText('Remove spec.txt'))
    await user.click(sendButton())
    await flush()
    expect(sent()).toEqual(['read these\n\nAttached files:\n/up/notes.txt'])
  })
})

describe('a file on its way', () => {
  const attach = (container: HTMLElement, name: string) =>
    act(() => void fireEvent.change(container.querySelector('input[type=file]')!, { target: { files: [new File([name], name, { type: 'text/plain' })] } }))

  test('shows how far it got, says when it is reconnecting, and stops when removed', async () => {
    const user = userEvent.setup()
    let report: (f: number, waiting?: boolean) => void = () => {}
    let signal: AbortSignal | undefined
    api.upload.mockImplementation((_h: string, _p: string | undefined, _f: File, onProgress?: typeof report, s?: AbortSignal) => {
      report = onProgress!
      signal = s
      return new Promise(() => {})
    })
    const { container } = await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
    attach(container, 'video.txt')
    await flush()
    act(() => report(0.4, false))
    expect(screen.getByText('40%')).toBeTruthy()
    act(() => report(0.4, true))
    expect(screen.getByText('Reconnecting…')).toBeTruthy()
    expect(screen.queryByText('40%')).toBeNull()
    act(() => report(0.6, false))
    expect(screen.getByText('60%')).toBeTruthy()
    expect(signal!.aborted).toBe(false)
    await user.click(screen.getByLabelText('Remove video.txt'))
    expect(signal!.aborted).toBe(true)
    expect(screen.queryByText('60%')).toBeNull()
  })
})

describe('the model menu', () => {
  test('Enter picks a model and leaves the unfinished message in the box', async () => {
    const user = userEvent.setup()
    const chosen: string[] = []
    const s = { agent: 'claude', def: AGENTS.claude, efforts: [], actions: { setModel: (v: string) => chosen.push(v) } } as unknown as SessionModel
    await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} controls={<ComposerControls s={s} />} />)
    await user.type(box(), 'unfinished draft')
    await user.click(screen.getByTitle('Model'))
    // the chip doesn't take focus, so the keys still land in the text box
    expect(document.activeElement).toBe(box())
    await user.keyboard('{ArrowDown}{Enter}')
    await flush()
    expect(chosen).toHaveLength(1)
    expect(screen.queryByRole('menu')).toBeNull()
    expect(api.send.mock.calls).toEqual([])
    expect(box().value).toBe('unfinished draft')
    // Escape closes the menu only
    await user.click(screen.getByTitle('Model'))
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu')).toBeNull()
    expect(box().value).toBe('unfinished draft')
  })
})

describe('the phone app hands over keyboard images', () => {
  test('a pasteFile event becomes an attachment, and stops when the composer goes', async () => {
    const listeners: Record<string, ((e: unknown) => void)[]> = {}
    ;(window as any).Capacitor = {
      isNativePlatform: () => true,
      getPlatform: () => 'android',
      Plugins: {
        InyonDevice: {
          addListener: (e: string, f: (e: unknown) => void) => {
            ;(listeners[e] ||= []).push(f)
            return Promise.resolve({ remove: () => (listeners[e] = listeners[e]!.filter((x) => x !== f)) })
          },
        },
      },
    }
    try {
      const { unmount } = await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
      const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
      act(() => listeners.pasteFile!.forEach((f) => f({ name: 'sticker.png', type: 'image/png', data: png })))
      await flush()
      expect(screen.getByLabelText('Remove sticker.png')).toBeTruthy()
      const [, , file] = api.upload.mock.calls.at(-1) as [string, string, File]
      expect([file.name, file.type, file.size]).toEqual(['sticker.png', 'image/png', 70])
      unmount()
      await flush()
      expect(listeners.pasteFile).toEqual([])
    } finally {
      delete (window as any).Capacitor
    }
  })
})

describe('after Send', () => {
  const pointer = (coarse: boolean) => {
    const was = window.matchMedia
    window.matchMedia = ((q: string) => ({ matches: coarse && q.includes('coarse'), media: q, addEventListener() {}, removeEventListener() {} })) as any
    return () => void (window.matchMedia = was)
  }

  test('a touch screen puts the keyboard away, so the reply has the screen', async () => {
    const restore = pointer(true)
    try {
      const user = userEvent.setup()
      await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
      await user.click(box())
      await user.type(box(), 'run the tests')
      expect(document.activeElement).toBe(box())
      await user.click(sendButton())
      await flush()
      expect(box().value).toBe('')
      expect(document.activeElement).not.toBe(box())
    } finally {
      restore()
    }
  })

  test('with a keyboard the box keeps focus for the next message', async () => {
    const restore = pointer(false)
    try {
      const user = userEvent.setup()
      await render(<Composer host="box" pane={nextPane()} agent="claude" working={false} />)
      await user.click(box())
      await user.type(box(), 'run the tests')
      await user.click(sendButton())
      await flush()
      expect(document.activeElement).toBe(box())
    } finally {
      restore()
    }
  })
})
