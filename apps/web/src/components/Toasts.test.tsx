import { expect, test } from 'bun:test'
import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { Toasts } from './Toasts.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'
import { toast, useStore } from '../lib/store.ts'

const wrapper = (t: { title: string }) => screen.getByText(t.title).closest('[data-key]') as HTMLElement

test('a toast drops in, and goes out through a fade instead of vanishing', async () => {
  const user = userEvent.setup()
  render(<MemoryRouter><Toasts /></MemoryRouter>)
  act(() => toast({ title: 'Saved', tone: 'success' }))
  expect(wrapper({ title: 'Saved' }).className).toContain('animate-toast-in')
  await user.click(screen.getByRole('button', { name: 'Dismiss' }))
  // gone from the store at once, still on screen fading for a moment
  expect(useStore.getState().toasts).toEqual([])
  expect(wrapper({ title: 'Saved' }).className).toContain('leave')
  await act(() => new Promise((r) => setTimeout(r, LEAVE_MS + 20)))
  expect(screen.queryByText('Saved')).toBeNull()
})

test('one that times out fades too, in its own place, and the others stay in order', async () => {
  render(<MemoryRouter><Toasts /></MemoryRouter>)
  act(() => {
    toast({ title: 'First', tone: 'info' })
    toast({ title: 'Second', tone: 'info' })
    toast({ title: 'Third', tone: 'info' })
  })
  act(() => useStore.getState().dismiss(useStore.getState().toasts[1].id))
  const titles = () => [...document.querySelectorAll('[data-key]')].map((el) => el.textContent?.replace('Dismiss', ''))
  expect(titles()).toEqual(['First', 'Second', 'Third'])
  expect(wrapper({ title: 'Second' }).className).toContain('leave')
  await act(() => new Promise((r) => setTimeout(r, LEAVE_MS + 20)))
  expect(titles()).toEqual(['First', 'Third'])
})
