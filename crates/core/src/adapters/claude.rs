use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::adapters::{
    JsonlCursor, ParsedSource, SourceCheckpoint, SourcePathKind, ToolAdapter,
    decode_checkpoint_state, local_day, normalize_path, parse_timestamp, parse_value,
    stable_event_id,
};
use crate::model::{EventKind, NormalizedEvent, TOOL_CLAUDE_CODE};

#[derive(Debug, Default)]
pub struct ClaudeCodeAdapter;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ClaudeState {
    pending_user_turns: Vec<PendingUserTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingUserTurn {
    line_offset: u64,
    occurred_at_utc: DateTime<Utc>,
    session_key: String,
    project_path: Option<String>,
}

impl ToolAdapter for ClaudeCodeAdapter {
    fn tool_id(&self) -> &'static str {
        TOOL_CLAUDE_CODE
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        match home {
            Some(home) => vec![home.join(".claude").join("projects")],
            None => Vec::new(),
        }
    }

    fn classify_path(&self, path: &Path) -> SourcePathKind {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            return SourcePathKind::Ignore;
        }
        if !self
            .source_roots()
            .iter()
            .any(|root| path.starts_with(root))
        {
            return SourcePathKind::Ignore;
        }
        if path
            .components()
            .any(|component| component.as_os_str() == "subagents")
        {
            return SourcePathKind::Ignore;
        }
        SourcePathKind::SourceFile(path.to_path_buf())
    }

    fn discover_sources(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for root in self.source_roots() {
            if !root.exists() {
                continue;
            }

            for entry in WalkDir::new(root).follow_links(false) {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if !entry.file_type().is_file() {
                    continue;
                }
                if entry.path().extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if entry
                    .path()
                    .components()
                    .any(|component| component.as_os_str() == "subagents")
                {
                    continue;
                }
                files.push(entry.into_path());
            }
        }
        files.sort();
        Ok(files)
    }

    fn parse_incremental(
        &self,
        source_path: &Path,
        checkpoint: Option<&SourceCheckpoint>,
    ) -> Result<ParsedSource> {
        let (offset, mut state) = decode_checkpoint_state::<ClaudeState>(source_path, checkpoint)?;
        let mut cursor = JsonlCursor::open(source_path, offset)?;
        let mut events = Vec::new();

        while let Some((line_offset, line)) = cursor.next_line()? {
            let Some(value) = parse_value(&line) else {
                continue;
            };
            if value
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(timestamp) = parse_timestamp(&value, "/timestamp") else {
                continue;
            };
            let top_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let session_key = value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| source_path.to_string_lossy().into_owned());
            let project_path = value
                .get("cwd")
                .and_then(Value::as_str)
                .map(|cwd| normalize_path(Path::new(cwd), None));

            match top_type {
                "user" => {
                    if is_real_user_prompt(&value) {
                        state.pending_user_turns.push(PendingUserTurn {
                            line_offset,
                            occurred_at_utc: timestamp,
                            session_key,
                            project_path,
                        });
                    }
                }
                "assistant" => {
                    let pending = std::mem::take(&mut state.pending_user_turns);
                    for pending_turn in pending {
                        events.push(NormalizedEvent {
                            id: stable_event_id(
                                self.tool_id(),
                                Some(&pending_turn.session_key),
                                source_path,
                                pending_turn.line_offset,
                                "conversation_turn",
                            ),
                            tool_id: self.tool_id().to_owned(),
                            session_key: pending_turn.session_key,
                            project_path: pending_turn.project_path,
                            occurred_at_utc: pending_turn.occurred_at_utc,
                            local_day: local_day(pending_turn.occurred_at_utc),
                            kind: EventKind::ConversationTurn,
                            value: 1,
                            unit_key: None,
                            source_path: source_path.to_string_lossy().into_owned(),
                        });
                    }

                    let usage = value.pointer("/message/usage").unwrap_or(&Value::Null);
                    if let Some(input_tokens) = usage.get("input_tokens").and_then(Value::as_u64)
                        && input_tokens > 0
                    {
                        events.push(NormalizedEvent {
                            id: stable_event_id(
                                self.tool_id(),
                                Some(&session_key),
                                source_path,
                                line_offset,
                                "input_tokens",
                            ),
                            tool_id: self.tool_id().to_owned(),
                            session_key: session_key.clone(),
                            project_path: project_path.clone(),
                            occurred_at_utc: timestamp,
                            local_day: local_day(timestamp),
                            kind: EventKind::InputTokens,
                            value: input_tokens,
                            unit_key: None,
                            source_path: source_path.to_string_lossy().into_owned(),
                        });
                    }
                    if let Some(output_tokens) = usage.get("output_tokens").and_then(Value::as_u64)
                        && output_tokens > 0
                    {
                        events.push(NormalizedEvent {
                            id: stable_event_id(
                                self.tool_id(),
                                Some(&session_key),
                                source_path,
                                line_offset,
                                "output_tokens",
                            ),
                            tool_id: self.tool_id().to_owned(),
                            session_key: session_key.clone(),
                            project_path: project_path.clone(),
                            occurred_at_utc: timestamp,
                            local_day: local_day(timestamp),
                            kind: EventKind::OutputTokens,
                            value: output_tokens,
                            unit_key: None,
                            source_path: source_path.to_string_lossy().into_owned(),
                        });
                    }

                    let mut file_paths = BTreeSet::new();
                    if let Some(content) =
                        value.pointer("/message/content").and_then(Value::as_array)
                    {
                        for item in content {
                            if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                                continue;
                            }
                            let Some(tool_name) = item.get("name").and_then(Value::as_str) else {
                                continue;
                            };
                            if !matches!(tool_name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit")
                            {
                                continue;
                            }
                            let Some(raw_path) =
                                item.pointer("/input/file_path").and_then(Value::as_str)
                            else {
                                continue;
                            };
                            file_paths.insert(normalize_path(
                                Path::new(raw_path),
                                project_path.as_deref(),
                            ));
                        }
                    }

                    for file_path in file_paths {
                        events.push(NormalizedEvent {
                            id: stable_event_id(
                                self.tool_id(),
                                Some(&session_key),
                                source_path,
                                line_offset,
                                &format!("file_edit:{file_path}"),
                            ),
                            tool_id: self.tool_id().to_owned(),
                            session_key: session_key.clone(),
                            project_path: project_path.clone(),
                            occurred_at_utc: timestamp,
                            local_day: local_day(timestamp),
                            kind: EventKind::FileEdit,
                            value: 1,
                            unit_key: Some(file_path),
                            source_path: source_path.to_string_lossy().into_owned(),
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(ParsedSource {
            events,
            checkpoint: SourceCheckpoint {
                offset: cursor.offset(),
                state_json: serde_json::to_value(state)?,
            },
        })
    }
}

fn is_real_user_prompt(entry: &Value) -> bool {
    if entry.get("toolUseResult").is_some() || entry.get("sourceToolAssistantUUID").is_some() {
        return false;
    }
    if entry
        .get("isMeta")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }

    let text = extract_message_text(entry.get("message").unwrap_or(&Value::Null));
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("[Request interrupted by user") || trimmed.starts_with("[Image:") {
        return false;
    }
    true
}

fn extract_message_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(extract_message_text)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return text.to_owned();
            }
            if let Some(content) = map.get("content") {
                return extract_message_text(content);
            }
            String::new()
        }
        _ => String::new(),
    }
}
