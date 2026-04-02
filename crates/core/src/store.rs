use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use rand::RngExt;
use rusqlite::{Connection, OptionalExtension, params};

use crate::adapters::SourceCheckpoint;
use crate::model::{
    AdapterDiagnostics, DailyQuest, DailyRecord, DiagnosticsView, DoctorReport, EventKind,
    HistoryDay, NormalizedEvent, QuestDifficulty, QuestKind, TodayView,
};
use crate::quest::generate_daily_quests;

const SCHEMA_VERSION: i32 = 2;
const METRIC_ACTIVE_PROJECTS: &str = "active_projects";
const METRIC_EDITED_FILES: &str = "edited_files";

pub struct Store {
    conn: Connection,
    db_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct IngestOutcome {
    pub changed_days: BTreeSet<NaiveDate>,
    pub newly_completed: Vec<DailyQuest>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("unable to open sqlite database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn,
            db_path: path.to_path_buf(),
        };
        store.ensure_schema()?;
        store.ensure_profile_id()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<()> {
        let current: i32 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current != SCHEMA_VERSION {
            self.reset_schema()?;
        }
        Ok(())
    }

    fn reset_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            DROP TABLE IF EXISTS normalized_events;
            DROP TABLE IF EXISTS source_checkpoints;
            DROP TABLE IF EXISTS tracked_sources;
            DROP TABLE IF EXISTS adapter_status;
            DROP TABLE IF EXISTS distinct_claims;
            DROP TABLE IF EXISTS daily_metric_totals;
            DROP TABLE IF EXISTS daily_metric_by_tool;
            DROP TABLE IF EXISTS daily_quests;
            DROP TABLE IF EXISTS daily_quest_progress_by_tool;
            DROP TABLE IF EXISTS daily_records;
            DROP TABLE IF EXISTS app_meta;

            CREATE TABLE normalized_events (
                id TEXT PRIMARY KEY,
                tool_id TEXT NOT NULL,
                session_key TEXT NOT NULL,
                project_path TEXT,
                occurred_at_utc TEXT NOT NULL,
                local_day TEXT NOT NULL,
                kind TEXT NOT NULL,
                value INTEGER NOT NULL,
                unit_key TEXT,
                source_path TEXT NOT NULL
            );
            CREATE INDEX idx_normalized_events_day_kind
                ON normalized_events(local_day, kind, occurred_at_utc, id);
            CREATE INDEX idx_normalized_events_day_project
                ON normalized_events(local_day, project_path, occurred_at_utc, id);

            CREATE TABLE source_checkpoints (
                tool_id TEXT NOT NULL,
                source_path TEXT NOT NULL,
                offset INTEGER NOT NULL,
                state_json TEXT NOT NULL,
                updated_at_utc TEXT NOT NULL,
                PRIMARY KEY (tool_id, source_path)
            );

            CREATE TABLE tracked_sources (
                tool_id TEXT NOT NULL,
                source_path TEXT NOT NULL,
                is_present INTEGER NOT NULL,
                last_offset INTEGER NOT NULL,
                last_seen_at_utc TEXT NOT NULL,
                last_error TEXT,
                PRIMARY KEY (tool_id, source_path)
            );

            CREATE TABLE adapter_status (
                tool_id TEXT PRIMARY KEY,
                roots_json TEXT NOT NULL,
                discovered_files INTEGER NOT NULL,
                discovery_error TEXT,
                last_discovery_at_utc TEXT
            );

            CREATE TABLE distinct_claims (
                day TEXT NOT NULL,
                metric_key TEXT NOT NULL,
                unit_key TEXT NOT NULL,
                tool_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                occurred_at_utc TEXT NOT NULL,
                PRIMARY KEY (day, metric_key, unit_key)
            );

            CREATE TABLE daily_metric_totals (
                day TEXT NOT NULL,
                metric_key TEXT NOT NULL,
                total INTEGER NOT NULL,
                updated_at_utc TEXT NOT NULL,
                PRIMARY KEY (day, metric_key)
            );

            CREATE TABLE daily_metric_by_tool (
                day TEXT NOT NULL,
                metric_key TEXT NOT NULL,
                tool_id TEXT NOT NULL,
                total INTEGER NOT NULL,
                PRIMARY KEY (day, metric_key, tool_id)
            );

            CREATE TABLE daily_quests (
                day TEXT NOT NULL,
                slot INTEGER NOT NULL,
                quest_kind TEXT NOT NULL,
                difficulty TEXT NOT NULL,
                threshold INTEGER NOT NULL,
                progress_total INTEGER NOT NULL,
                completed_at_utc TEXT,
                completed_by_tool_id TEXT,
                completion_event_id TEXT,
                PRIMARY KEY (day, slot)
            );

            CREATE TABLE daily_quest_progress_by_tool (
                day TEXT NOT NULL,
                slot INTEGER NOT NULL,
                tool_id TEXT NOT NULL,
                progress INTEGER NOT NULL,
                PRIMARY KEY (day, slot, tool_id)
            );

            CREATE TABLE daily_records (
                day TEXT PRIMARY KEY,
                completed_quests INTEGER NOT NULL,
                total_quests INTEGER NOT NULL,
                all_completed INTEGER NOT NULL,
                closing_streak INTEGER NOT NULL,
                updated_at_utc TEXT NOT NULL
            );

            CREATE TABLE app_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    fn ensure_profile_id(&self) -> Result<()> {
        if self.meta_value("profile_id")?.is_some() {
            return Ok(());
        }

        let bytes: [u8; 16] = rand::rng().random();
        let profile_id = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.set_meta_value("profile_id", &profile_id)?;
        Ok(())
    }

    pub fn reset_state_preserving_profile(&self) -> Result<()> {
        let profile_id = self.meta_value("profile_id")?;
        self.reset_schema()?;
        if let Some(profile_id) = profile_id {
            self.set_meta_value("profile_id", &profile_id)?;
        } else {
            self.ensure_profile_id()?;
        }
        Ok(())
    }

    pub fn reset_all_state(&self) -> Result<()> {
        self.reset_schema()?;
        self.ensure_profile_id()?;
        Ok(())
    }

    pub fn profile_id(&self) -> Result<String> {
        self.meta_value("profile_id")?
            .context("profile_id missing from app_meta")
    }

    pub fn mark_synced_now(&self) -> Result<()> {
        self.set_meta_value("last_sync_at", &Utc::now().to_rfc3339())
    }

    pub fn load_checkpoint(
        &self,
        tool_id: &str,
        source_path: &Path,
    ) -> Result<Option<SourceCheckpoint>> {
        let source_path = source_path.to_string_lossy().into_owned();
        let row = self
            .conn
            .query_row(
                "SELECT offset, state_json FROM source_checkpoints WHERE tool_id = ?1 AND source_path = ?2",
                params![tool_id, source_path],
                |row| {
                    let offset: i64 = row.get(0)?;
                    let state_json: String = row.get(1)?;
                    Ok((offset, state_json))
                },
            )
            .optional()?;

        row.map(|(offset, state_json)| {
            Ok(SourceCheckpoint {
                offset: offset as u64,
                state_json: serde_json::from_str(&state_json)?,
            })
        })
        .transpose()
    }

    pub fn save_checkpoint(
        &self,
        tool_id: &str,
        source_path: &Path,
        checkpoint: &SourceCheckpoint,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "
            INSERT INTO source_checkpoints(tool_id, source_path, offset, state_json, updated_at_utc)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(tool_id, source_path) DO UPDATE SET
                offset = excluded.offset,
                state_json = excluded.state_json,
                updated_at_utc = excluded.updated_at_utc
            ",
            params![
                tool_id,
                source_path.to_string_lossy().to_string(),
                checkpoint.offset as i64,
                serde_json::to_string(&checkpoint.state_json)?,
                now
            ],
        )?;
        Ok(())
    }

    pub fn update_tracked_source(
        &self,
        tool_id: &str,
        source_path: &Path,
        is_present: bool,
        checkpoint: Option<&SourceCheckpoint>,
        error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let last_offset = checkpoint.map(|value| value.offset as i64).unwrap_or(0);
        self.conn.execute(
            "
            INSERT INTO tracked_sources(tool_id, source_path, is_present, last_offset, last_seen_at_utc, last_error)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(tool_id, source_path) DO UPDATE SET
                is_present = excluded.is_present,
                last_offset = excluded.last_offset,
                last_seen_at_utc = excluded.last_seen_at_utc,
                last_error = excluded.last_error
            ",
            params![
                tool_id,
                source_path.to_string_lossy().to_string(),
                if is_present { 1 } else { 0 },
                last_offset,
                now,
                error
            ],
        )?;
        Ok(())
    }

    pub fn reconcile_tracked_sources(
        &self,
        tool_id: &str,
        discovered_sources: &BTreeSet<PathBuf>,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "
            SELECT source_path
            FROM tracked_sources
            WHERE tool_id = ?1
            ",
        )?;
        let rows = stmt.query_map(params![tool_id], |row| row.get::<_, String>(0))?;
        for row in rows {
            let source_path = PathBuf::from(row?);
            if discovered_sources.contains(&source_path) {
                continue;
            }
            self.update_tracked_source(tool_id, &source_path, false, None, None)?;
        }
        Ok(())
    }

    pub fn update_adapter_status(
        &self,
        tool_id: &str,
        roots: &[PathBuf],
        discovered_files: usize,
        discovery_error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let roots_json = serde_json::to_string(
            &roots
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )?;
        self.conn.execute(
            "
            INSERT INTO adapter_status(tool_id, roots_json, discovered_files, discovery_error, last_discovery_at_utc)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(tool_id) DO UPDATE SET
                roots_json = excluded.roots_json,
                discovered_files = excluded.discovered_files,
                discovery_error = excluded.discovery_error,
                last_discovery_at_utc = excluded.last_discovery_at_utc
            ",
            params![
                tool_id,
                roots_json,
                discovered_files as i64,
                discovery_error,
                now
            ],
        )?;
        Ok(())
    }

    pub fn adapter_statuses(&self) -> Result<Vec<AdapterDiagnostics>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT tool_id, roots_json, discovered_files, discovery_error
            FROM adapter_status
            ORDER BY tool_id ASC
            ",
        )?;
        let rows = stmt.query_map([], |row| {
            let roots_json: String = row.get(1)?;
            Ok(AdapterDiagnostics {
                tool_id: row.get(0)?,
                roots: serde_json::from_str(&roots_json).map_err(to_sql_error)?,
                discovered_files: row.get::<_, i64>(2)? as usize,
                discovery_error: row.get(3)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn tracked_source_exists(&self, tool_id: &str, source_path: &Path) -> Result<bool> {
        let exists = self
            .conn
            .query_row(
                "
                SELECT EXISTS(
                    SELECT 1
                    FROM tracked_sources
                    WHERE tool_id = ?1 AND source_path = ?2
                )
                ",
                params![tool_id, source_path.to_string_lossy().to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(exists != 0)
    }

    pub fn insert_events_raw(&mut self, events: &[NormalizedEvent]) -> Result<usize> {
        Ok(self.insert_new_events(events)?.len())
    }

    fn insert_new_events(&mut self, events: &[NormalizedEvent]) -> Result<Vec<NormalizedEvent>> {
        let tx = self.conn.transaction()?;
        let mut inserted = Vec::new();
        {
            let mut stmt = tx.prepare(
                "
                INSERT OR IGNORE INTO normalized_events(
                    id, tool_id, session_key, project_path, occurred_at_utc, local_day, kind, value, unit_key, source_path
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ",
            )?;
            for event in events {
                let rows = stmt.execute(params![
                    event.id,
                    event.tool_id,
                    event.session_key,
                    event.project_path,
                    event.occurred_at_utc.to_rfc3339(),
                    event.local_day.to_string(),
                    event.kind.as_str(),
                    event.value as i64,
                    event.unit_key,
                    event.source_path
                ])?;
                if rows > 0 {
                    inserted.push(event.clone());
                }
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    pub fn latest_activity_day(&self) -> Result<Option<NaiveDate>> {
        self.conn
            .query_row("SELECT MIN(local_day) FROM normalized_events", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten()
            .as_deref()
            .map(parse_day)
            .transpose()
    }

    pub fn ingest_events_incremental(
        &mut self,
        events: &[NormalizedEvent],
    ) -> Result<IngestOutcome> {
        let inserted = self.insert_new_events(events)?;
        self.advance_to_today()?;

        if inserted.is_empty() {
            self.set_meta_value("last_sync_at", &Utc::now().to_rfc3339())?;
            return Ok(IngestOutcome::default());
        }

        let mut outcome = IngestOutcome::default();
        let mut inserted = inserted;
        inserted.sort_by_key(|event| (event.local_day, event.occurred_at_utc, event.id.clone()));

        let mut earliest_changed = Local::now().date_naive();
        for event in &inserted {
            earliest_changed = earliest_changed.min(event.local_day);
            self.ensure_day_initialized(
                event.local_day,
                self.previous_day_streak(event.local_day)?,
            )?;
            let newly_completed = self.apply_event(event)?;
            outcome.changed_days.insert(event.local_day);
            outcome.newly_completed.extend(newly_completed);
        }

        if earliest_changed < Local::now().date_naive() {
            self.recompute_records_from(earliest_changed)?;
        }

        self.set_meta_value("last_sync_at", &Utc::now().to_rfc3339())?;
        Ok(outcome)
    }

    pub fn rebuild_derived_state_from_events(&self) -> Result<()> {
        self.clear_derived_state()?;
        let today = Local::now().date_naive();
        let start_day = self.latest_activity_day()?.unwrap_or(today);
        let mut previous_streak = 0usize;
        let mut day = start_day;

        loop {
            self.ensure_day_initialized(day, previous_streak)?;
            for event in self.events_for_day(day)? {
                let _ = self.apply_event(&event)?;
            }

            if day == today {
                if let Some(record) = self.daily_record(day)?
                    && !record.all_completed
                {
                    self.set_daily_record_preview(day, previous_streak)?;
                }
                break;
            }

            self.finalize_closed_day(day)?;
            previous_streak = self
                .daily_record(day)?
                .map(|record| record.closing_streak)
                .unwrap_or(0);
            day = next_day(day)?;
        }

        self.set_meta_value("last_sync_at", &Utc::now().to_rfc3339())?;
        Ok(())
    }

    fn clear_derived_state(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            DELETE FROM distinct_claims;
            DELETE FROM daily_metric_totals;
            DELETE FROM daily_metric_by_tool;
            DELETE FROM daily_quest_progress_by_tool;
            DELETE FROM daily_quests;
            DELETE FROM daily_records;
            ",
        )?;
        Ok(())
    }

    pub fn advance_to_today(&self) -> Result<()> {
        let today = Local::now().date_naive();
        let Some(mut current_day) = self.latest_record_day()? else {
            self.ensure_day_initialized(today, 0)?;
            return Ok(());
        };

        while current_day < today {
            self.finalize_closed_day(current_day)?;
            let streak = self
                .daily_record(current_day)?
                .map(|record| record.closing_streak)
                .unwrap_or(0);
            let next = next_day(current_day)?;
            self.ensure_day_initialized(next, streak)?;
            current_day = next;
        }
        Ok(())
    }

    pub fn rollover_to_today(&self) -> Result<Vec<DailyQuest>> {
        self.advance_to_today()?;
        let today = Local::now().date_naive();
        self.set_meta_value("last_sync_at", &Utc::now().to_rfc3339())?;
        self.quests_for_day(today)
    }

    fn latest_record_day(&self) -> Result<Option<NaiveDate>> {
        self.conn
            .query_row("SELECT MAX(day) FROM daily_records", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten()
            .as_deref()
            .map(parse_day)
            .transpose()
    }

    fn ensure_day_initialized(&self, day: NaiveDate, previous_streak: usize) -> Result<()> {
        let quest_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM daily_quests WHERE day = ?1",
            params![day.to_string()],
            |row| row.get(0),
        )?;
        if quest_count != 3 {
            self.conn.execute(
                "DELETE FROM daily_quest_progress_by_tool WHERE day = ?1",
                params![day.to_string()],
            )?;
            self.conn.execute(
                "DELETE FROM daily_quests WHERE day = ?1",
                params![day.to_string()],
            )?;
            let profile_id = self.profile_id()?;
            for quest in generate_daily_quests(&profile_id, day)? {
                self.conn.execute(
                    "
                    INSERT INTO daily_quests(day, slot, quest_kind, difficulty, threshold, progress_total)
                    VALUES (?1, ?2, ?3, ?4, ?5, 0)
                    ",
                    params![
                        day.to_string(),
                        quest.slot as i64,
                        quest.kind.as_str(),
                        quest.difficulty.as_str(),
                        quest.threshold as i64
                    ],
                )?;
            }
        }

        if self.daily_record(day)?.is_none() {
            self.conn.execute(
                "
                INSERT INTO daily_records(day, completed_quests, total_quests, all_completed, closing_streak, updated_at_utc)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ",
                params![
                    day.to_string(),
                    0_i64,
                    3_i64,
                    0_i64,
                    previous_streak as i64,
                    Utc::now().to_rfc3339()
                ],
            )?;
        }

        Ok(())
    }

    fn events_for_day(&self, day: NaiveDate) -> Result<Vec<NormalizedEvent>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, tool_id, session_key, project_path, occurred_at_utc, local_day, kind, value, unit_key, source_path
            FROM normalized_events
            WHERE local_day = ?1
            ORDER BY occurred_at_utc ASC, id ASC
            ",
        )?;
        let rows = stmt.query_map(params![day.to_string()], |row| {
            Ok(NormalizedEvent {
                id: row.get(0)?,
                tool_id: row.get(1)?,
                session_key: row.get(2)?,
                project_path: row.get(3)?,
                occurred_at_utc: parse_timestamp_str(&row.get::<_, String>(4)?)
                    .map_err(to_sql_error)?,
                local_day: parse_day(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
                kind: EventKind::from_str(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
                value: row.get::<_, i64>(7)? as u64,
                unit_key: row.get(8)?,
                source_path: row.get(9)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn apply_event(&self, event: &NormalizedEvent) -> Result<Vec<DailyQuest>> {
        let mut newly_completed = Vec::new();

        match event.kind {
            EventKind::ConversationTurn => {
                if let Some(quest) = self.increment_metric_and_quest(
                    event.local_day,
                    QuestKind::ConversationTurns,
                    &event.tool_id,
                    1,
                    event,
                )? {
                    newly_completed.push(quest);
                }
            }
            EventKind::InputTokens => {
                if let Some(quest) = self.increment_metric_and_quest(
                    event.local_day,
                    QuestKind::InputTokens,
                    &event.tool_id,
                    event.value,
                    event,
                )? {
                    newly_completed.push(quest);
                }
            }
            EventKind::OutputTokens => {
                if let Some(quest) = self.increment_metric_and_quest(
                    event.local_day,
                    QuestKind::OutputTokens,
                    &event.tool_id,
                    event.value,
                    event,
                )? {
                    newly_completed.push(quest);
                }
            }
            EventKind::FileEdit => {
                if let Some(unit_key) = &event.unit_key
                    && self.insert_distinct_claim(
                        event.local_day,
                        METRIC_EDITED_FILES,
                        unit_key,
                        &event.tool_id,
                        &event.id,
                        event.occurred_at_utc,
                    )?
                    && let Some(quest) = self.increment_metric_and_quest(
                        event.local_day,
                        QuestKind::EditedFiles,
                        &event.tool_id,
                        1,
                        event,
                    )?
                {
                    newly_completed.push(quest);
                }
            }
        }

        if let Some(project_path) = &event.project_path
            && matches!(
                event.kind,
                EventKind::ConversationTurn
                    | EventKind::InputTokens
                    | EventKind::OutputTokens
                    | EventKind::FileEdit
            )
            && self.insert_distinct_claim(
                event.local_day,
                METRIC_ACTIVE_PROJECTS,
                project_path,
                &event.tool_id,
                &event.id,
                event.occurred_at_utc,
            )?
            && let Some(quest) = self.increment_metric_and_quest(
                event.local_day,
                QuestKind::ActiveProjects,
                &event.tool_id,
                1,
                event,
            )?
        {
            newly_completed.push(quest);
        }

        Ok(newly_completed)
    }

    fn insert_distinct_claim(
        &self,
        day: NaiveDate,
        metric_key: &str,
        unit_key: &str,
        tool_id: &str,
        event_id: &str,
        occurred_at_utc: DateTime<Utc>,
    ) -> Result<bool> {
        let inserted = self.conn.execute(
            "
            INSERT OR IGNORE INTO distinct_claims(day, metric_key, unit_key, tool_id, event_id, occurred_at_utc)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                day.to_string(),
                metric_key,
                unit_key,
                tool_id,
                event_id,
                occurred_at_utc.to_rfc3339()
            ],
        )?;
        Ok(inserted > 0)
    }

    fn increment_metric_and_quest(
        &self,
        day: NaiveDate,
        kind: QuestKind,
        tool_id: &str,
        increment: u64,
        event: &NormalizedEvent,
    ) -> Result<Option<DailyQuest>> {
        self.increment_metric(day, kind, tool_id, increment)?;

        let Some(mut quest) = self.quest_for_kind(day, kind)? else {
            return Ok(None);
        };

        self.increment_quest_progress_by_tool(day, quest.slot, tool_id, increment)?;
        let next_total = quest.progress_total + increment;
        let mut completed_now = false;
        let completed_at = if quest.completed_at_utc.is_none() && next_total >= quest.threshold {
            completed_now = true;
            Some(event.occurred_at_utc.to_rfc3339())
        } else {
            quest
                .completed_at_utc
                .map(|timestamp| timestamp.to_rfc3339())
        };
        let completed_by_tool_id = if completed_now {
            Some(event.tool_id.clone())
        } else {
            quest.completed_by_tool_id.clone()
        };
        let completion_event_id = if completed_now {
            Some(event.id.clone())
        } else {
            quest.completion_event_id.clone()
        };

        self.conn.execute(
            "
            UPDATE daily_quests
            SET progress_total = ?3,
                completed_at_utc = ?4,
                completed_by_tool_id = ?5,
                completion_event_id = ?6
            WHERE day = ?1 AND slot = ?2
            ",
            params![
                day.to_string(),
                quest.slot as i64,
                next_total as i64,
                completed_at,
                completed_by_tool_id,
                completion_event_id
            ],
        )?;

        if !completed_now {
            return Ok(None);
        }

        self.increment_daily_record_completed(day)?;
        quest = self
            .quest_for_slot(day, quest.slot)?
            .context("quest disappeared after update")?;
        Ok(Some(quest))
    }

    fn increment_metric(
        &self,
        day: NaiveDate,
        kind: QuestKind,
        tool_id: &str,
        increment: u64,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "
            INSERT INTO daily_metric_totals(day, metric_key, total, updated_at_utc)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(day, metric_key) DO UPDATE SET
                total = daily_metric_totals.total + excluded.total,
                updated_at_utc = excluded.updated_at_utc
            ",
            params![day.to_string(), kind.as_str(), increment as i64, now],
        )?;
        self.conn.execute(
            "
            INSERT INTO daily_metric_by_tool(day, metric_key, tool_id, total)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(day, metric_key, tool_id) DO UPDATE SET
                total = daily_metric_by_tool.total + excluded.total
            ",
            params![day.to_string(), kind.as_str(), tool_id, increment as i64],
        )?;
        Ok(())
    }

    fn increment_quest_progress_by_tool(
        &self,
        day: NaiveDate,
        slot: usize,
        tool_id: &str,
        increment: u64,
    ) -> Result<()> {
        self.conn.execute(
            "
            INSERT INTO daily_quest_progress_by_tool(day, slot, tool_id, progress)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(day, slot, tool_id) DO UPDATE SET
                progress = daily_quest_progress_by_tool.progress + excluded.progress
            ",
            params![day.to_string(), slot as i64, tool_id, increment as i64],
        )?;
        Ok(())
    }

    fn increment_daily_record_completed(&self, day: NaiveDate) -> Result<()> {
        let mut record = self
            .daily_record(day)?
            .context("daily record missing while updating quest completion")?;
        record.completed_quests += 1;
        record.all_completed = record.completed_quests == record.total_quests;
        record.closing_streak = if record.all_completed {
            self.previous_day_streak(day)? + 1
        } else {
            self.previous_day_streak(day)?
        };
        self.conn.execute(
            "
            UPDATE daily_records
            SET completed_quests = ?2,
                all_completed = ?3,
                closing_streak = ?4,
                updated_at_utc = ?5
            WHERE day = ?1
            ",
            params![
                day.to_string(),
                record.completed_quests as i64,
                if record.all_completed { 1 } else { 0 },
                record.closing_streak as i64,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    fn previous_day_streak(&self, day: NaiveDate) -> Result<usize> {
        let Some(previous_day) = day.checked_sub_days(Days::new(1)) else {
            return Ok(0);
        };
        Ok(self
            .daily_record(previous_day)?
            .map(|record| record.closing_streak)
            .unwrap_or(0))
    }

    fn finalize_closed_day(&self, day: NaiveDate) -> Result<()> {
        let Some(record) = self.daily_record(day)? else {
            return Ok(());
        };
        if record.all_completed || record.closing_streak == 0 {
            return Ok(());
        }
        self.conn.execute(
            "
            UPDATE daily_records
            SET closing_streak = 0,
                updated_at_utc = ?2
            WHERE day = ?1
            ",
            params![day.to_string(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn set_daily_record_preview(&self, day: NaiveDate, closing_streak: usize) -> Result<()> {
        self.conn.execute(
            "
            UPDATE daily_records
            SET closing_streak = ?2,
                updated_at_utc = ?3
            WHERE day = ?1
            ",
            params![
                day.to_string(),
                closing_streak as i64,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    fn recompute_records_from(&self, start_day: NaiveDate) -> Result<()> {
        let today = Local::now().date_naive();
        let mut previous_streak =
            if let Some(previous_day) = start_day.checked_sub_days(Days::new(1)) {
                self.daily_record(previous_day)?
                    .map(|record| record.closing_streak)
                    .unwrap_or(0)
            } else {
                0
            };

        let mut day = start_day;
        loop {
            self.ensure_day_initialized(day, previous_streak)?;
            let completed_quests = self.count_completed_quests(day)?;
            let total_quests = self.count_total_quests(day)?;
            let all_completed = total_quests > 0 && completed_quests == total_quests;
            let closing_streak = if day == today && !all_completed {
                previous_streak
            } else if all_completed {
                previous_streak + 1
            } else {
                0
            };
            self.conn.execute(
                "
                UPDATE daily_records
                SET completed_quests = ?2,
                    total_quests = ?3,
                    all_completed = ?4,
                    closing_streak = ?5,
                    updated_at_utc = ?6
                WHERE day = ?1
                ",
                params![
                    day.to_string(),
                    completed_quests as i64,
                    total_quests as i64,
                    if all_completed { 1 } else { 0 },
                    closing_streak as i64,
                    Utc::now().to_rfc3339()
                ],
            )?;

            previous_streak = closing_streak;
            if day == today {
                break;
            }
            day = next_day(day)?;
        }
        Ok(())
    }

    fn count_completed_quests(&self, day: NaiveDate) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM daily_quests WHERE day = ?1 AND completed_at_utc IS NOT NULL",
            params![day.to_string()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    fn count_total_quests(&self, day: NaiveDate) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM daily_quests WHERE day = ?1",
            params![day.to_string()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    fn quest_for_kind(&self, day: NaiveDate, kind: QuestKind) -> Result<Option<DailyQuest>> {
        self.conn
            .query_row(
                "
                SELECT slot
                FROM daily_quests
                WHERE day = ?1 AND quest_kind = ?2
                ",
                params![day.to_string(), kind.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|slot| self.quest_for_slot(day, slot as usize))
            .transpose()
            .map(Option::flatten)
    }

    fn quest_for_slot(&self, day: NaiveDate, slot: usize) -> Result<Option<DailyQuest>> {
        let mut quests = self.quests_for_day(day)?;
        Ok(quests.drain(..).find(|quest| quest.slot == slot))
    }

    pub fn quests_for_day(&self, day: NaiveDate) -> Result<Vec<DailyQuest>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT slot, quest_kind, difficulty, threshold, progress_total, completed_at_utc,
                   completed_by_tool_id, completion_event_id
            FROM daily_quests
            WHERE day = ?1
            ORDER BY slot ASC
            ",
        )?;
        let rows = stmt.query_map(params![day.to_string()], |row| {
            let completed_at: Option<String> = row.get(5)?;
            Ok(DailyQuest {
                day,
                slot: row.get::<_, i64>(0)? as usize,
                kind: parse_quest_kind(&row.get::<_, String>(1)?)?,
                difficulty: parse_difficulty(&row.get::<_, String>(2)?)?,
                threshold: row.get::<_, i64>(3)? as u64,
                progress_total: row.get::<_, i64>(4)? as u64,
                progress_by_tool: BTreeMap::new(),
                completed_at_utc: completed_at
                    .as_deref()
                    .map(parse_timestamp_str)
                    .transpose()
                    .map_err(to_sql_error)?,
                completed_by_tool_id: row.get(6)?,
                completion_event_id: row.get(7)?,
            })
        })?;

        let mut quests = Vec::new();
        for row in rows {
            let mut quest = row?;
            quest.progress_by_tool = self.quest_progress_by_tool(day, quest.slot)?;
            quests.push(quest);
        }
        Ok(quests)
    }

    fn quest_progress_by_tool(&self, day: NaiveDate, slot: usize) -> Result<BTreeMap<String, u64>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT tool_id, progress
            FROM daily_quest_progress_by_tool
            WHERE day = ?1 AND slot = ?2
            ORDER BY tool_id ASC
            ",
        )?;
        let rows = stmt.query_map(params![day.to_string(), slot as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?;

        let mut progress = BTreeMap::new();
        for row in rows {
            let (tool_id, value) = row?;
            progress.insert(tool_id, value);
        }
        Ok(progress)
    }

    pub fn today_view(&self, service_status: String) -> Result<TodayView> {
        let today = Local::now().date_naive();
        let quests = self.quests_for_day(today)?;
        let record = self.daily_record(today)?.unwrap_or(DailyRecord {
            day: today,
            completed_quests: 0,
            total_quests: quests.len(),
            all_completed: false,
            closing_streak: self.previous_day_streak(today)?,
        });

        Ok(TodayView {
            today,
            quests,
            record,
            next_reset_at: next_reset_utc(),
            service_status,
        })
    }

    pub fn history_days(&self, limit: usize) -> Result<Vec<HistoryDay>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT day, completed_quests, total_quests, all_completed, closing_streak
            FROM daily_records
            ORDER BY day DESC
            LIMIT ?1
            ",
        )?;
        let records = stmt.query_map(params![limit as i64], |row| {
            Ok(DailyRecord {
                day: parse_day(&row.get::<_, String>(0)?).map_err(to_sql_error)?,
                completed_quests: row.get::<_, i64>(1)? as usize,
                total_quests: row.get::<_, i64>(2)? as usize,
                all_completed: row.get::<_, i64>(3)? != 0,
                closing_streak: row.get::<_, i64>(4)? as usize,
            })
        })?;

        let mut days = Vec::new();
        for record in records {
            let record = record?;
            days.push(HistoryDay {
                quests: self.quests_for_day(record.day)?,
                record,
            });
        }
        Ok(days)
    }

    pub fn diagnostics_view(
        &self,
        adapter_sources: Vec<AdapterDiagnostics>,
    ) -> Result<DiagnosticsView> {
        let checkpoint_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM source_checkpoints", [], |row| {
                    row.get(0)
                })?;
        let event_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM normalized_events", [], |row| {
                    row.get(0)
                })?;
        let last_sync_at = self
            .meta_value("last_sync_at")?
            .as_deref()
            .map(parse_timestamp_str)
            .transpose()?;

        Ok(DiagnosticsView {
            database_path: self.db_path.to_string_lossy().into_owned(),
            adapter_sources,
            checkpoint_count: checkpoint_count as usize,
            event_count: event_count as usize,
            last_sync_at,
        })
    }

    pub fn doctor_report(
        &self,
        adapter_sources: Vec<AdapterDiagnostics>,
        notifier_supported: bool,
        service_supported: bool,
    ) -> Result<DoctorReport> {
        Ok(DoctorReport {
            diagnostics: self.diagnostics_view(adapter_sources)?,
            notifier_supported,
            service_supported,
        })
    }

    fn daily_record(&self, day: NaiveDate) -> Result<Option<DailyRecord>> {
        self.conn
            .query_row(
                "
                SELECT completed_quests, total_quests, all_completed, closing_streak
                FROM daily_records
                WHERE day = ?1
                ",
                params![day.to_string()],
                |row| {
                    Ok(DailyRecord {
                        day,
                        completed_quests: row.get::<_, i64>(0)? as usize,
                        total_quests: row.get::<_, i64>(1)? as usize,
                        all_completed: row.get::<_, i64>(2)? != 0,
                        closing_streak: row.get::<_, i64>(3)? as usize,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn meta_value(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM app_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn set_meta_value(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "
            INSERT INTO app_meta(key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![key, value],
        )?;
        Ok(())
    }
}

fn parse_day(text: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").context("invalid day format")
}

fn parse_timestamp_str(text: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(text)
        .context("invalid timestamp format")?
        .with_timezone(&Utc))
}

fn parse_quest_kind(text: &str) -> rusqlite::Result<QuestKind> {
    QuestKind::from_str(text).map_err(to_sql_error)
}

fn parse_difficulty(text: &str) -> rusqlite::Result<QuestDifficulty> {
    QuestDifficulty::from_str(text).map_err(to_sql_error)
}

fn next_day(day: NaiveDate) -> Result<NaiveDate> {
    day.checked_add_days(Days::new(1))
        .context("date overflow while iterating days")
}

fn next_reset_utc() -> DateTime<Utc> {
    let now_local = Local::now();
    let tomorrow = now_local
        .date_naive()
        .checked_add_days(Days::new(1))
        .expect("tomorrow should exist");
    let midnight_local = tomorrow
        .and_hms_opt(0, 0, 0)
        .expect("midnight should be valid");
    DateTime::<Utc>::from(
        now_local
            .timezone()
            .from_local_datetime(&midnight_local)
            .unwrap(),
    )
}

fn to_sql_error<E>(error: E) -> rusqlite::Error
where
    E: std::fmt::Display,
{
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}
