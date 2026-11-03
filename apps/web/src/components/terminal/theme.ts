import type { ITheme } from '@xterm/xterm'

// The terminal's palette and type, shared by the web app's TerminalView and the new app's terminal
// (apps/app/term), so both draw a pane the same way.

export const LIGHT: ITheme = {
  background: '#e9e8e4', foreground: '#1d1c1a', cursor: '#1d1c1a', cursorAccent: '#e9e8e4',
  selectionBackground: '#1d1c1a30',
  black: '#1f1e1d', red: '#b5453b', green: '#4f7a3a', yellow: '#9a6a13', blue: '#2f5f9e', magenta: '#8a4b9e', cyan: '#2d7a7a', white: '#d9d5c9',
  brightBlack: '#8f8d86', brightRed: '#c96442', brightGreen: '#5f8f48', brightYellow: '#b7791f', brightBlue: '#4a78b8', brightMagenta: '#a060b5', brightCyan: '#3b9090', brightWhite: '#f0eee6',
}
export const DARK: ITheme = {
  background: '#1b1a18', foreground: '#f1f0ec', cursor: '#f1f0ec', cursorAccent: '#1b1a18',
  selectionBackground: '#f1f0ec38',
  black: '#1f1e1d', red: '#ec7a6f', green: '#a5c98a', yellow: '#e3b46b', blue: '#8fb6e8', magenta: '#c5a3e3', cyan: '#7cc4c4', white: '#c3c1b8',
  brightBlack: '#6e6c66', brightRed: '#f09a8f', brightGreen: '#bddba6', brightYellow: '#f0cb8d', brightBlue: '#aecbf0', brightMagenta: '#d7bdf0', brightCyan: '#9ed8d8', brightWhite: '#f4f3ee',
}

/** The system's monospace font, then the symbols it may lack (symbols.css) as the last resort. */
export const TERMINAL_FONT = "'JetBrains Mono Variable', ui-monospace, Menlo, monospace, 'Inyon Symbols'"

/**
 * Text at least this far from its background. Agents pick colours for the terminal they expect:
 * a light theme's syntax colours, 256-colour greys or ANSI black, drawn on this dark ground (or
 * the reverse), came out dark on dark, as parts of Claude's diffs did over psmux. xterm nudges
 * such text lighter or darker until it reads; dim text gets half the ratio, so it still looks dim.
 */
export const MIN_CONTRAST = 4.5
