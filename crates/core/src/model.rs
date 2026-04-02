use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

pub const TOOL_CODEX: &str = "codex";
pub const TOOL_CLAUDE_CODE: &str = "claude-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    ConversationTurn,
    InputTokens,
    OutputTokens,
    FileEdit,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConversationTurn => "conversation_turn",
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
            Self::FileEdit => "file_edit",
        }
    }
}

impl Display for EventKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EventKind {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "conversation_turn" => Ok(Self::ConversationTurn),
            "input_tokens" => Ok(Self::InputTokens),
            "output_tokens" => Ok(Self::OutputTokens),
            "file_edit" => Ok(Self::FileEdit),
            _ => Err("unknown event kind"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedEvent {
    pub id: String,
    pub tool_id: String,
    pub session_key: String,
    pub project_path: Option<String>,
    pub occurred_at_utc: DateTime<Utc>,
    pub local_day: NaiveDate,
    pub kind: EventKind,
    pub value: u64,
    pub unit_key: Option<String>,
    pub source_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuestKind {
    ActiveProjects,
    ConversationTurns,
    InputTokens,
    OutputTokens,
    EditedFiles,
}

impl QuestKind {
    pub const ALL: [QuestKind; 5] = [
        QuestKind::ActiveProjects,
        QuestKind::ConversationTurns,
        QuestKind::InputTokens,
        QuestKind::OutputTokens,
        QuestKind::EditedFiles,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveProjects => "active_projects",
            Self::ConversationTurns => "conversation_turns",
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
            Self::EditedFiles => "edited_files",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ActiveProjects => "Active Projects",
            Self::ConversationTurns => "Conversation Turns",
            Self::InputTokens => "Input Tokens",
            Self::OutputTokens => "Output Tokens",
            Self::EditedFiles => "Edited Files",
        }
    }

    pub fn unit_label(self) -> &'static str {
        match self {
            Self::ActiveProjects => "projects",
            Self::ConversationTurns => "turns",
            Self::InputTokens | Self::OutputTokens => "tokens",
            Self::EditedFiles => "files",
        }
    }
}

impl Display for QuestKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QuestKind {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active_projects" => Ok(Self::ActiveProjects),
            "conversation_turns" => Ok(Self::ConversationTurns),
            "input_tokens" => Ok(Self::InputTokens),
            "output_tokens" => Ok(Self::OutputTokens),
            "edited_files" => Ok(Self::EditedFiles),
            _ => Err("unknown quest kind"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuestDifficulty {
    Easy,
    Normal,
    Hard,
}

impl QuestDifficulty {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Hard => "Hard",
        }
    }
}

impl Display for QuestDifficulty {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QuestDifficulty {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "easy" => Ok(Self::Easy),
            "normal" => Ok(Self::Normal),
            "hard" => Ok(Self::Hard),
            _ => Err("unknown quest difficulty"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestContribution {
    pub tool_id: String,
    pub progress: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyQuest {
    pub day: NaiveDate,
    pub slot: usize,
    pub kind: QuestKind,
    pub difficulty: QuestDifficulty,
    pub threshold: u64,
    pub progress_total: u64,
    pub progress_by_tool: BTreeMap<String, u64>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub completed_by_tool_id: Option<String>,
    pub completion_event_id: Option<String>,
}

impl DailyQuest {
    pub fn is_completed(&self) -> bool {
        self.completed_at_utc.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRecord {
    pub day: NaiveDate,
    pub completed_quests: usize,
    pub total_quests: usize,
    pub all_completed: bool,
    pub closing_streak: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodayView {
    pub today: NaiveDate,
    pub quests: Vec<DailyQuest>,
    pub record: DailyRecord,
    pub next_reset_at: DateTime<Utc>,
    pub service_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryDay {
    pub record: DailyRecord,
    pub quests: Vec<DailyQuest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsView {
    pub database_path: String,
    pub adapter_sources: Vec<AdapterDiagnostics>,
    pub checkpoint_count: usize,
    pub event_count: usize,
    pub last_sync_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterDiagnostics {
    pub tool_id: String,
    pub roots: Vec<String>,
    pub discovered_files: usize,
    pub discovery_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub diagnostics: DiagnosticsView,
    pub notifier_supported: bool,
    pub service_supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationTestKind {
    Quest,
    AllClear,
    Reminder,
    Reset,
}
