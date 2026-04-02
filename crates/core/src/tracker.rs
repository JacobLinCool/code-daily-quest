use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use chrono::{Days, Local, NaiveDate, Utc};

use crate::adapters::{SourcePathKind, ToolAdapter, default_adapters};
use crate::model::{
    AdapterDiagnostics, DailyQuest, DoctorReport, HistoryDay, NotificationTestKind,
    TOOL_CLAUDE_CODE, TOOL_CODEX, TodayView,
};
use crate::paths::AppPaths;
use crate::platform::{default_autostart_installer, default_notifier};
use crate::quest::generate_daily_quests;
use crate::store::{IngestOutcome, Store};

pub struct Tracker {
    paths: AppPaths,
    store: Store,
    adapters: Vec<Box<dyn ToolAdapter>>,
}

pub struct StartupSummary {
    pub adapter_sources: Vec<AdapterDiagnostics>,
}

pub struct SyncSummary {
    pub adapter_sources: Vec<AdapterDiagnostics>,
    pub newly_completed: Vec<DailyQuest>,
}

impl Tracker {
    pub fn open(paths: AppPaths) -> Result<Self> {
        Ok(Self {
            store: Store::open(&paths.db_path)?,
            adapters: default_adapters(),
            paths,
        })
    }

    #[cfg(test)]
    fn with_adapters(paths: AppPaths, adapters: Vec<Box<dyn ToolAdapter>>) -> Result<Self> {
        Ok(Self {
            store: Store::open(&paths.db_path)?,
            adapters,
            paths,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.paths.db_path
    }

    pub fn initialize_live_tracking(&mut self) -> Result<StartupSummary> {
        let mut catch_up_events = Vec::new();
        let today = Local::now().date_naive();

        for adapter in &self.adapters {
            let roots = adapter.source_roots();
            match adapter.discover_sources() {
                Ok(sources) => {
                    let discovered = sources.iter().cloned().collect::<BTreeSet<_>>();
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        sources.len(),
                        None,
                    )?;
                    self.store
                        .reconcile_tracked_sources(adapter.tool_id(), &discovered)?;
                    for source in sources {
                        let has_checkpoint = self
                            .store
                            .load_checkpoint(adapter.tool_id(), &source)?
                            .is_some();
                        if let Some(parsed_events) = self.parse_source(adapter.as_ref(), &source)? {
                            if has_checkpoint {
                                catch_up_events.extend(parsed_events);
                            } else {
                                catch_up_events.extend(
                                    parsed_events
                                        .into_iter()
                                        .filter(|event| event.local_day == today),
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        0,
                        Some(&error.to_string()),
                    )?;
                }
            }
        }

        if catch_up_events.is_empty() {
            self.store.mark_synced_now()?;
        } else {
            let _ = self.store.ingest_events_incremental(&catch_up_events)?;
        }

        Ok(StartupSummary {
            adapter_sources: self.store.adapter_statuses()?,
        })
    }

    pub fn retro_rebuild(&mut self, days: u32) -> Result<StartupSummary> {
        self.store.reset_state_preserving_profile()?;
        let earliest_day = retro_earliest_day(days);

        for adapter in &self.adapters {
            let roots = adapter.source_roots();
            match adapter.discover_sources() {
                Ok(sources) => {
                    let discovered = sources.iter().cloned().collect::<BTreeSet<_>>();
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        sources.len(),
                        None,
                    )?;
                    self.store
                        .reconcile_tracked_sources(adapter.tool_id(), &discovered)?;
                    for source in sources {
                        if let Some(parsed_events) = self.parse_source(adapter.as_ref(), &source)? {
                            let filtered = parsed_events
                                .into_iter()
                                .filter(|event| event.local_day >= earliest_day)
                                .collect::<Vec<_>>();
                            let _ = self.store.insert_events_raw(&filtered)?;
                        }
                    }
                }
                Err(error) => {
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        0,
                        Some(&error.to_string()),
                    )?;
                }
            }
        }

        self.store.rebuild_derived_state_from_events()?;
        Ok(StartupSummary {
            adapter_sources: self.store.adapter_statuses()?,
        })
    }

    pub fn clear_state(&self) -> Result<()> {
        self.store.reset_all_state()
    }

    pub fn sync_changed_sources(
        &mut self,
        changed_paths: &BTreeSet<PathBuf>,
        live_notifications: bool,
    ) -> Result<SyncSummary> {
        self.store.advance_to_today()?;
        let mut events = Vec::new();

        for (adapter_index, paths) in self.group_changed_paths(changed_paths) {
            let adapter = &self.adapters[adapter_index];
            for path in paths {
                if path.exists() {
                    match self.parse_source(adapter.as_ref(), &path)? {
                        Some(parsed_events) => events.extend(parsed_events),
                        None => continue,
                    }
                } else if self.store.tracked_source_exists(adapter.tool_id(), &path)? {
                    self.store.update_tracked_source(
                        adapter.tool_id(),
                        &path,
                        false,
                        None,
                        None,
                    )?;
                }
            }
        }

        let ingest = self.store.ingest_events_incremental(&events)?;
        self.sync_summary(ingest, live_notifications)
    }

    pub fn rollover(&self) -> Result<Vec<DailyQuest>> {
        self.store.rollover_to_today()
    }

    pub fn today_view(&self) -> Result<TodayView> {
        let installer = default_autostart_installer(&self.paths);
        self.store.today_view(installer.status()?)
    }

    pub fn today_view_with_service_status(&self, service_status: String) -> Result<TodayView> {
        self.store.today_view(service_status)
    }

    pub fn today_snapshot(&self) -> Result<TodayView> {
        self.store.today_view(String::new())
    }

    pub fn history_days(&self, limit: usize) -> Result<Vec<HistoryDay>> {
        self.store.history_days(limit)
    }

    pub fn doctor_snapshot(&self) -> Result<DoctorReport> {
        let notifier = default_notifier();
        let installer = default_autostart_installer(&self.paths);
        self.store.doctor_report(
            self.store.adapter_statuses()?,
            notifier.is_supported(),
            installer.is_supported(),
        )
    }

    pub fn send_test_notification(&self, kind: NotificationTestKind) -> Result<()> {
        let notifier = default_notifier();
        if !notifier.is_supported() {
            bail!("notifications are unsupported on this platform");
        }

        let today = Local::now().date_naive();
        let sample_quests = self.sample_notification_quests(today)?;

        match kind {
            NotificationTestKind::Quest => {
                let mut quest = sample_quests
                    .first()
                    .cloned()
                    .expect("sample quests should not be empty");
                quest.progress_total = quest.threshold;
                quest
                    .progress_by_tool
                    .insert(TOOL_CODEX.to_string(), quest.threshold);
                quest.completed_at_utc = Some(Utc::now());
                quest.completed_by_tool_id = Some(TOOL_CODEX.to_string());
                quest.completion_event_id = Some("test-notification".to_string());
                notifier.notify_quest_completed(&quest)
            }
            NotificationTestKind::AllClear => notifier.notify_all_clear(today, 7),
            NotificationTestKind::Reminder => {
                let remaining = sample_quests
                    .into_iter()
                    .take(2)
                    .enumerate()
                    .map(|(index, mut quest)| {
                        quest.progress_total = (quest.threshold / 2).max(1);
                        let tool = if index % 2 == 0 {
                            TOOL_CODEX
                        } else {
                            TOOL_CLAUDE_CODE
                        };
                        quest
                            .progress_by_tool
                            .insert(tool.to_string(), quest.progress_total);
                        quest
                    })
                    .collect::<Vec<_>>();
                notifier.notify_pending_reminder(today, &remaining)
            }
            NotificationTestKind::Reset => notifier.notify_daily_reset(today, &sample_quests),
        }
    }

    pub fn doctor_rescan(&mut self) -> Result<DoctorReport> {
        for adapter in &self.adapters {
            let roots = adapter.source_roots();
            match adapter.discover_sources() {
                Ok(sources) => {
                    let discovered = sources.iter().cloned().collect::<BTreeSet<_>>();
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        sources.len(),
                        None,
                    )?;
                    self.store
                        .reconcile_tracked_sources(adapter.tool_id(), &discovered)?;
                    for source in discovered {
                        let checkpoint = self.store.load_checkpoint(adapter.tool_id(), &source)?;
                        self.store.update_tracked_source(
                            adapter.tool_id(),
                            &source,
                            true,
                            checkpoint.as_ref(),
                            None,
                        )?;
                    }
                }
                Err(error) => {
                    self.store.update_adapter_status(
                        adapter.tool_id(),
                        &roots,
                        0,
                        Some(&error.to_string()),
                    )?;
                }
            }
        }
        self.doctor_snapshot()
    }

    pub fn install_service(&self, executable: &Path) -> Result<()> {
        default_autostart_installer(&self.paths).install(executable)
    }

    pub fn uninstall_service(&self) -> Result<()> {
        default_autostart_installer(&self.paths).uninstall()
    }

    pub fn service_status(&self) -> Result<String> {
        default_autostart_installer(&self.paths).status()
    }

    fn sync_summary(&self, ingest: IngestOutcome, live_notifications: bool) -> Result<SyncSummary> {
        Ok(SyncSummary {
            adapter_sources: self.store.adapter_statuses()?,
            newly_completed: if live_notifications {
                ingest.newly_completed
            } else {
                Vec::new()
            },
        })
    }

    fn group_changed_paths(
        &self,
        changed_paths: &BTreeSet<PathBuf>,
    ) -> BTreeMap<usize, BTreeSet<PathBuf>> {
        let mut grouped = BTreeMap::new();
        for path in changed_paths {
            for (index, adapter) in self.adapters.iter().enumerate() {
                if let SourcePathKind::SourceFile(source_path) = adapter.classify_path(path) {
                    grouped
                        .entry(index)
                        .or_insert_with(BTreeSet::new)
                        .insert(source_path);
                }
            }
        }
        grouped
    }

    fn parse_source(
        &self,
        adapter: &dyn ToolAdapter,
        source: &Path,
    ) -> Result<Option<Vec<crate::model::NormalizedEvent>>> {
        let checkpoint = self.checkpoint_for_source(adapter.tool_id(), source)?;
        match adapter.parse_incremental(source, checkpoint.as_ref()) {
            Ok(parsed) => {
                self.store
                    .save_checkpoint(adapter.tool_id(), source, &parsed.checkpoint)?;
                self.store.update_tracked_source(
                    adapter.tool_id(),
                    source,
                    true,
                    Some(&parsed.checkpoint),
                    None,
                )?;
                Ok(Some(parsed.events))
            }
            Err(error) => {
                self.store.update_tracked_source(
                    adapter.tool_id(),
                    source,
                    source.exists(),
                    checkpoint.as_ref(),
                    Some(&error.to_string()),
                )?;
                Ok(None)
            }
        }
    }

    fn checkpoint_for_source(
        &self,
        tool_id: &str,
        source: &Path,
    ) -> Result<Option<crate::adapters::SourceCheckpoint>> {
        let checkpoint = self.store.load_checkpoint(tool_id, source)?;
        let Some(checkpoint) = checkpoint else {
            return Ok(None);
        };
        let source_len = std::fs::metadata(source)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if source_len < checkpoint.offset {
            return Ok(None);
        }
        Ok(Some(checkpoint))
    }

    fn sample_notification_quests(&self, day: NaiveDate) -> Result<Vec<DailyQuest>> {
        let profile_id = self.store.profile_id()?;
        generate_daily_quests(&profile_id, day)
    }
}

fn retro_earliest_day(days: u32) -> NaiveDate {
    let today = Local::now().date_naive();
    let offset = u64::from(days.saturating_sub(1));
    today.checked_sub_days(Days::new(offset)).unwrap_or(today)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use chrono::{Days, Local, Utc};

    use crate::adapters::{ParsedSource, SourceCheckpoint, SourcePathKind, ToolAdapter};
    use crate::model::{DoctorReport, EventKind, NormalizedEvent};
    use crate::paths::AppPaths;

    use super::Tracker;

    #[derive(Default)]
    struct CountingAdapter {
        discover_calls: Arc<AtomicUsize>,
        parse_calls: Arc<AtomicUsize>,
        parsed_paths: Arc<Mutex<Vec<PathBuf>>>,
        sources: Vec<PathBuf>,
        events: Vec<NormalizedEvent>,
    }

    impl ToolAdapter for CountingAdapter {
        fn tool_id(&self) -> &'static str {
            "counting"
        }

        fn source_roots(&self) -> Vec<PathBuf> {
            vec![PathBuf::from("/tmp/counting")]
        }

        fn classify_path(&self, path: &Path) -> SourcePathKind {
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                SourcePathKind::SourceFile(path.to_path_buf())
            } else {
                SourcePathKind::Ignore
            }
        }

        fn discover_sources(&self) -> Result<Vec<PathBuf>> {
            self.discover_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.sources.clone())
        }

        fn parse_incremental(
            &self,
            source_path: &Path,
            _checkpoint: Option<&SourceCheckpoint>,
        ) -> Result<ParsedSource> {
            self.parse_calls.fetch_add(1, Ordering::SeqCst);
            self.parsed_paths
                .lock()
                .unwrap()
                .push(source_path.to_path_buf());
            let events = if self.events.is_empty() {
                vec![NormalizedEvent {
                    id: format!("event:{}", source_path.display()),
                    tool_id: "counting".to_string(),
                    session_key: "session".to_string(),
                    project_path: Some("/tmp/project".to_string()),
                    occurred_at_utc: Utc::now(),
                    local_day: Local::now().date_naive(),
                    kind: EventKind::ConversationTurn,
                    value: 1,
                    unit_key: None,
                    source_path: source_path.to_string_lossy().into_owned(),
                }]
            } else {
                self.events.clone()
            };
            Ok(ParsedSource {
                events,
                checkpoint: SourceCheckpoint {
                    offset: 1,
                    state_json: serde_json::json!({}),
                },
            })
        }
    }

    #[test]
    fn doctor_snapshot_uses_cached_adapter_status() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_dir: temp.path().to_path_buf(),
            db_path: temp.path().join("tracker.sqlite3"),
            launch_agent_path: temp.path().join("launchd.plist"),
        };
        let adapter = CountingAdapter::default();
        let discover_calls = adapter.discover_calls.clone();
        let tracker = Tracker::with_adapters(paths.clone(), vec![Box::new(adapter)]).unwrap();
        tracker
            .store
            .update_adapter_status("counting", &[PathBuf::from("/tmp/counting")], 3, None)
            .unwrap();

        let report: DoctorReport = tracker.doctor_snapshot().unwrap();
        assert_eq!(report.diagnostics.adapter_sources[0].discovered_files, 3);
        assert_eq!(discover_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn sync_changed_sources_only_parses_targeted_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source_a = temp.path().join("a.jsonl");
        let source_b = temp.path().join("b.jsonl");
        std::fs::write(&source_a, "{}\n").unwrap();
        std::fs::write(&source_b, "{}\n").unwrap();

        let paths = AppPaths {
            data_dir: temp.path().to_path_buf(),
            db_path: temp.path().join("tracker.sqlite3"),
            launch_agent_path: temp.path().join("launchd.plist"),
        };
        let adapter = CountingAdapter {
            discover_calls: Arc::new(AtomicUsize::new(0)),
            parse_calls: Arc::new(AtomicUsize::new(0)),
            parsed_paths: Arc::new(Mutex::new(Vec::new())),
            sources: vec![source_a.clone(), source_b.clone()],
            events: Vec::new(),
        };
        let discover_calls = adapter.discover_calls.clone();
        let parse_calls = adapter.parse_calls.clone();
        let parsed_paths = adapter.parsed_paths.clone();
        let mut tracker = Tracker::with_adapters(paths, vec![Box::new(adapter)]).unwrap();

        let _ = tracker.initialize_live_tracking().unwrap();
        assert_eq!(discover_calls.load(Ordering::SeqCst), 1);
        assert_eq!(parse_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            tracker.doctor_snapshot().unwrap().diagnostics.event_count,
            2
        );

        let mut changed = BTreeSet::new();
        changed.insert(source_b.clone());
        let _ = tracker.sync_changed_sources(&changed, true).unwrap();

        assert_eq!(discover_calls.load(Ordering::SeqCst), 1);
        assert_eq!(parse_calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            tracker.doctor_snapshot().unwrap().diagnostics.event_count,
            2
        );
        let parsed = parsed_paths.lock().unwrap();
        assert_eq!(parsed.last().unwrap(), &source_b);
    }

    #[test]
    fn retro_rebuild_ingests_existing_history() {
        let temp = tempfile::tempdir().unwrap();
        let source_a = temp.path().join("a.jsonl");
        let source_b = temp.path().join("b.jsonl");
        std::fs::write(&source_a, "{}\n").unwrap();
        std::fs::write(&source_b, "{}\n").unwrap();

        let paths = AppPaths {
            data_dir: temp.path().to_path_buf(),
            db_path: temp.path().join("tracker.sqlite3"),
            launch_agent_path: temp.path().join("launchd.plist"),
        };
        let adapter = CountingAdapter {
            discover_calls: Arc::new(AtomicUsize::new(0)),
            parse_calls: Arc::new(AtomicUsize::new(0)),
            parsed_paths: Arc::new(Mutex::new(Vec::new())),
            sources: vec![source_a, source_b],
            events: Vec::new(),
        };
        let parse_calls = adapter.parse_calls.clone();
        let mut tracker = Tracker::with_adapters(paths, vec![Box::new(adapter)]).unwrap();

        let _ = tracker.retro_rebuild(90).unwrap();

        assert_eq!(parse_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            tracker.doctor_snapshot().unwrap().diagnostics.event_count,
            2
        );
    }

    #[test]
    fn retro_rebuild_respects_day_window() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("retro.jsonl");
        std::fs::write(&source, "{}\n").unwrap();

        let today = Local::now().date_naive();
        let old_day = today.checked_sub_days(Days::new(120)).unwrap();
        let recent_day = today.checked_sub_days(Days::new(10)).unwrap();
        let paths = AppPaths {
            data_dir: temp.path().to_path_buf(),
            db_path: temp.path().join("tracker.sqlite3"),
            launch_agent_path: temp.path().join("launchd.plist"),
        };
        let adapter = CountingAdapter {
            discover_calls: Arc::new(AtomicUsize::new(0)),
            parse_calls: Arc::new(AtomicUsize::new(0)),
            parsed_paths: Arc::new(Mutex::new(Vec::new())),
            sources: vec![source.clone()],
            events: vec![
                NormalizedEvent {
                    id: "old".to_string(),
                    tool_id: "counting".to_string(),
                    session_key: "session-old".to_string(),
                    project_path: Some("/tmp/project-old".to_string()),
                    occurred_at_utc: Utc::now() - chrono::Duration::days(120),
                    local_day: old_day,
                    kind: EventKind::ConversationTurn,
                    value: 1,
                    unit_key: None,
                    source_path: source.to_string_lossy().into_owned(),
                },
                NormalizedEvent {
                    id: "recent".to_string(),
                    tool_id: "counting".to_string(),
                    session_key: "session-recent".to_string(),
                    project_path: Some("/tmp/project-recent".to_string()),
                    occurred_at_utc: Utc::now() - chrono::Duration::days(10),
                    local_day: recent_day,
                    kind: EventKind::ConversationTurn,
                    value: 1,
                    unit_key: None,
                    source_path: source.to_string_lossy().into_owned(),
                },
            ],
        };
        let mut tracker = Tracker::with_adapters(paths, vec![Box::new(adapter)]).unwrap();

        let _ = tracker.retro_rebuild(90).unwrap();

        assert_eq!(
            tracker.doctor_snapshot().unwrap().diagnostics.event_count,
            1
        );
        let history = tracker.history_days(30).unwrap();
        assert!(history.iter().any(|day| day.record.day == recent_day));
        assert!(!history.iter().any(|day| day.record.day == old_day));
    }

    #[test]
    fn initialize_live_tracking_imports_today_but_not_older_history() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("today.jsonl");
        std::fs::write(&source, "{}\n").unwrap();

        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().unwrap();
        let paths = AppPaths {
            data_dir: temp.path().to_path_buf(),
            db_path: temp.path().join("tracker.sqlite3"),
            launch_agent_path: temp.path().join("launchd.plist"),
        };
        let adapter = CountingAdapter {
            discover_calls: Arc::new(AtomicUsize::new(0)),
            parse_calls: Arc::new(AtomicUsize::new(0)),
            parsed_paths: Arc::new(Mutex::new(Vec::new())),
            sources: vec![source.clone()],
            events: vec![
                NormalizedEvent {
                    id: "yesterday".to_string(),
                    tool_id: "counting".to_string(),
                    session_key: "session".to_string(),
                    project_path: Some("/tmp/project".to_string()),
                    occurred_at_utc: Utc::now() - chrono::Duration::days(1),
                    local_day: yesterday,
                    kind: EventKind::ConversationTurn,
                    value: 1,
                    unit_key: None,
                    source_path: source.to_string_lossy().into_owned(),
                },
                NormalizedEvent {
                    id: "today".to_string(),
                    tool_id: "counting".to_string(),
                    session_key: "session".to_string(),
                    project_path: Some("/tmp/project".to_string()),
                    occurred_at_utc: Utc::now(),
                    local_day: today,
                    kind: EventKind::ConversationTurn,
                    value: 1,
                    unit_key: None,
                    source_path: source.to_string_lossy().into_owned(),
                },
            ],
        };
        let mut tracker = Tracker::with_adapters(paths, vec![Box::new(adapter)]).unwrap();

        let _ = tracker.initialize_live_tracking().unwrap();

        let report = tracker.doctor_snapshot().unwrap();
        assert_eq!(report.diagnostics.event_count, 1);
        let history = tracker.history_days(5).unwrap();
        assert!(history.iter().all(|day| day.record.day != yesterday));
        assert!(history.iter().any(|day| day.record.day == today));
    }
}
