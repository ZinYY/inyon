import { expect, test } from 'bun:test'
import { fireEvent, screen } from '@testing-library/react'
import { promptKey, type ScreenPrompt } from '@inyon/protocol'
import { PromptCard } from './PromptCard.tsx'
import { api } from '../../test/fakes.ts'
import { renderSettled as render } from '../../test/render.tsx'

test('an answer says which question it was for, so a late tap never lands on the next one', async () => {
  const prompt: ScreenPrompt = {
    body: ['Do you want to make this edit to auth.ts?'],
    options: [
      { n: 1, label: 'Yes', selected: true },
      { n: 2, label: "Yes, and don't ask again", selected: false },
      { n: 3, label: 'No', selected: false },
    ],
    multi: false,
  }
  await render(<PromptCard prompt={prompt} host="box" pane="%3" />)
  fireEvent.click(screen.getByRole('button', { name: /^1\s*Yes$/ }))
  expect(api.answer.mock.calls.at(-1)).toEqual(['box', '%3', 1, undefined, promptKey(prompt)])
})

// Claude's "Edit file" prompt, as the hub reads it off the screen (Windows paths, a wrapped line)
const editPrompt: ScreenPrompt = {
  body: [
    'Edit file',
    'src\\cart.js',
    '╌',
    '  11',
    '  12  // SAVE10 takes 10% off',
    '  14 -  if (code === \'SAVE10\') return',
    '        amount * 0.9',
    '  14 +  if (code?.toUpperCase() === \'SAVE10\') return amount * 0.9',
    '╌',
    'Do you want to make this edit to cart.js?',
  ],
  options: [
    { n: 1, label: 'Yes', selected: true },
    { n: 2, label: 'No', selected: false },
  ],
  multi: false,
}

test("an edit's diff is coloured like the Changes view, wrapped tails with their line", async () => {
  const { container } = await render(<PromptCard prompt={editPrompt} host="box" pane="%3" />)
  const lines = [...container.querySelectorAll('pre [data-diff]')]
  expect(lines.map((l) => l.getAttribute('data-diff'))).toEqual(['same', 'same', 'del', 'del', 'add'])
  expect(lines[2]!.className).toContain('bg-[var(--diff-del)]')
  expect(lines[4]!.className).toContain('bg-[var(--diff-add)]')
  expect(lines[1]!.className).not.toContain('diff-')
  expect(container.querySelector('pre .text-danger')?.textContent).toBe('-')
  expect(container.querySelector('pre .text-ok')?.textContent).toBe('+')
})

test('a command is shown as it is, without diff colours', async () => {
  const prompt: ScreenPrompt = { body: ['Bash command', '╌', '  npm test -- --grep "+1 -1"', '╌', 'Do you want to proceed?'], options: [{ n: 1, label: 'Yes', selected: true }], multi: false }
  const { container } = await render(<PromptCard prompt={prompt} host="box" pane="%3" />)
  expect(container.querySelector('[data-diff]')).toBeNull()
  expect(container.querySelector('pre')!.textContent).toBe('npm test -- --grep "+1 -1"')
})

// a plan Claude wrote line by line read as one run-on paragraph; the hub now sends whole lines
test('whole lines from the hub keep their breaks; screen-wrapped ones are joined', async () => {
  const body = ['Ready to code?', 'Plan', 'Add the passkey table', 'Move the login form']
  const options = [{ n: 1, label: 'Yes', selected: true }]
  const text = (c: HTMLElement) => (c.querySelector('.whitespace-pre-line') as HTMLElement).textContent
  const whole = await render(<PromptCard prompt={{ body, options, multi: false, wholeLines: true } as ScreenPrompt} host="box" pane="%3" />)
  expect(text(whole.container)).toBe('Plan\nAdd the passkey table\nMove the login form')
  whole.unmount()
  const wrapped = await render(<PromptCard prompt={{ body, options, multi: false }} host="box" pane="%3" />)
  expect(text(wrapped.container)).toBe('Plan Add the passkey table Move the login form')
})

test('held while the machine reconnects: nothing can be pressed', async () => {
  await render(<PromptCard prompt={editPrompt} host="box" pane="%3" disabled />)
  for (const name of [/^1\s*Yes$/, /^2\s*No$/, 'Cancel']) expect((screen.getByRole('button', { name }) as HTMLButtonElement).disabled).toBe(true)
})
