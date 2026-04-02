use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::adapters::{
    JsonlCursor, ParsedSource, SourceCheckpoint, SourcePathKind, ToolAdapter,
    decode_checkpoint_state, local_day, normalize_path, parse_timestamp, parse_value,
    stable_event_id,
};
use crate::model::{EventKind, NormalizedEvent, TOOL_CODEX};

#[derive(Debug, Default)]
pub struct CodexAdapter;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CodexState {
    session_key: Option<String>,
    project_path: Option<String>,
    pending_user_turns: Vec<PendingUserTurn>,
    last_input_total: Option<u64>,
    last_output_total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingUserTurn {
    line_offset: u64,
    occurred_at_utc: DateTime<Utc>,
}

impl ToolAdapter for CodexAdapter {
    fn tool_id(&self) -> &'static str {
        TOOL_CODEX
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        match home {
            Some(home) => vec![
                home.join(".codex").join("sessions"),
                home.join(".codex").join("archived_sessions"),
            ],
            None => Vec::new(),
        }
    }

    fn classify_path(&self, path: &Path) -> SourcePathKind {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            return SourcePathKind::Ignore;
        }

        if self
            .source_roots()
            .iter()
            .any(|root| path.starts_with(root))
        {
            SourcePathKind::SourceFile(path.to_path_buf())
        } else {
            SourcePathKind::Ignore
        }
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
        let (offset, mut state) = decode_checkpoint_state::<CodexState>(source_path, checkpoint)?;
        let mut cursor = JsonlCursor::open(source_path, offset)?;
        let mut events = Vec::new();
        let patch_header = Regex::new(r"(?m)^\*\*\* (?:Add|Update|Delete) File: (.+)$")?;

        while let Some((line_offset, line)) = cursor.next_line()? {
            let Some(value) = parse_value(&line) else {
                continue;
            };
            let Some(timestamp) = parse_timestamp(&value, "/timestamp") else {
                continue;
            };
            let top_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();

            match top_type {
                "session_meta" => {
                    if let Some(payload) = value.get("payload") {
                        state.session_key = payload
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .or_else(|| Some(source_path.to_string_lossy().into_owned()));
                        state.project_path = payload
                            .get("cwd")
                            .and_then(Value::as_str)
                            .map(|cwd| normalize_path(Path::new(cwd), None));
                    }
                }
                "event_msg" => {
                    let payload = value.get("payload").unwrap_or(&Value::Null);
                    match payload
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                    {
                        "user_message" => state.pending_user_turns.push(PendingUserTurn {
                            line_offset,
                            occurred_at_utc: timestamp,
                        }),
                        "agent_message" => {
                            let pending = std::mem::take(&mut state.pending_user_turns);
                            for pending_turn in pending {
                                events.push(NormalizedEvent {
                                    id: stable_event_id(
                                        self.tool_id(),
                                        state.session_key.as_deref(),
                                        source_path,
                                        pending_turn.line_offset,
                                        "conversation_turn",
                                    ),
                                    tool_id: self.tool_id().to_owned(),
                                    session_key: state.session_key.clone().unwrap_or_else(|| {
                                        source_path.to_string_lossy().into_owned()
                                    }),
                                    project_path: state.project_path.clone(),
                                    occurred_at_utc: pending_turn.occurred_at_utc,
                                    local_day: local_day(pending_turn.occurred_at_utc),
                                    kind: EventKind::ConversationTurn,
                                    value: 1,
                                    unit_key: None,
                                    source_path: source_path.to_string_lossy().into_owned(),
                                });
                            }
                        }
                        "token_count" => {
                            let Some(total_usage) = payload.pointer("/info/total_token_usage")
                            else {
                                continue;
                            };
                            if let Some(current_input) =
                                total_usage.get("input_tokens").and_then(Value::as_u64)
                            {
                                let previous = state.last_input_total.unwrap_or(0);
                                let delta = current_input.saturating_sub(previous);
                                state.last_input_total = Some(current_input);
                                if delta > 0 {
                                    events.push(NormalizedEvent {
                                        id: stable_event_id(
                                            self.tool_id(),
                                            state.session_key.as_deref(),
                                            source_path,
                                            line_offset,
                                            "input_tokens",
                                        ),
                                        tool_id: self.tool_id().to_owned(),
                                        session_key: state.session_key.clone().unwrap_or_else(
                                            || source_path.to_string_lossy().into_owned(),
                                        ),
                                        project_path: state.project_path.clone(),
                                        occurred_at_utc: timestamp,
                                        local_day: local_day(timestamp),
                                        kind: EventKind::InputTokens,
                                        value: delta,
                                        unit_key: None,
                                        source_path: source_path.to_string_lossy().into_owned(),
                                    });
                                }
                            }
                            if let Some(current_output) =
                                total_usage.get("output_tokens").and_then(Value::as_u64)
                            {
                                let previous = state.last_output_total.unwrap_or(0);
                                let delta = current_output.saturating_sub(previous);
                                state.last_output_total = Some(current_output);
                                if delta > 0 {
                                    events.push(NormalizedEvent {
                                        id: stable_event_id(
                                            self.tool_id(),
                                            state.session_key.as_deref(),
                                            source_path,
                                            line_offset,
                                            "output_tokens",
                                        ),
                                        tool_id: self.tool_id().to_owned(),
                                        session_key: state.session_key.clone().unwrap_or_else(
                                            || source_path.to_string_lossy().into_owned(),
                                        ),
                                        project_path: state.project_path.clone(),
                                        occurred_at_utc: timestamp,
                                        local_day: local_day(timestamp),
                                        kind: EventKind::OutputTokens,
                                        value: delta,
                                        unit_key: None,
                                        source_path: source_path.to_string_lossy().into_owned(),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "response_item" => {
                    let payload = value.get("payload").unwrap_or(&Value::Null);
                    if payload.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
                        continue;
                    }
                    if payload.get("name").and_then(Value::as_str) != Some("apply_patch") {
                        continue;
                    }
                    let Some(patch_input) = payload.get("input").and_then(Value::as_str) else {
                        continue;
                    };

                    let mut unique_paths = BTreeSet::new();
                    for capture in patch_header.captures_iter(patch_input) {
                        let raw_path = capture.get(1).map(|m| m.as_str()).unwrap_or_default();
                        let normalized =
                            normalize_path(Path::new(raw_path), state.project_path.as_deref());
                        unique_paths.insert(normalized);
                    }

                    for file_path in unique_paths {
                        events.push(NormalizedEvent {
                            id: stable_event_id(
                                self.tool_id(),
                                state.session_key.as_deref(),
                                source_path,
                                line_offset,
                                &format!("file_edit:{file_path}"),
                            ),
                            tool_id: self.tool_id().to_owned(),
                            session_key: state
                                .session_key
                                .clone()
                                .unwrap_or_else(|| source_path.to_string_lossy().into_owned()),
                            project_path: state.project_path.clone(),
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
