import { expect, test } from 'bun:test'
import { fireEvent, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { SessionHeader } from './SessionHeader.tsx'
import { useSession } from './useSession.ts'
import { useStore } from '../../lib/store.ts'
import { api } from '../../test/fakes.ts'
import { agentPane, host } from '../../test/fixtures.ts'
import { renderSettled, flush } from '../../test/render.tsx'

function Header() {
  const s = useSession('box', '%1')
  return <SessionHeader s={s} view="chat" setView={() => {}} panelOpen={false} togglePanel={() => {}} openSheet={() => {}} />
}

test('the three-dot menu reloads the chat without clearing or restarting the agent', async () => {
  useStore.setState({ hosts: [host('box', { panes: [agentPane('%1', 'codex')] })] })
  await renderSettled(<MemoryRouter><Header /></MemoryRouter>)
  fireEvent.click(screen.getAllByRole('button', { name: 'More' }).at(-1)!)
  fireEvent.click(screen.getByText('Reload full chat'))
  await flush()
  expect(api.reloadChat).toHaveBeenCalledWith('box', '%1')
  expect(api.send).not.toHaveBeenCalled()
  expect(api.restart).not.toHaveBeenCalled()
})
