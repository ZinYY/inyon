//! Inyon's wire types: the JSON the hub, the engine and the app send each other, as Rust types
//! that read and write exactly what `packages/protocol` describes in TypeScript.
//!
//! Numbers follow one rule. A count, an id, a size or a timestamp the hub makes itself is an
//! integer; a measurement copied from an agent's own output (a percentage, a cost, a duration) is
//! a [`Num`], since the agent may send a fraction and JavaScript passes it through untouched.

mod agents;
pub mod compat;
mod js;
mod organize;
mod types;
pub mod v2;
mod versions;

pub use agents::*;
pub use js::{Num, True};
pub use organize::*;
pub use types::*;
pub use v2::{ChatState, ClientMsg2, Delta, DraftState, Keys, Progress, QueueKind, QueuedMsg, Rows, ServerMsg2, Snapshot, StreamPos, Zip};
pub use versions::*;

/// Every exported type, in the order `src/gen/index.ts` lists them. A macro rather than a list,
/// since each entry is a type: `with_types!(m)` calls `m!(A, B, …)`.
#[macro_export]
#[doc(hidden)]
macro_rules! with_types {
    ($m:ident) => {
        $m!(
            // agents.ts
            AgentKind,
            SkinTokens,
            CommandSource,
            SlashCommand,
            AgentModel,
            CatalogSource,
            ModeOption,
            AgentCatalog,
            AgentInstall,
            LatestSource,
            ReplyFont,
            Skin,
            ConfigFormat,
            ConfigFile,
            UltracodeCommands,
            AgentSet,
            AgentDef,
            // versions.ts
            ReleaseFile,
            // index.ts
            HostKind,
            HostAuth,
            HostConfig,
            HostStatus,
            AgentState,
            RateWindow,
            Credits,
            StaleLimits,
            StaleContext,
            Stale,
            AgentStatus,
            MappedBy,
            QuickOption,
            AgentPaneInfo,
            TurnFailure,
            PaneInfo,
            WindowInfo,
            SessionInfo,
            IntegrationState,
            NodeUpdateState,
            NodeUpdate,
            MachineKind,
            TmuxState,
            AccountLimits,
            HostState,
            AssistantBlock,
            PatchHunk,
            ToolResultMeta,
            NoticeLevel,
            ChatItem,
            TaskType,
            TaskStatus,
            BackgroundTask,
            SubagentStatus,
            SubagentInfo,
            ArtifactInfo,
            SessionStats,
            SendStatus,
            SendState,
            HeldPrompt,
            ChosenControls,
            PastConversation,
            LiveKind,
            LiveItem,
            ChatGoal,
            RemoteControl,
            Turn,
            ChatInfo,
            PromptOption,
            PromptTab,
            PromptInput,
            ScreenPrompt,
            ScreenState,
            ProviderVia,
            OpenCodeProvider,
            ProvidersReport,
            PhonePairCode,
            PhoneClaim,
            HubInfo,
            PhoneClaimResult,
            PairedPhone,
            CompanionScope,
            CompanionPairCode,
            CompanionClaim,
            CompanionClaimResult,
            PairedCompanion,
            CompanionMe,
            PageProgress,
            PageEntry,
            PageList,
            PageOpen,
            PageRegister,
            PageRecord,
            PagesFile,
            PageKind,
            PageMarker,
            ClientMsg,
            DraftSource,
            NotifyKind,
            // organize.rs
            ProjectColor,
            SessionMark,
            ProjectRoot,
            Project,
            ProjectGroup,
            Space,
            DelayedSend,
            SavedPrompt,
            SleepRule,
            Organize,
            ServerMsg,
            // v2.rs (/ws2)
            Zip,
            StreamPos,
            ClientMsg2,
            QueuedMsg,
            QueueKind,
            Rows,
            Keys,
            DraftState,
            Progress,
            ChatState,
            Snapshot,
            Delta,
            ServerMsg2
        )
    };
}
