export type TerminalRenderer = 'webgl' | 'dom'

let current: TerminalRenderer = 'dom'

/** How the terminal is drawn (components/terminal/webgl.ts decides), for Settings to show. */
export const getRenderer = () => current
export const setRenderer = (r: TerminalRenderer) => {
  current = r
}
