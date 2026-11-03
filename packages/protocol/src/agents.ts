// Everything Inyon knows about each coding agent: how to spot it, how it looks, what it can do.

export type AgentKind =
  | 'claude'
  | 'codex'
  | 'opencode'
  | 'copilot'
  | 'gemini'
  | 'antigravity'
  | 'cursor'
  | 'grok'
  | 'pi'
  | 'amp'
  | 'goose'
  | 'hermes'
  | 'kimi'
  | 'kiro'
  | 'qwen'
  | 'qoder'
  | 'codebuddy'

export interface SkinTokens {
  bg: string
  panel: string
  raised: string
  hover: string
  line: string
  lineStrong: string
  fg: string
  fg2: string
  fg3: string
  accent: string
  accentFg: string
  accentSoft: string
  accentText: string
  bubble: string
  code: string
}

export interface SlashCommand {
  name: string
  description: string
  source: 'built-in' | 'skill' | 'project' | 'user' | 'plugin'
  /** runs immediately when picked (no arguments expected) */
  instant?: boolean
}

export interface AgentModel {
  id: string
  label: string
  note?: string
  context?: number
  /** Advertised choices for this model; empty means there is no adjustable effort. */
  efforts?: string[]
  /** The level the CLI uses when none is chosen (Codex's footer then just says "default"; Claude Code
   *  names none until one is picked). */
  defaultEffort?: string
  /** CLI model ids for a family whose effort is encoded in the model name. */
  variants?: Record<string, string>
}

export interface AgentCatalog {
  models: AgentModel[]
  source: 'cli' | 'fallback'
  updatedAt: number
  error?: string
  /** the agent's own list of modes when it can be asked (OpenCode's primary agents, custom ones included) */
  modes?: { id: string; label: string }[]
}

export interface AgentDef {
  kind: AgentKind
  /**
   * How to install it, from its official instructions: its own installer when it has one, and its
   * npm package, used when there's no installer or the installer fails. Then how to sign in.
   */
  install: { cmd?: string; npm?: string; docs: string }
  /** where its newest version is published (its npm package); none means Inyon can't tell */
  latest?: { npm: string }
  /** the agent's own update command, from its --help; npm installs are updated with npm instead */
  update?: string
  login: string
  /** a command that prints the version */
  version: string
  /** files that exist once the user has signed in (any of them) */
  authFiles: string[]
  name: string
  vendor: string
  /** process names / argv patterns that identify the agent inside a pane */
  comm: string[]
  args: RegExp[]
  launch: string
  /** reply typography */
  replyFont: 'serif' | 'sans' | 'mono'
  spinner: string[]
  skin: { light: SkinTokens; dark: SkinTokens }
  /** conversation readable as chat (otherwise terminal-first) */
  chat: boolean
  /** The running CLI supports direct ! shell commands. */
  shell?: boolean
  /** context: window size in tokens, used when the agent hasn't reported it */
  models: AgentModel[]
  efforts: string[]
  modes: { id: string; label: string }[]
  commands: SlashCommand[]
  /**
   * The commands that open the agent's own list of past conversations in its terminal. Inyon
   * lists and switches them itself for these agents; none means it can't for this one.
   */
  conversations?: string[]
  configFiles: { path: string; label: string; format: 'json' | 'toml' | 'md' }[]
  /** how to set things from the outside */
  set: {
    /** `effort` is the session's current level, for agents that name both in one id */
    model?: (id: string, effort?: string) => string
    effort?: (level: string) => string
    goal?: (text: string) => string
    clearGoal?: string
    compact?: string
    clear?: string
    rc?: string
    /** Claude Code's ultracode switch: the command that turns it on or off for the session */
    ultracode?: (on: boolean) => string
  }
}

const shared = { bubbleLight: '#EFEDE4' }

const CLAUDE_COMMANDS: [string, string, boolean?][] = [
  ['/add-dir', 'Add a new working directory'],
  ['/advisor', 'Let Claude consult a stronger model at key moments', true],
  ['/agents', 'Manage subagents', true],
  ['/artifacts', 'Browse your published and shared artifacts', true],
  ['/autocompact', 'Set how full the context gets before auto-summarizing'],
  ['/background', 'Send this session to the background and free the terminal', true],
  ['/branch', 'Create a branch of the current conversation at this point', true],
  ['/btw', 'Ask a quick side question without interrupting'],
  ['/cd', 'Move this session to a new working directory'],
  ['/clear', 'Start a new session with empty context', true],
  ['/color', 'Set the prompt bar color for this session'],
  ['/compact', 'Free up context by summarizing the conversation so far'],
  ['/config', 'Open settings', true],
  ['/context', 'Visualize current context usage', true],
  ['/copy', "Copy Claude's last response"],
  ['/diff', 'Toggle the diff panel showing uncommitted changes', true],
  ['/doctor', 'Health-check your Claude Code setup', true],
  ['/effort', 'Set effort level for model usage'],
  ['/export', 'Export the current conversation'],
  ['/fast', 'Toggle fast mode', true],
  ['/focus', 'Toggle focus view', true],
  ['/fork', 'Copy this conversation into a new background session', true],
  ['/goal', 'Set a goal Claude checks before stopping'],
  ['/help', 'Show help and available commands', true],
  ['/hooks', 'View hook configurations', true],
  ['/init', 'Create a CLAUDE.md with codebase documentation', true],
  ['/insights', 'Report analyzing your Claude Code sessions', true],
  ['/list-agents', 'List subagents and sessions you can message', true],
  ['/loop', 'Run a prompt or command on a recurring interval'],
  ['/mcp', 'Manage MCP servers', true],
  ['/memory', 'Edit CLAUDE.md files and memory settings', true],
  ['/model', 'Set the AI model'],
  ['/output-style', 'List output styles or switch to one'],
  ['/permissions', 'Manage allow and deny tool permission rules', true],
  ['/plan', 'Enable plan mode or view the current plan', true],
  ['/plugin', 'Manage Claude Code plugins', true],
  ['/recap', 'Generate a one-line session recap now', true],
  ['/release-notes', 'View release notes', true],
  ['/reload-plugins', 'Activate pending plugin changes', true],
  ['/reload-skills', 'Pick up skills changed on disk', true],
  ['/remote-control', 'Control this session from your phone or claude.ai/code', true],
  ['/rename', 'Rename the current conversation'],
  ['/resume', 'Resume a previous conversation', true],
  ['/review', 'Review a pull request'],
  ['/rewind', 'Restore code and/or conversation to an earlier point', true],
  ['/schedule', 'Create or run scheduled cloud agents'],
  ['/security-review', 'Security review of pending changes', true],
  ['/simplify', 'Review changed code for cleanups', true],
  ['/skills', 'List available skills', true],
  ['/status', 'Version, model, account and connectivity', true],
  ['/statusline', "Set up Claude Code's status line"],
  ['/subtask', 'Send a subagent off with your full context'],
  ['/tasks', 'View and manage everything running in the background', true],
  ['/teleport', 'Send this session to the cloud, or resume one', true],
  ['/theme', 'Change the theme', true],
  ['/usage', 'Session cost, plan usage and activity stats', true],
  ['/usage-credits', 'Configure usage credits', true],
  ['/workflows', 'Browse running and completed workflows', true],
]

const CODEX_COMMANDS: [string, string, boolean?][] = [
  ['/model', 'Choose model and reasoning effort', true],
  ['/permissions', 'Choose what Codex is allowed to do', true],
  ['/plan', 'Switch to Plan mode'],
  ['/goal', 'Set or view the goal for a long-running task'],
  ['/new', 'Start a new chat', true],
  ['/rename', 'Rename the current thread'],
  ['/resume', 'Resume a saved chat', true],
  ['/fork', 'Fork the current chat', true],
  ['/side', 'Start a side conversation in an ephemeral fork'],
  ['/compact', 'Summarize the conversation to free up context', true],
  ['/recap', 'Summarize the current conversation now', true],
  ['/review', 'Review your current changes and find issues', true],
  ['/diff', 'Show the git diff, including untracked files', true],
  ['/mention', 'Mention a file'],
  ['/init', 'Create an AGENTS.md file with instructions', true],
  ['/status', 'Session configuration and token usage', true],
  ['/usage', 'Account usage, or use a usage limit reset', true],
  ['/ps', 'List background terminals', true],
  ['/stop', 'Stop all background terminals', true],
  ['/mcp', 'List MCP tools', true],
  ['/skills', 'Use skills for specific tasks', true],
  ['/memories', 'Configure memory use and generation', true],
  ['/hooks', 'View and manage lifecycle hooks', true],
  ['/plugins', 'Browse plugins', true],
  ['/agents', 'Open the agent command center', true],
  ['/subagents', "Switch between this session's subagents", true],
  ['/worktree', 'Start or continue in a new worktree', true],
  ['/cd', 'Change the working directory'],
  ['/pwd', 'Show the working directory', true],
  ['/copy', 'Copy the last response', true],
  ['/export', 'Export the conversation as markdown', true],
  ['/approve', 'Approve one retry of a recent auto-review denial', true],
  ['/experimental', 'Toggle experimental features', true],
  ['/archive', 'Archive this session', true],
  ['/delete', 'Permanently delete this session', true],
  ['/clear', 'Clear the terminal and start a new chat', true],
  ['/feedback', 'Send logs to maintainers', true],
  ['/logout', 'Log out of Codex', true],
  ['/exit', 'Exit Codex', true],
]

// OpenCode 1.19, as its "/" popup lists them inside a conversation
const OPENCODE_COMMANDS: [string, string, boolean?][] = [
  ['/agents', 'Switch agent', true],
  ['/compact', 'Compact session', true],
  ['/connect', 'Connect provider', true],
  ['/copy', 'Copy session transcript', true],
  ['/debug', 'View debug info', true],
  ['/diff', 'Open diff viewer', true],
  ['/editor', 'Open editor', true],
  ['/exit', 'Exit the app', true],
  ['/export', 'Export session transcript', true],
  ['/fork', 'Fork session', true],
  ['/help', 'Help', true],
  ['/init', 'guided AGENTS.md setup', true],
  ['/mcps', 'Toggle MCPs', true],
  ['/models', 'Switch model', true],
  ['/move', 'Move to another project dir', true],
  ['/new', 'New session', true],
  ['/redo', 'Redo the last undone message', true],
  ['/rename', 'Rename session'],
  ['/review', 'review changes [commit|branch|pr], defaults to uncommitted'],
  ['/sessions', 'Switch session', true],
  ['/skills', 'Skills', true],
  ['/status', 'View status', true],
  ['/themes', 'Switch theme', true],
  ['/thinking', 'Collapse thinking', true],
  ['/timeline', 'Jump to message', true],
  ['/timestamps', 'Show timestamps', true],
  ['/undo', 'Undo previous message', true],
  ['/update', 'Update OpenCode', true],
  ['/variants', 'Switch model variant', true],
]

const GEMINI_COMMANDS: [string, string, boolean?][] = [
  ['/model', 'Choose a Gemini model', true],
  ['/compress', 'Replace the context with a summary', true],
  ['/clear', 'Clear the screen and conversation', true],
  ['/memory', 'Manage GEMINI.md memory'],
  ['/stats', 'Session statistics and token usage', true],
  ['/tools', 'List available tools', true],
  ['/mcp', 'List MCP servers and tools', true],
  ['/chat', 'Save, resume and list conversation checkpoints'],
  ['/restore', 'Restore files to before a tool ran'],
  ['/help', 'Show help', true],
  ['/quit', 'Exit Gemini CLI', true],
]

// Antigravity CLI 1.3 (`agy`), from its /help. Aliases in the description, as /help shows them.
const ANTIGRAVITY_COMMANDS: [string, string, boolean?][] = [
  ['/add-dir', 'Add a directory to the workspace'],
  ['/agents', 'List available custom agents', true],
  ['/artifact', 'View and review artifacts', true],
  ['/btw', 'Ask a side question without interrupting the current task'],
  ['/changelog', 'Show release notes and changes', true],
  ['/clear', 'Clear conversation and start a new one (also /new)', true],
  ['/codesearch', 'Search code in the workspace (also /cs, /search)'],
  ['/config', 'Open settings panel (also /settings)', true],
  ['/context', 'Visualize current context usage', true],
  ['/copy', "Copy the last response; '/copy n' for the n-th, '/copy btw' for the side answer"],
  ['/credits', 'Show remaining G1 credits and purchase link', true],
  ['/diff', 'View uncommitted changes and per-turn diffs', true],
  ['/effort', 'Set the reasoning effort'],
  ['/exit', 'Exit the CLI (also /quit)', true],
  ['/feedback', 'Submit feedback to improve the agent', true],
  ['/fork', 'Branch the conversation at this point (also /branch)'],
  ['/help', 'Show available commands and keybindings', true],
  ['/hooks', 'Manage hook configurations for tool events', true],
  ['/keybindings', 'Set custom keybindings', true],
  ['/logout', 'Log out', true],
  ['/mcp', 'Manage MCP servers', true],
  ['/model', 'Set a model, or run a single prompt on another model'],
  ['/open', 'Open a file or view opened/edited files'],
  ['/permissions', 'Manage tool permissions', true],
  ['/plugin', 'Manage, discover, install, uninstall, enable and disable plugins'],
  ['/remote-control', 'Turn on remote control for this session (off to disable)'],
  ['/rename', 'Rename the current conversation'],
  ['/resume', 'Browse and resume past conversations (also /switch)', true],
  ['/rewind', 'Rewind the conversation to a previous message (also /undo)', true],
  ['/skills', 'List available skills', true],
  ['/statusline', 'Toggle the status line', true],
  ['/tasks', 'View background tasks', true],
  ['/title', 'Toggle custom terminal window title', true],
  ['/usage', 'View model quota usage (also /quota)', true],
  ['/voice', 'Dictate a prompt using your microphone (also /record)', true],
  ['/goal', 'Run until the specified goal is completely finished'],
  ['/schedule', 'Run an instruction on a recurring schedule or as a one-time timer'],
  ['/browser', 'Invoke a browser agent for web tasks'],
  ['/plan', 'Plan carefully before executing a task'],
  ['/grill-me', 'Interview me to align on a plan'],
  ['/teamwork-preview', 'Invoke a team of agents to tackle large projects'],
  ['/learn', 'Capture reusable skills or rules from recent work'],
  ['/boost', 'Invoke the Boost multi-agent orchestrator for complex tasks'],
  ['/agy-customizations', 'Guide to the Antigravity customization system'],
  ['/antigravity-guide', 'Guide, quick reference and sitemap for Google Antigravity'],
]

const cmds = (list: [string, string, boolean?][]): SlashCommand[] =>
  list.map(([name, description, instant]) => ({ name, description, source: 'built-in', instant }))

const NEUTRAL = {
  light: { bg: '#FAFAF9', panel: '#FFFFFF', raised: '#F3F3F1', hover: '#EDEDEA', line: '#E2E1DD', lineStrong: '#CFCEC8', fg: '#1A1A19', fg2: '#4A4945', fg3: '#5F5E59', bubble: '#EFEFEC', code: '#F3F3F1' },
  dark: { bg: '#0F0F10', panel: '#18181A', raised: '#141416', hover: '#1E1E21', line: '#2A2A2E', lineStrong: '#38383D', fg: '#EDEDEE', fg2: '#BDBDC2', fg3: '#8F8F96', bubble: '#1F1F22', code: '#121214' },
}

type Accent = Pick<SkinTokens, 'accent' | 'accentFg' | 'accentSoft' | 'accentText'>
const accent = (a: string, fg: string, soft: string, text: string): Accent => ({ accent: a, accentFg: fg, accentSoft: soft, accentText: text })
/** A quiet neutral skin with the agent's brand colour as its accent. */
const accentSkin = (light: Accent, dark: Accent): AgentDef['skin'] => ({ light: { ...NEUTRAL.light, ...light }, dark: { ...NEUTRAL.dark, ...dark } })

/** The program as its process names it, or as the script node, bun or python runs. */
const program = (name: string) => new RegExp(`^(?:\\S*/)?(?:(?:node|bun|python3?)\\s+(?:\\S*/)?)?${name}(?:\\s|$)`)

/**
 * An agent whose conversation stays in its terminal: Inyon finds it in a pane, starts, installs
 * and updates it, and tells from its screen when it's busy or asking something.
 */
const terminalAgent = (
  d: Pick<AgentDef, 'kind' | 'install' | 'latest' | 'update' | 'login' | 'version' | 'authFiles' | 'name' | 'vendor' | 'comm' | 'args' | 'launch' | 'skin'> &
    Partial<Pick<AgentDef, 'configFiles'>>,
): AgentDef => ({
  replyFont: 'sans',
  spinner: ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
  chat: false,
  models: [],
  efforts: [],
  modes: [],
  commands: [],
  configFiles: [],
  set: {},
  ...d,
})

export const AGENTS: Record<AgentKind, AgentDef> = {
  claude: {
    kind: 'claude',
    install: { cmd: 'curl -fsSL https://claude.ai/install.sh | bash', docs: 'https://docs.claude.com/en/docs/claude-code/setup' },
    latest: { npm: '@anthropic-ai/claude-code' },
    update: 'claude update',
    login: 'claude /login',
    version: 'claude --version',
    authFiles: ['~/.claude/.credentials.json', '~/.claude.json'],
    name: 'Claude Code',
    vendor: 'Anthropic',
    comm: ['claude'],
    args: [/(^|\/)claude(\s|$)/, /@anthropic-ai\/claude-code\//],
    launch: 'claude',
    replyFont: 'serif',
    spinner: ['·', '✢', '✳', '✶', '✻', '✽', '✻', '✶', '✳', '✢'],
    skin: {
      light: {
        bg: '#FAF9F5', panel: '#FFFFFF', raised: '#F4F2EC', hover: '#F0EEE7', line: '#E8E5DC', lineStrong: '#D9D5C9',
        fg: '#1F1E1D', fg2: '#5C5B56', fg3: '#6A6862', accent: '#B85A38', accentFg: '#FFFFFF', accentSoft: '#F5E6DE',
        accentText: '#A8502F', bubble: shared.bubbleLight, code: '#F4F2EC',
      },
      dark: {
        bg: '#262624', panel: '#30302E', raised: '#2A2927', hover: '#33322F', line: '#3B3A37', lineStrong: '#4A4844',
        fg: '#F4F3EE', fg2: '#C3C1B8', fg3: '#A6A39A', accent: '#D97757', accentFg: '#1F1E1D', accentSoft: '#3E2C25',
        accentText: '#E8957A', bubble: '#1A1918', code: '#1F1E1D',
      },
    },
    chat: true,
    shell: true,
    models: [
      { id: 'opus', label: 'Opus 5.5', note: 'Default · 1M context', context: 1_000_000, defaultEffort: 'high' },
      { id: 'sonnet', label: 'Sonnet 5.5', note: 'Faster, cheaper', defaultEffort: 'medium' },
      { id: 'haiku', label: 'Haiku 5.5', note: 'Quick edits' },
      { id: 'claude-fable-5-1', label: 'Fable 5.1', note: 'Most capable' },
    ],
    efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
    modes: [
      { id: 'default', label: 'Ask each time' },
      { id: 'acceptEdits', label: 'Accept edits' },
      { id: 'plan', label: 'Plan' },
      { id: 'auto', label: 'Auto' },
      { id: 'bypassPermissions', label: 'Bypass' },
    ],
    commands: cmds(CLAUDE_COMMANDS),
    conversations: ['/resume'],
    configFiles: [
      { path: '~/.claude/settings.json', label: 'User settings', format: 'json' },
      { path: '~/.claude/CLAUDE.md', label: 'User memory', format: 'md' },
      { path: '{cwd}/.claude/settings.json', label: 'Project settings', format: 'json' },
      { path: '{cwd}/.claude/settings.local.json', label: 'Local project settings', format: 'json' },
      { path: '{cwd}/CLAUDE.md', label: 'Project memory', format: 'md' },
    ],
    set: {
      model: (id) => `/model ${id}`,
      effort: (l) => `/effort ${l}`,
      goal: (t) => `/goal ${t}`,
      clearGoal: '/goal clear',
      compact: '/compact',
      clear: '/clear',
      rc: '/remote-control',
      ultracode: (on) => `/effort ultracode ${on ? 'on' : 'off'}`,
    },
  },
  codex: {
    kind: 'codex',
    install: { cmd: 'curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh', npm: '@openai/codex', docs: 'https://developers.openai.com/codex/cli' },
    latest: { npm: '@openai/codex' },
    update: 'codex update',
    login: 'codex login',
    version: 'codex --version',
    authFiles: ['~/.codex/auth.json'],
    name: 'Codex',
    vendor: 'OpenAI',
    comm: ['codex'],
    args: [/(^|\/)codex(\s|$)/, /@openai\/codex/],
    launch: 'codex',
    replyFont: 'sans',
    spinner: ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
    skin: {
      light: {
        bg: '#FFFFFF', panel: '#FFFFFF', raised: '#F7F7F8', hover: '#F2F2F3', line: '#E5E5E5', lineStrong: '#D4D4D4',
        fg: '#0D0D0D', fg2: '#4D4D4D', fg3: '#6E6E6E', accent: '#2F6FEB', accentFg: '#FFFFFF', accentSoft: '#E8F0FE',
        accentText: '#1F5FD6', bubble: '#F3F3F3', code: '#F7F7F8',
      },
      dark: {
        bg: '#0D0D0D', panel: '#171717', raised: '#141414', hover: '#1C1C1C', line: '#2A2A2A', lineStrong: '#3A3A3A',
        fg: '#ECECEC', fg2: '#B8B8B8', fg3: '#8F8F8F', accent: '#63A8F8', accentFg: '#00002E', accentSoft: '#14233A',
        accentText: '#8CC0FF', bubble: '#1F1F1F', code: '#111111',
      },
    },
    chat: true,
    shell: true,
    // what /model lists (Codex 0.162, ~/.codex/models_cache.json)
    models: [
      { id: 'gpt-6.1-sol', label: 'GPT-6.1-Sol', note: 'Latest workhorse for coding and everyday work', context: 272_000 },
      { id: 'gpt-6-astra', label: 'GPT-6-Astra', note: 'Frontier intelligence for the hardest work', context: 272_000 },
      { id: 'gpt-6-sol', label: 'GPT-6-Sol', note: 'Previous generation workhorse', context: 272_000 },
      { id: 'gpt-6-luna', label: 'GPT-6-Luna', note: 'Fast and affordable', context: 272_000, efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
      { id: 'gpt-5.6-sol', label: 'GPT-5.6-Sol', note: 'Older generation workhorse', context: 272_000 },
      { id: 'gpt-5.6-terra', label: 'GPT-5.6-Terra', note: 'Older balanced model', context: 272_000 },
      { id: 'gpt-5.6-luna', label: 'GPT-5.6-Luna', note: 'Older fast model', context: 272_000, efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
    ],
    efforts: ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'],
    // permission presets (/permissions), plus Plan mode (Shift+Tab)
    modes: [
      { id: 'default', label: 'Ask for approval' },
      { id: 'auto-review', label: 'Approve for me' },
      { id: 'read-only', label: 'Read only' },
      { id: 'full-access', label: 'Full access' },
      { id: 'plan', label: 'Plan' },
    ],
    commands: cmds(CODEX_COMMANDS),
    conversations: ['/resume'],
    configFiles: [
      { path: '~/.codex/config.toml', label: 'Config', format: 'toml' },
      { path: '~/.codex/AGENTS.md', label: 'Global instructions', format: 'md' },
      { path: '{cwd}/AGENTS.md', label: 'Project instructions', format: 'md' },
    ],
    set: {
      // the hub turns these into /model picker steps (Codex's /model takes no arguments)
      model: (id) => `/model ${id}`,
      effort: (level) => `/effort ${level}`,
      goal: (text) => `/goal ${text}`,
      clearGoal: '/goal clear',
      compact: '/compact',
      clear: '/new',
    },
  },
  opencode: {
    kind: 'opencode',
    install: { cmd: 'curl -fsSL https://opencode.ai/install | bash', docs: 'https://opencode.ai/docs' },
    latest: { npm: 'opencode-ai' },
    update: 'opencode upgrade',
    login: 'opencode auth login',
    version: 'opencode --version',
    authFiles: ['~/.local/share/opencode/auth.json'],
    name: 'OpenCode',
    vendor: 'sst',
    comm: ['opencode'],
    args: [/(^|\/)opencode(\s|$)/, /opencode-ai/],
    launch: 'opencode --hostname 127.0.0.1 --port 0',
    replyFont: 'mono',
    spinner: ['▁', '▃', '▄', '▅', '▆', '▇', '▆', '▅', '▄', '▃'],
    skin: {
      light: {
        bg: '#FBFAF8', panel: '#FFFFFF', raised: '#F4F2EF', hover: '#EFEDE9', line: '#E4E1DB', lineStrong: '#D3CFC6',
        fg: '#14171C', fg2: '#4A5260', fg3: '#5F6774', accent: '#B45A18', accentFg: '#FFFFFF', accentSoft: '#FBE9DA',
        accentText: '#A4500F', bubble: '#F1EEE9', code: '#F4F2EF',
      },
      dark: {
        bg: '#0B0D10', panel: '#151820', raised: '#11141A', hover: '#1A1E26', line: '#262B33', lineStrong: '#333944',
        fg: '#E6E8EB', fg2: '#B4BAC4', fg3: '#8B93A0', accent: '#F5A35C', accentFg: '#1A1206', accentSoft: '#2E2116',
        accentText: '#F7B57C', bubble: '#1B1F27', code: '#0F1216',
      },
    },
    chat: true,
    shell: true,
    models: [],
    efforts: [],
    // the built-in agents; a machine's own list (custom agents too) replaces this when it's asked
    modes: [
      { id: 'build', label: 'Build' },
      { id: 'plan', label: 'Plan' },
    ],
    commands: cmds(OPENCODE_COMMANDS),
    conversations: ['/sessions'],
    configFiles: [
      { path: '~/.config/opencode/opencode.json', label: 'Config', format: 'json' },
      { path: '{cwd}/opencode.json', label: 'Project config', format: 'json' },
      { path: '{cwd}/AGENTS.md', label: 'Project instructions', format: 'md' },
    ],
    set: { model: (id) => `/model ${id}`, effort: (level) => `/effort ${level}`, compact: '/compact', clear: '/new' },
  },
  copilot: {
    kind: 'copilot',
    install: { cmd: 'curl -fsSL https://gh.io/copilot-install | bash', npm: '@github/copilot', docs: 'https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli' },
    latest: { npm: '@github/copilot' },
    update: 'copilot update',
    login: 'copilot login --device-code',
    version: 'copilot --version',
    authFiles: ['~/.copilot/config.json'],
    name: 'GitHub Copilot',
    vendor: 'GitHub',
    comm: ['copilot'],
    args: [/(^|\/)copilot(\s|$)/, /@github\/copilot/],
    launch: 'copilot',
    replyFont: 'sans',
    spinner: ['◐', '◓', '◑', '◒'],
    skin: {
      light: {
        bg: '#FBFAFE', panel: '#FFFFFF', raised: '#F4F2FB', hover: '#EFEDF8', line: '#E3E0F0', lineStrong: '#D2CEE6',
        fg: '#17151F', fg2: '#4E4A61', fg3: '#6B6782', accent: '#6E5CE6', accentFg: '#FFFFFF', accentSoft: '#ECE8FD',
        accentText: '#5A48D1', bubble: '#F0EEF8', code: '#F4F2FB',
      },
      dark: {
        bg: '#0E0D14', panel: '#18171F', raised: '#14131B', hover: '#1E1D27', line: '#2A2836', lineStrong: '#383547',
        fg: '#ECEBF5', fg2: '#BEBBD0', fg3: '#918EA6', accent: '#8B7CF6', accentFg: '#14121F', accentSoft: '#231F3D',
        accentText: '#A99EFA', bubble: '#201F2B', code: '#111018',
      },
    },
    chat: true,
    shell: true,
    models: [{ id: 'auto', label: 'Auto' }],
    efforts: [],
    modes: [{ id: 'default', label: 'Manual approval' }, { id: 'allow-all', label: 'Full access' }],
    commands: cmds([['/model', 'Choose a model', true], ['/permissions', 'Change permissions', true], ['/plan', 'Plan before implementing', true], ['/autopilot', 'Run autonomously', true], ['/usage', 'Show usage', true], ['/context', 'Show context', true], ['/compact', 'Compact conversation', true], ['/login', 'Sign in', true], ['/clear', 'Clear conversation', true], ['/agent', 'Choose an agent', true], ['/skills', 'Manage skills', true], ['/mcp', 'Manage MCP servers', true], ['/plugin', 'Manage plugins', true], ['/tasks', 'View tasks', true], ['/diff', 'View changes', true], ['/review', 'Review changes'], ['/security-review', 'Review security'], ['/lsp', 'Manage language servers', true], ['/sandbox', 'Manage sandbox', true], ['/add-dir', 'Add a workspace directory'], ['/list-dirs', 'List workspace directories', true], ['/cwd', 'Change working directory'], ['/resume', 'Resume a session', true], ['/rename', 'Rename this session'], ['/fork', 'Fork this session'], ['/copy', 'Copy a response', true], ['/rewind', 'Rewind conversation', true], ['/help', 'Show help', true], ['/version', 'Show version', true], ['/memory', 'Manage memory', true], ['/instructions', 'Show instructions', true], ['/limits', 'Show limits', true], ['/user', 'Show account', true], ['/new', 'Start a session', true], ['/settings', 'Manage settings', true], ['/subagents', 'Manage subagents', true]]),
    conversations: ['/resume', '/switch'],
    configFiles: [{ path: '{cwd}/AGENTS.md', label: 'Project instructions', format: 'md' }],
    set: { model: (id) => `/model ${id}`, compact: '/compact', clear: '/clear' },
  },
  gemini: {
    kind: 'gemini',
    install: { npm: '@google/gemini-cli', docs: 'https://github.com/google-gemini/gemini-cli' },
    latest: { npm: '@google/gemini-cli' },
    login: 'gemini',
    version: 'gemini --version',
    authFiles: ['~/.gemini/oauth_creds.json', '~/.gemini/google_accounts.json'],
    name: 'Gemini CLI (legacy)',
    vendor: 'Google',
    comm: ['gemini'],
    args: [/(^|\/)gemini(\s|$)/, /@google\/gemini-cli/],
    launch: 'gemini',
    replyFont: 'sans',
    spinner: ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'],
    skin: {
      light: {
        bg: '#F8FAFD', panel: '#FFFFFF', raised: '#F0F4F9', hover: '#E9EEF6', line: '#DDE3EA', lineStrong: '#C7D0DB',
        fg: '#1F1F1F', fg2: '#444746', fg3: '#5F6368', accent: '#1A73E8', accentFg: '#FFFFFF', accentSoft: '#E8F0FE',
        accentText: '#1558B0', bubble: '#E9EEF6', code: '#F0F4F9',
      },
      dark: {
        bg: '#131314', panel: '#1E1F20', raised: '#1A1B1C', hover: '#252629', line: '#2E3033', lineStrong: '#3C4043',
        fg: '#E3E3E3', fg2: '#C4C7C5', fg3: '#9AA0A6', accent: '#8AB4F8', accentFg: '#131314', accentSoft: '#1F2A3C',
        accentText: '#A8C7FA', bubble: '#282A2C', code: '#18191A',
      },
    },
    chat: true,
    models: [
      { id: 'gemini-2.5-pro', label: 'Gemini 2.5 Pro' },
      { id: 'gemini-2.5-flash', label: 'Gemini 2.5 Flash' },
    ],
    efforts: [],
    modes: [],
    commands: cmds(GEMINI_COMMANDS),
    configFiles: [
      { path: '~/.gemini/settings.json', label: 'Settings', format: 'json' },
      { path: '~/.gemini/GEMINI.md', label: 'Global memory', format: 'md' },
      { path: '{cwd}/GEMINI.md', label: 'Project memory', format: 'md' },
    ],
    set: { compact: '/compress', clear: '/clear' },
  },
  antigravity: {
    kind: 'antigravity',
    install: { cmd: 'curl -fsSL https://antigravity.google/cli/install.sh | bash', docs: 'https://antigravity.google/docs/cli/install' },
    update: 'agy update',
    login: 'agy',
    version: 'agy --version',
    authFiles: ['~/.gemini/antigravity-cli/antigravity-oauth-token'],
    name: 'Antigravity CLI',
    vendor: 'Google',
    comm: ['agy'],
    args: [/(^|\/)agy(\s|$)/],
    launch: 'agy',
    replyFont: 'sans',
    spinner: ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'],
    skin: {
      light: {
        bg: '#F8FAFD', panel: '#FFFFFF', raised: '#F0F4F9', hover: '#E9EEF6', line: '#DDE3EA', lineStrong: '#C7D0DB',
        fg: '#1F1F1F', fg2: '#444746', fg3: '#5F6368', accent: '#1A73E8', accentFg: '#FFFFFF', accentSoft: '#E8F0FE',
        accentText: '#1A5FD1', bubble: '#E9EEF6', code: '#F0F4F9',
      },
      dark: {
        bg: '#131314', panel: '#1E1F20', raised: '#1A1B1C', hover: '#252629', line: '#2E3033', lineStrong: '#3C4043',
        fg: '#E3E3E3', fg2: '#C4C7C5', fg3: '#9AA0A6', accent: '#8AB4F8', accentFg: '#131314', accentSoft: '#1F2A3C',
        accentText: '#A8C7FA', bubble: '#282A2C', code: '#18191A',
      },
    },
    chat: true,
    // the picker lists model families; effort is set on its own (/effort), so the full id is model + effort
    models: [
      { id: 'gemini-3.8-flash', label: 'Gemini 3.8 Flash', note: 'Default · 1M context', context: 1_048_576 },
      { id: 'gemini-3.7-flash', label: 'Gemini 3.7 Flash', context: 1_048_576 },
      { id: 'gemini-3.6-flash', label: 'Gemini 3.6 Flash', context: 1_048_576 },
      { id: 'gemini-3.1-pro', label: 'Gemini 3.1 Pro', note: 'Low or high effort', context: 1_048_576 },
      { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', note: 'Thinking' },
      { id: 'claude-opus-4-6-thinking', label: 'Claude Opus 4.6', note: 'Thinking' },
      { id: 'gpt-oss-120b-medium', label: 'GPT-OSS 120B' },
    ],
    efforts: ['low', 'medium', 'high'],
    modes: [
      { id: 'default', label: 'Ask each time' },
      { id: 'acceptEdits', label: 'Accept edits' },
      { id: 'plan', label: 'Plan' },
    ],
    commands: cmds(ANTIGRAVITY_COMMANDS),
    conversations: ['/resume', '/switch'],
    configFiles: [
      { path: '~/.gemini/antigravity-cli/settings.json', label: 'Settings', format: 'json' },
      { path: '~/.gemini/config/mcp_config.json', label: 'MCP servers', format: 'json' },
      { path: '~/.gemini/config/hooks.json', label: 'Hooks', format: 'json' },
      { path: '{cwd}/.agents/hooks.json', label: 'Project hooks', format: 'json' },
      { path: '{cwd}/AGENTS.md', label: 'Project instructions', format: 'md' },
    ],
    set: {
      // Gemini Flash takes an effort suffix; the others are selected as listed
      model: (id, effort) => `/model ${agyModelId(id, effort)}`,
      effort: (l) => `/effort ${l}`,
      goal: (t) => `/goal ${t}`,
      clear: '/clear',
      rc: '/remote-control',
    },
  },
  cursor: terminalAgent({
    kind: 'cursor',
    install: { cmd: 'curl https://cursor.com/install -fsS | bash', docs: 'https://cursor.com/docs/cli/overview' },
    update: 'cursor-agent update',
    login: 'cursor-agent login',
    version: 'cursor-agent --version',
    authFiles: ['~/.config/cursor/auth.json'],
    name: 'Cursor CLI',
    vendor: 'Cursor',
    comm: ['cursor-agent'],
    // its launcher runs Node under the name it was started by (agent or cursor-agent)
    args: [program('cursor-agent'), /\/cursor-agent\/versions\/[^/\s]+\/index\.js/],
    launch: 'cursor-agent',
    skin: accentSkin(accent('#1B1913', '#FFFFFF', '#ECEBE7', '#1B1913'), accent('#EDECEC', '#14120B', '#2A2924', '#EDECEC')),
    configFiles: [{ path: '~/.cursor/cli-config.json', label: 'Config', format: 'json' }],
  }),
  grok: terminalAgent({
    kind: 'grok',
    install: { cmd: 'curl -fsSL https://x.ai/cli/install.sh | bash', docs: 'https://docs.x.ai/build/overview' },
    update: 'grok update',
    login: 'grok login --device-auth',
    version: 'grok --version',
    authFiles: ['~/.grok/auth.json'],
    name: 'Grok Build',
    vendor: 'xAI',
    comm: ['grok'],
    args: [program('grok')],
    launch: 'grok',
    skin: accentSkin(accent('#0A0A0A', '#FFFFFF', '#EDEDED', '#0A0A0A'), accent('#F5F5F5', '#0A0A0A', '#262626', '#F5F5F5')),
    configFiles: [{ path: '~/.grok/config.toml', label: 'Config', format: 'toml' }],
  }),
  pi: terminalAgent({
    kind: 'pi',
    install: { cmd: 'curl -fsSL https://pi.dev/install.sh | sh', npm: '@earendil-works/pi-coding-agent', docs: 'https://pi.dev' },
    latest: { npm: '@earendil-works/pi-coding-agent' },
    update: 'pi update',
    // signs in from inside: /login
    login: 'pi',
    version: 'pi --version',
    authFiles: ['~/.pi/agent/auth.json'],
    name: 'Pi',
    vendor: 'Earendil',
    comm: ['pi'],
    args: [program('pi')],
    launch: 'pi',
    skin: accentSkin(accent('#B4483A', '#FFFFFF', '#FBE6E2', '#A63F32'), accent('#F09082', '#1F1A19', '#33221F', '#F4A99D')),
  }),
  amp: terminalAgent({
    kind: 'amp',
    install: { cmd: 'curl -fsSL https://ampcode.com/install.sh | bash', npm: '@ampcode/cli', docs: 'https://ampcode.com/manual' },
    // it updates itself; an npm install is updated with npm
    latest: { npm: '@ampcode/cli' },
    login: 'amp login',
    version: 'amp --version',
    authFiles: ['~/.local/share/amp/secrets.json'],
    name: 'Amp',
    vendor: 'Amp',
    comm: ['amp'],
    args: [program('amp'), /@ampcode\/cli\//],
    launch: 'amp',
    skin: accentSkin(accent('#C8352A', '#FFFFFF', '#FDE5E2', '#B8301F'), accent('#F36B5F', '#1A0F0E', '#35201D', '#F68D83')),
  }),
  goose: terminalAgent({
    kind: 'goose',
    install: {
      cmd: 'curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | CONFIGURE=false bash',
      docs: 'https://goose-docs.ai/docs/getting-started/installation',
    },
    update: 'goose update',
    // picks a provider and its key
    login: 'goose configure',
    version: 'goose --version',
    authFiles: ['~/.config/goose/config.yaml'],
    name: 'goose',
    vendor: 'Agentic AI Foundation',
    comm: ['goose'],
    args: [program('goose')],
    launch: 'goose',
    skin: accentSkin(accent('#101010', '#FFFFFF', '#ECECEC', '#101010'), accent('#F2F2F2', '#101010', '#262626', '#F2F2F2')),
  }),
  hermes: terminalAgent({
    kind: 'hermes',
    install: { cmd: 'curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash', docs: 'https://hermes-agent.nousresearch.com/docs/getting-started/installation' },
    update: 'hermes update',
    // its setup wizard: provider, sign-in or key, model
    login: 'hermes setup',
    version: 'hermes --version',
    authFiles: ['~/.hermes/auth.json', '~/.hermes/.env'],
    name: 'Hermes Agent',
    vendor: 'Nous Research',
    comm: ['hermes'],
    args: [program('hermes')],
    launch: 'hermes',
    skin: accentSkin(accent('#8B6508', '#FFFFFF', '#F7EFD6', '#7A5807'), accent('#FFD700', '#14120A', '#2F2A12', '#FFE14D')),
  }),
  kimi: terminalAgent({
    kind: 'kimi',
    install: { cmd: 'curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash', npm: '@moonshot-ai/kimi-code', docs: 'https://moonshotai.github.io/kimi-code/en/guides/getting-started' },
    latest: { npm: '@moonshot-ai/kimi-code' },
    update: 'kimi upgrade',
    login: 'kimi login',
    version: 'kimi --version',
    authFiles: ['~/.kimi-code/credentials/kimi-code.json'],
    name: 'Kimi Code',
    vendor: 'Moonshot AI',
    comm: ['kimi'],
    args: [program('kimi'), /@moonshot-ai\/kimi-code\//],
    launch: 'kimi',
    skin: accentSkin(accent('#0F6FDB', '#FFFFFF', '#E3F0FF', '#0B63C7'), accent('#4DA3FF', '#0A1220', '#14243A', '#7DBBFF')),
    configFiles: [{ path: '~/.kimi-code/config.toml', label: 'Config', format: 'toml' }],
  }),
  kiro: terminalAgent({
    kind: 'kiro',
    install: { cmd: 'curl -fsSL https://cli.kiro.dev/install | bash', docs: 'https://kiro.dev/docs/cli/' },
    update: 'kiro-cli update --non-interactive',
    login: 'kiro-cli login --use-device-flow',
    version: 'kiro-cli --version',
    authFiles: ['~/.local/share/kiro-cli/data.sqlite3'],
    name: 'Kiro CLI',
    vendor: 'AWS',
    // its chat runs as a separate kiro-cli-chat process
    comm: ['kiro-cli', 'kiro-cli-chat'],
    args: [program('kiro-cli'), program('kiro-cli-chat')],
    launch: 'kiro-cli',
    skin: accentSkin(accent('#8A3FF5', '#FFFFFF', '#F0E7FF', '#7A30E6'), accent('#B489FF', '#160A2A', '#2A1C45', '#C6A6FF')),
  }),
  qwen: terminalAgent({
    kind: 'qwen',
    install: {
      cmd: 'curl -fsSL https://qwen-code-assets.oss-cn-hangzhou.aliyuncs.com/installation/install-qwen-standalone.sh | bash',
      npm: '@qwen-code/qwen-code',
      docs: 'https://github.com/QwenLM/qwen-code#installation',
    },
    latest: { npm: '@qwen-code/qwen-code' },
    update: 'qwen update',
    // signs in from inside: /auth
    login: 'qwen',
    version: 'qwen --version',
    authFiles: ['~/.qwen/settings.json'],
    name: 'Qwen Code',
    vendor: 'Alibaba',
    comm: ['qwen'],
    args: [program('qwen'), /qwen-code\/lib\/cli-entry\.js/, /@qwen-code\/qwen-code\//],
    launch: 'qwen',
    skin: accentSkin(accent('#615CED', '#FFFFFF', '#ECEBFD', '#5550E0'), accent('#9A97F5', '#11103A', '#24234A', '#B3B1F8')),
    configFiles: [
      { path: '~/.qwen/settings.json', label: 'Settings', format: 'json' },
      { path: '{cwd}/QWEN.md', label: 'Project memory', format: 'md' },
    ],
  }),
  qoder: terminalAgent({
    kind: 'qoder',
    install: { cmd: 'curl -fsSL https://qoder.com/install | bash', npm: '@qoder-ai/qodercli', docs: 'https://docs.qoder.com/cli/installation' },
    latest: { npm: '@qoder-ai/qodercli' },
    update: 'qodercli update',
    login: 'qodercli login',
    version: 'qodercli --version',
    authFiles: ['~/.qoder/.auth/user'],
    name: 'Qoder CLI',
    vendor: 'Qoder',
    comm: ['qodercli', 'qoder'],
    args: [program('qodercli'), program('qoder')],
    launch: 'qodercli',
    skin: accentSkin(accent('#15803D', '#FFFFFF', '#DFF7E7', '#13773A'), accent('#2ADB5C', '#0A1A0F', '#13301C', '#5EE585')),
  }),
  codebuddy: terminalAgent({
    kind: 'codebuddy',
    install: { npm: '@tencent-ai/codebuddy-code', docs: 'https://www.codebuddy.ai/docs/cli/installation' },
    latest: { npm: '@tencent-ai/codebuddy-code' },
    update: 'codebuddy update',
    // signs in on first run
    login: 'codebuddy',
    version: 'codebuddy --version',
    authFiles: ['~/.codebuddy/.credentials.json'],
    name: 'CodeBuddy Code',
    vendor: 'Tencent',
    comm: ['codebuddy'],
    args: [program('codebuddy'), /@tencent-ai\/codebuddy-code\//],
    launch: 'codebuddy',
    skin: accentSkin(accent('#6C4DFF', '#FFFFFF', '#EEEAFF', '#5B3BF0'), accent('#9C86FF', '#140E33', '#251E4A', '#B4A3FF')),
    configFiles: [
      { path: '~/.codebuddy/settings.json', label: 'Settings', format: 'json' },
      { path: '{cwd}/CODEBUDDY.md', label: 'Project memory', format: 'md' },
    ],
  }),
}

/**
 * agy names a model and its effort together ("gemini-3.8-flash-high"). A bare family keeps the
 * session's effort when the model has it (Pro has only low and high), else gets high, agy's default.
 */
export const agyModelId = (id: string, effort?: string) => {
  const m = id.match(/^gemini-[\d.]+-(flash|pro)$/)
  if (!m) return id
  const ok = m[1] === 'pro' ? ['low', 'high'] : ['low', 'medium', 'high']
  return `${id}-${effort && ok.includes(effort) ? effort : 'high'}`
}

export const AGENT_KINDS = Object.keys(AGENTS) as AgentKind[]

/**
 * The agent catalog this app knows, sent to the hub as `?catalog=`: from 2 the app falls back on
 * its own for a kind it doesn't have, so the hub shows it every agent (crates/inyon-protocol's
 * compat.rs). Older apps send nothing and get the newer agents as plain terminals.
 */
export const AGENT_CATALOG = 2

const unknown = new Map<string, AgentDef>()

/**
 * An agent's entry, or for a kind this build doesn't have (a newer hub's) a plain terminal agent
 * under that name, so an app never breaks on an agent added after it was built.
 */
export function agentDef(kind: string): AgentDef {
  const known = (AGENTS as Partial<Record<string, AgentDef>>)[kind]
  if (known) return known
  let def = unknown.get(kind)
  if (!def) {
    def = terminalAgent({
      kind: kind as AgentKind,
      install: { docs: '' },
      login: kind,
      version: `${kind} --version`,
      authFiles: [],
      name: kind.charAt(0).toUpperCase() + kind.slice(1),
      vendor: '',
      comm: [],
      args: [],
      launch: kind,
      skin: accentSkin(accent('#1A1A19', '#FFFFFF', '#ECECE9', '#1A1A19'), accent('#EDEDEE', '#0F0F10', '#26262A', '#EDEDEE')),
    })
    unknown.set(kind, def)
  }
  return def
}

/** Whether this build knows the agent (has its mark, skin and controls). */
export const knownAgent = (kind: string): kind is AgentKind => Object.hasOwn(AGENTS, kind)

/**
 * Which agent (if any) a process is. `cmd` alone is too generic, so it needs a matching argv. Only
 * the agents read as chat: the terminal-first ones are found by the Rust hub alone, since this
 * TypeScript core is on its way out.
 */
export function detectAgent(comm: string, args: string): AgentKind | undefined {
  for (const a of Object.values(AGENTS).filter((a) => a.chat)) {
    if (a.comm.includes(comm)) return a.kind
    if (/^(sh|bash|zsh|-?zsh|-?bash|fish|tmux)$/.test(comm)) continue
    if (a.args.some((r) => r.test(args))) return a.kind
  }
  return undefined
}
