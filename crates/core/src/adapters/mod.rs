use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use blake3::Hasher;
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::NormalizedEvent;

mod claude;
mod codex;

pub use claude::ClaudeCodeAdapter;
pub use codex::CodexAdapter;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceCheckpoint {
    pub offset: u64,
    pub state_json: Value,
}

#[derive(Debug, Clone)]
pub struct ParsedSource {
    pub events: Vec<NormalizedEvent>,
    pub checkpoint: SourceCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcePathKind {
    SourceFile(PathBuf),
    Ignore,
}

pub trait ToolAdapter: Send + Sync {
    fn tool_id(&self) -> &'static str;
    fn source_roots(&self) -> Vec<PathBuf>;
    fn classify_path(&self, path: &Path) -> SourcePathKind;
    fn discover_sources(&self) -> Result<Vec<PathBuf>>;
    fn parse_incremental(
        &self,
        source_path: &Path,
        checkpoint: Option<&SourceCheckpoint>,
    ) -> Result<ParsedSource>;
}

pub fn default_adapters() -> Vec<Box<dyn ToolAdapter>> {
    vec![Box::new(CodexAdapter), Box::new(ClaudeCodeAdapter)]
}

pub(crate) struct JsonlCursor {
    reader: BufReader<File>,
    offset: u64,
}

impl JsonlCursor {
    pub(crate) fn open(path: &Path, offset: u64) -> Result<Self> {
        let mut file = File::open(path)
            .with_context(|| format!("unable to open source file {}", path.display()))?;
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("unable to seek source file {}", path.display()))?;

        Ok(Self {
            reader: BufReader::new(file),
            offset,
        })
    }

    pub(crate) fn next_line(&mut self) -> Result<Option<(u64, String)>> {
        let line_start = self.offset;
        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .context("unable to read jsonl line")?;
        if bytes == 0 {
            return Ok(None);
        }
        self.offset += bytes as u64;
        Ok(Some((line_start, line)))
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }
}

pub(crate) fn parse_value(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok()
}

pub(crate) fn parse_timestamp(value: &Value, pointer: &str) -> Option<DateTime<Utc>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn local_day(timestamp: DateTime<Utc>) -> NaiveDate {
    timestamp.with_timezone(&Local).date_naive()
}

pub(crate) fn stable_event_id(
    tool_id: &str,
    session_key: Option<&str>,
    source_path: &Path,
    line_offset: u64,
    discriminator: &str,
) -> String {
    let mut hasher = Hasher::new();
    hasher.update(tool_id.as_bytes());
    if let Some(session_key) = session_key {
        hasher.update(session_key.as_bytes());
    } else {
        hasher.update(source_path.to_string_lossy().as_bytes());
    }
    hasher.update(line_offset.to_string().as_bytes());
    hasher.update(discriminator.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(crate) fn normalize_path(path: &Path, cwd: Option<&str>) -> String {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base) = cwd {
        Path::new(base).join(path)
    } else {
        path.to_path_buf()
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    normalized.to_string_lossy().into_owned()
}

pub(crate) fn decode_checkpoint_state<T>(
    source_path: &Path,
    checkpoint: Option<&SourceCheckpoint>,
) -> Result<(u64, T)>
where
    T: DeserializeOwned + Default,
{
    match checkpoint {
        Some(checkpoint) => {
            let state =
                serde_json::from_value(checkpoint.state_json.clone()).with_context(|| {
                    format!(
                        "unable to decode checkpoint state for {} at offset {}",
                        source_path.display(),
                        checkpoint.offset
                    )
                })?;
            Ok((checkpoint.offset, state))
        }
        None => Ok((0, T::default())),
    }
}
