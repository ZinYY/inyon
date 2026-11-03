import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test'
import { copyText } from './clipboard.ts'

// happy-dom has no execCommand and no isSecureContext; each test says what the browser offers
const writeText = mock(async (_text: string) => {})
const execCommand = mock((_cmd: string) => true)
const realClipboard = navigator.clipboard
let copied: string | undefined

beforeEach(() => {
  writeText.mockReset()
  writeText.mockImplementation(async () => {})
  execCommand.mockReset()
  // what execCommand would copy: the selected textarea's value
  execCommand.mockImplementation(() => ((copied = (document.activeElement as HTMLTextAreaElement | null)?.value ?? document.querySelector('textarea')?.value), true))
  copied = undefined
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
  ;(document as any).execCommand = execCommand
})

afterEach(() => {
  Object.defineProperty(navigator, 'clipboard', { value: realClipboard, configurable: true })
  delete (window as any).isSecureContext
  delete (document as any).execCommand
})

const secure = (on: boolean) => Object.defineProperty(window, 'isSecureContext', { value: on, configurable: true })

describe('copyText', () => {
  test('uses the clipboard API on HTTPS', async () => {
    secure(true)
    expect(await copyText('hello')).toBe(true)
    expect(writeText).toHaveBeenCalledWith('hello')
    expect(execCommand).not.toHaveBeenCalled()
  })

  test('falls back on plain HTTP, leaving no textarea behind', async () => {
    secure(false)
    expect(await copyText('over http')).toBe(true)
    expect(writeText).not.toHaveBeenCalled()
    expect(execCommand).toHaveBeenCalledWith('copy')
    expect(copied).toBe('over http')
    expect(document.querySelector('textarea')).toBeNull()
  })

  test('falls back when the clipboard API refuses', async () => {
    secure(true)
    writeText.mockImplementation(async () => {
      throw new Error('denied')
    })
    expect(await copyText('x')).toBe(true)
    expect(execCommand).toHaveBeenCalled()
  })

  test('reports failure when nothing works', async () => {
    secure(false)
    execCommand.mockImplementation(() => false)
    expect(await copyText('x')).toBe(false)
    execCommand.mockImplementation(() => {
      throw new Error('nope')
    })
    expect(await copyText('x')).toBe(false)
    expect(document.querySelector('textarea')).toBeNull()
  })

  test('gives focus back to what had it', async () => {
    secure(false)
    const input = document.createElement('input')
    document.body.appendChild(input)
    input.focus()
    await copyText('x')
    expect(document.activeElement).toBe(input)
    input.remove()
  })
})
