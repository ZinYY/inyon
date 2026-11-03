import { agentDef } from '@inyon/protocol'

/** What starting an agent from a pane's terminal leads to: its chat view, or (for a terminal-first agent) this terminal. */
export const startingNote = (kind: string) => (agentDef(kind).chat ? 'The chat view opens as soon as it is running.' : 'It opens here in the terminal.')
