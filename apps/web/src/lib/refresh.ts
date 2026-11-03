import { api } from './api.ts'
import { socket } from './socket.ts'
import { toast } from './store.ts'

/** Pull to refresh: reconnect if the link dropped, and have every machine read again. */
export async function refreshAll() {
  if (!socket.up) socket.reconnect()
  try {
    await api.refresh()
  } catch (e) {
    toast({ title: 'Could not refresh', body: (e as Error).message, tone: 'error' })
  }
}
