// The hand-written types and the ones generated from the Rust crate (gen/index.ts) must describe
// the same JSON. Each line fails the protocol package's typecheck when the two sides differ in any
// way, so a field changed in only one place can't slip through.
import type * as G from './gen/index.ts'
import type * as H from './index.ts'

// Identical types, not just mutually assignable ones: those miss an optional field present on one
// side only, and most fields here are optional
type Same<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false
type Check<T extends true> = T

export type Checks = [
  // agents.ts
  Check<Same<G.AgentKind, H.AgentKind>>,
  Check<Same<G.SkinTokens, H.SkinTokens>>,
  Check<Same<G.SlashCommand, H.SlashCommand>>,
  Check<Same<G.AgentModel, H.AgentModel>>,
  Check<Same<G.AgentCatalog, H.AgentCatalog>>,
  // the TypeScript catalog holds regular expressions and functions where the generated one holds
  // their sources and templates (scripts/catalog.ts turns one into the other)
  Check<Same<Omit<G.AgentDef, 'args' | 'set'>, Omit<H.AgentDef, 'args' | 'set'>>>,
  Check<Same<keyof G.AgentSet, keyof H.AgentDef['set']>>,
  Check<Same<G.AgentDef['args'], string[]>>,
  // versions.ts
  Check<Same<G.ReleaseFile, H.ReleaseFile>>,
  // index.ts
  Check<Same<G.HostKind, H.HostKind>>,
  Check<Same<G.HostConfig, H.HostConfig>>,
  Check<Same<G.HostStatus, H.HostStatus>>,
  Check<Same<G.AgentState, H.AgentState>>,
  Check<Same<G.RateWindow, H.RateWindow>>,
  Check<Same<G.AgentStatus, H.AgentStatus>>,
  Check<Same<G.AgentPaneInfo, H.AgentPaneInfo>>,
  Check<Same<G.TurnFailure, H.TurnFailure>>,
  Check<Same<G.PaneInfo, H.PaneInfo>>,
  Check<Same<G.WindowInfo, H.WindowInfo>>,
  Check<Same<G.SessionInfo, H.SessionInfo>>,
  Check<Same<G.IntegrationState, H.IntegrationState>>,
  Check<Same<G.NodeUpdate, H.NodeUpdate>>,
  Check<Same<G.MachineKind, H.MachineKind>>,
  Check<Same<G.AccountLimits, H.AccountLimits>>,
  Check<Same<G.HostState, H.HostState>>,
  Check<Same<G.AssistantBlock, H.AssistantBlock>>,
  Check<Same<G.ToolResultMeta, H.ToolResultMeta>>,
  Check<Same<G.ChatItem, H.ChatItem>>,
  Check<Same<G.BackgroundTask, H.BackgroundTask>>,
  Check<Same<G.SubagentInfo, H.SubagentInfo>>,
  Check<Same<G.ArtifactInfo, H.ArtifactInfo>>,
  Check<Same<G.SessionStats, H.SessionStats>>,
  Check<Same<G.SendState, H.SendState>>,
  Check<Same<G.HeldPrompt, H.HeldPrompt>>,
  Check<Same<G.ChosenControls, H.ChosenControls>>,
  Check<Same<G.PastConversation, H.PastConversation>>,
  Check<Same<G.LiveItem, H.LiveItem>>,
  Check<Same<G.ChatInfo, H.ChatInfo>>,
  Check<Same<G.PromptOption, H.PromptOption>>,
  Check<Same<G.ScreenPrompt, H.ScreenPrompt>>,
  Check<Same<G.ScreenState, H.ScreenState>>,
  Check<Same<G.OpenCodeProvider, H.OpenCodeProvider>>,
  Check<Same<G.ProvidersReport, H.ProvidersReport>>,
  Check<Same<G.PhonePairCode, H.PhonePairCode>>,
  Check<Same<G.PhoneClaim, H.PhoneClaim>>,
  Check<Same<G.PhoneClaimResult, H.PhoneClaimResult>>,
  Check<Same<G.PairedPhone, H.PairedPhone>>,
  Check<Same<G.CompanionScope, H.CompanionScope>>,
  Check<Same<G.CompanionPairCode, H.CompanionPairCode>>,
  Check<Same<G.CompanionClaim, H.CompanionClaim>>,
  Check<Same<G.CompanionClaimResult, H.CompanionClaimResult>>,
  Check<Same<G.PairedCompanion, H.PairedCompanion>>,
  Check<Same<G.CompanionMe, H.CompanionMe>>,
  Check<Same<G.PageProgress, H.PageProgress>>,
  Check<Same<G.PageEntry, H.PageEntry>>,
  Check<Same<G.PageList, H.PageList>>,
  Check<Same<G.PageOpen, H.PageOpen>>,
  Check<Same<G.PageRegister, H.PageRegister>>,
  Check<Same<G.PageRecord, H.PageRecord>>,
  Check<Same<G.PagesFile, H.PagesFile>>,
  Check<Same<G.PageKind, H.PageKind>>,
  Check<Same<G.PageMarker, H.PageMarker>>,
  Check<Same<G.ClientMsg, H.ClientMsg>>,
  Check<Same<G.ProjectColor, H.ProjectColor>>,
  Check<Same<G.SessionMark, H.SessionMark>>,
  Check<Same<G.ProjectRoot, H.ProjectRoot>>,
  Check<Same<G.Project, H.Project>>,
  Check<Same<G.ProjectGroup, H.ProjectGroup>>,
  Check<Same<G.Space, H.Space>>,
  Check<Same<G.DelayedSend, H.DelayedSend>>,
  Check<Same<G.SavedPrompt, H.SavedPrompt>>,
  Check<Same<G.SleepRule, H.SleepRule>>,
  Check<Same<G.Organize, H.Organize>>,
  Check<Same<G.ServerMsg, H.ServerMsg>>,
]
