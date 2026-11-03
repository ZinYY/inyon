import type { ReactElement } from 'react'
import { act, render } from '@testing-library/react'

/** Lets pending promises (the fake api answering) and their state updates finish. */
export const flush = () => act(async () => {})

/** Renders and waits for what the first effects started, such as fetching. */
export async function renderSettled(ui: ReactElement) {
  const r = render(ui)
  await flush()
  return r
}
