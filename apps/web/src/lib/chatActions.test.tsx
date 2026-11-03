import { expect, mock, test } from 'bun:test'
import { useContext } from 'react'
import { render } from '@testing-library/react'
import { ChatActionsContext, type ChatActions } from './chatActions.ts'

function Probe({ seen }: { seen: (a: ChatActions) => void }) {
  seen(useContext(ChatActionsContext))
  return null
}

test('outside a session view the actions do nothing, without throwing', () => {
  let actions: ChatActions | undefined
  render(<Probe seen={(a) => (actions = a)} />)
  expect(() => actions!.openSubagent('t1')).not.toThrow()
  expect(() => actions!.openTask({ id: 'x', type: 'shell', description: '', status: 'running' })).not.toThrow()
})

test('a session view provides its own', () => {
  const value = { openSubagent: mock(() => {}), openTask: mock(() => {}) }
  let actions: ChatActions | undefined
  render(
    <ChatActionsContext.Provider value={value}>
      <Probe seen={(a) => (actions = a)} />
    </ChatActionsContext.Provider>,
  )
  actions!.openSubagent('t1')
  expect(value.openSubagent).toHaveBeenCalledWith('t1')
})
