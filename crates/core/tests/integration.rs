use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use chrono::{Datelike, Days, Local, TimeZone, Utc};
use code_daily_quest_core::adapters::{ClaudeCodeAdapter, CodexAdapter, ToolAdapter};
use code_daily_quest_core::model::{
    EventKind, NormalizedEvent, QuestDifficulty, QuestKind, TOOL_CLAUDE_CODE, TOOL_CODEX,
};
use code_daily_quest_core::quest::generate_daily_quests;
use code_daily_quest_core::store::Store;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn quest_generation_is_deterministic_and_pool_constrained() {
    let day = chrono::NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
    let first = generate_daily_quests("profile-1", day).unwrap();
    let second = generate_daily_quests("profile-1", day).unwrap();

    assert_eq!(first.len(), 3);
    assert_eq!(
        first
            .iter()
            .map(|quest| (quest.kind, quest.difficulty, quest.threshold))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|quest| (quest.kind, quest.difficulty, quest.threshold))
            .collect::<Vec<_>>()
    );

    let unique_kinds = first.iter().map(|quest| quest.kind).collect::<HashSet<_>>();
    assert_eq!(unique_kinds.len(), 3);

    for quest in first {
        let valid = match (quest.kind, quest.difficulty) {
            (QuestKind::ActiveProjects, QuestDifficulty::Easy) => &[1][..],
            (QuestKind::ActiveProjects, QuestDifficulty::Normal) => &[2, 3][..],
            (QuestKind::ActiveProjects, QuestDifficulty::Hard) => &[4][..],
            (QuestKind::ConversationTurns, QuestDifficulty::Easy) => &[2, 3][..],
            (QuestKind::ConversationTurns, QuestDifficulty::Normal) => &[4, 5, 6][..],
            (QuestKind::ConversationTurns, QuestDifficulty::Hard) => &[8][..],
            (QuestKind::InputTokens | QuestKind::OutputTokens, QuestDifficulty::Easy) => {
                &[1024, 2048, 4096][..]
            }
            (QuestKind::InputTokens | QuestKind::OutputTokens, QuestDifficulty::Normal) => {
                &[8192, 16384, 32768][..]
            }
            (QuestKind::InputTokens | QuestKind::OutputTokens, QuestDifficulty::Hard) => {
                &[65536, 131072, 262144][..]
            }
            (QuestKind::EditedFiles, QuestDifficulty::Easy) => &[2, 3][..],
            (QuestKind::EditedFiles, QuestDifficulty::Normal) => &[5][..],
            (QuestKind::EditedFiles, QuestDifficulty::Hard) => &[8][..],
        };
        assert!(valid.contains(&quest.threshold));
    }
}

#[test]
fn codex_adapter_tracks_pending_turns_token_deltas_and_patch_files() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("codex.jsonl");
    write_jsonl(
        &file,
        &[
            json!({
                "timestamp": "2026-04-02T00:00:00Z",
                "type": "session_meta",
                "payload": { "id": "session-1", "cwd": "/tmp/project" }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:01Z",
                "type": "event_msg",
                "payload": { "type": "user_message", "message": "hello" }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:02Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": { "input_tokens": 100, "output_tokens": 10 } }
                }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:03Z",
                "type": "event_msg",
                "payload": { "type": "agent_message", "message": "done" }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:04Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": { "input_tokens": 150, "output_tokens": 25 } }
                }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:05Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "name": "apply_patch",
                    "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-x\n+y\n*** Add File: Cargo.toml\n+z\n*** End Patch"
                }
            }),
        ],
    );

    let adapter = CodexAdapter;
    let first = adapter.parse_incremental(&file, None).unwrap();
    assert_eq!(count_kind(&first.events, EventKind::ConversationTurn), 1);
    assert_eq!(sum_kind(&first.events, EventKind::InputTokens), 150);
    assert_eq!(sum_kind(&first.events, EventKind::OutputTokens), 25);
    assert_eq!(count_kind(&first.events, EventKind::FileEdit), 2);
    assert!(
        first
            .events
            .iter()
            .any(|event| event.unit_key.as_deref() == Some("/tmp/project/src/lib.rs"))
    );
    assert!(
        first
            .events
            .iter()
            .any(|event| event.unit_key.as_deref() == Some("/tmp/project/Cargo.toml"))
    );
}

#[test]
fn claude_adapter_ignores_sidechains_tool_results_and_meta_noise() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("claude.jsonl");
    write_jsonl(
        &file,
        &[
            json!({
                "timestamp": "2026-04-02T00:00:00Z",
                "type": "user",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "message": { "content": [{ "type": "text", "text": "ship it" }] }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:01Z",
                "type": "user",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "toolUseResult": "ok",
                "sourceToolAssistantUUID": "abc",
                "message": { "content": [{ "type": "tool_result", "content": "ok" }] }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:02Z",
                "type": "assistant",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "message": {
                    "usage": { "input_tokens": 12, "output_tokens": 7 },
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Write",
                            "input": { "file_path": "src/main.rs" }
                        }
                    ]
                }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:03Z",
                "type": "assistant",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "isSidechain": true,
                "message": {
                    "usage": { "input_tokens": 999, "output_tokens": 999 },
                    "content": []
                }
            }),
            json!({
                "timestamp": "2026-04-02T00:00:04Z",
                "type": "user",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "isMeta": true,
                "message": "[Image: source.png]"
            }),
            json!({
                "timestamp": "2026-04-02T00:00:05Z",
                "type": "user",
                "sessionId": "claude-1",
                "cwd": "/tmp/project",
                "message": "[Request interrupted by user for tool use]"
            }),
        ],
    );

    let adapter = ClaudeCodeAdapter;
    let parsed = adapter.parse_incremental(&file, None).unwrap();
    assert_eq!(count_kind(&parsed.events, EventKind::ConversationTurn), 1);
    assert_eq!(sum_kind(&parsed.events, EventKind::InputTokens), 12);
    assert_eq!(sum_kind(&parsed.events, EventKind::OutputTokens), 7);
    assert_eq!(count_kind(&parsed.events, EventKind::FileEdit), 1);
    assert!(
        parsed
            .events
            .iter()
            .all(|event| event.tool_id == TOOL_CLAUDE_CODE)
    );
}

#[test]
fn stable_event_ids_survive_source_rename() {
    let dir = tempdir().unwrap();
    let first_path = dir.path().join("session-a.jsonl");
    let second_path = dir.path().join("session-b.jsonl");
    let lines = vec![
        json!({
            "timestamp": "2026-04-02T00:00:00Z",
            "type": "session_meta",
            "payload": { "id": "stable-session", "cwd": "/tmp/project" }
        }),
        json!({
            "timestamp": "2026-04-02T00:00:01Z",
            "type": "event_msg",
            "payload": { "type": "user_message", "message": "hello" }
        }),
        json!({
            "timestamp": "2026-04-02T00:00:02Z",
            "type": "event_msg",
            "payload": { "type": "agent_message", "message": "done" }
        }),
    ];
    write_jsonl(&first_path, &lines);
    write_jsonl(&second_path, &lines);

    let adapter = CodexAdapter;
    let first_ids = adapter
        .parse_incremental(&first_path, None)
        .unwrap()
        .events
        .into_iter()
        .map(|event| event.id)
        .collect::<HashSet<_>>();
    let second_ids = adapter
        .parse_incremental(&second_path, None)
        .unwrap()
        .events
        .into_iter()
        .map(|event| event.id)
        .collect::<HashSet<_>>();

    assert_eq!(first_ids, second_ids);
}

#[test]
fn incremental_ingest_preserves_last_hit_and_first_claim_ownership() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("tracker.sqlite3")).unwrap();
    let profile_id = store.profile_id().unwrap();

    let input_day = find_day_with_kind(&profile_id, QuestKind::InputTokens);
    let input_threshold = quest_threshold(&profile_id, input_day, QuestKind::InputTokens);

    let file_day = find_day_with_kind(&profile_id, QuestKind::EditedFiles);
    let file_threshold = quest_threshold(&profile_id, file_day, QuestKind::EditedFiles);

    let mut events = vec![
        synthetic_event(SyntheticEventSpec {
            id: "input-codex",
            tool_id: TOOL_CODEX,
            session_key: "session-a",
            project_path: None,
            local_day: input_day,
            offset_minutes: 0,
            kind: EventKind::InputTokens,
            value: input_threshold - 1,
            unit_key: None,
        }),
        synthetic_event(SyntheticEventSpec {
            id: "input-claude",
            tool_id: TOOL_CLAUDE_CODE,
            session_key: "session-a",
            project_path: None,
            local_day: input_day,
            offset_minutes: 1,
            kind: EventKind::InputTokens,
            value: 1,
            unit_key: None,
        }),
        synthetic_event(SyntheticEventSpec {
            id: "file-codex-1",
            tool_id: TOOL_CODEX,
            session_key: "session-b",
            project_path: None,
            local_day: file_day,
            offset_minutes: 0,
            kind: EventKind::FileEdit,
            value: 1,
            unit_key: Some("/tmp/project-files/a.rs"),
        }),
        synthetic_event(SyntheticEventSpec {
            id: "file-claude-duplicate",
            tool_id: TOOL_CLAUDE_CODE,
            session_key: "session-b",
            project_path: None,
            local_day: file_day,
            offset_minutes: 1,
            kind: EventKind::FileEdit,
            value: 1,
            unit_key: Some("/tmp/project-files/a.rs"),
        }),
    ];

    for index in 1..file_threshold {
        let tool = if index == file_threshold - 1 {
            TOOL_CLAUDE_CODE
        } else {
            TOOL_CODEX
        };
        let id = format!("file-{index}-{tool}");
        let path = format!("/tmp/project-files/{index}.rs");
        events.push(synthetic_event(SyntheticEventSpec {
            id: &id,
            tool_id: tool,
            session_key: "session-b",
            project_path: None,
            local_day: file_day,
            offset_minutes: (index + 1) as i64,
            kind: EventKind::FileEdit,
            value: 1,
            unit_key: Some(&path),
        }));
    }

    let outcome = store.ingest_events_incremental(&events).unwrap();
    assert_eq!(outcome.newly_completed.len(), 2);

    let input_quest = store
        .quests_for_day(input_day)
        .unwrap()
        .into_iter()
        .find(|quest| quest.kind == QuestKind::InputTokens)
        .unwrap();
    assert_eq!(
        input_quest.completed_by_tool_id.as_deref(),
        Some(TOOL_CLAUDE_CODE)
    );
    assert_eq!(
        input_quest.progress_by_tool.get(TOOL_CODEX),
        Some(&(input_threshold - 1))
    );
    assert_eq!(input_quest.progress_by_tool.get(TOOL_CLAUDE_CODE), Some(&1));

    let file_quest = store
        .quests_for_day(file_day)
        .unwrap()
        .into_iter()
        .find(|quest| quest.kind == QuestKind::EditedFiles)
        .unwrap();
    assert_eq!(file_quest.progress_total, file_threshold);
    assert_eq!(
        file_quest.completed_by_tool_id.as_deref(),
        Some(TOOL_CLAUDE_CODE)
    );
    assert_eq!(
        file_quest.progress_by_tool.get(TOOL_CODEX),
        Some(&(file_threshold - 1))
    );
    assert_eq!(file_quest.progress_by_tool.get(TOOL_CLAUDE_CODE), Some(&1));
}

#[test]
fn reset_all_state_wipes_local_database_and_rotates_profile() {
    let dir = tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("tracker.sqlite3")).unwrap();
    let old_profile_id = store.profile_id().unwrap();
    let today = Local::now().date_naive();

    store
        .ingest_events_incremental(&[synthetic_event(SyntheticEventSpec {
            id: "clear-me",
            tool_id: TOOL_CODEX,
            session_key: "session-clear",
            project_path: Some("/tmp/project-clear"),
            local_day: today,
            offset_minutes: 0,
            kind: EventKind::ConversationTurn,
            value: 1,
            unit_key: None,
        })])
        .unwrap();

    let before = store.diagnostics_view(Vec::new()).unwrap();
    assert_eq!(before.event_count, 1);

    store.reset_all_state().unwrap();

    let after = store.diagnostics_view(Vec::new()).unwrap();
    let new_profile_id = store.profile_id().unwrap();
    assert_eq!(after.event_count, 0);
    assert_eq!(after.checkpoint_count, 0);
    assert!(store.history_days(10).unwrap().is_empty());
    assert_ne!(old_profile_id, new_profile_id);
}

#[test]
fn rebuild_matches_incremental_and_empty_days_break_streak() {
    let dir = tempdir().unwrap();
    let incremental_path = dir.path().join("incremental.sqlite3");
    let rebuilt_path = dir.path().join("rebuilt.sqlite3");
    let mut incremental = Store::open(&incremental_path).unwrap();
    let mut rebuilt = Store::open(&rebuilt_path).unwrap();
    let profile_id = incremental.profile_id().unwrap();
    drop(rebuilt);
    set_profile_id(&rebuilt_path, &profile_id);
    rebuilt = Store::open(&rebuilt_path).unwrap();

    let start_day = Local::now()
        .date_naive()
        .checked_sub_days(Days::new(3))
        .unwrap();
    let end_day = Local::now()
        .date_naive()
        .checked_sub_days(Days::new(1))
        .unwrap();
    let gap_day = Local::now()
        .date_naive()
        .checked_sub_days(Days::new(2))
        .unwrap();

    let mut events = completion_events_for_day(&profile_id, start_day, TOOL_CODEX, "start", 0);
    events.extend(completion_events_for_day(
        &profile_id,
        end_day,
        TOOL_CLAUDE_CODE,
        "end",
        100,
    ));

    incremental.ingest_events_incremental(&events).unwrap();
    rebuilt.insert_events_raw(&events).unwrap();
    rebuilt.rebuild_derived_state_from_events().unwrap();

    let incremental_history = history_map(&incremental.history_days(10).unwrap());
    let rebuilt_history = history_map(&rebuilt.history_days(10).unwrap());

    assert_eq!(
        incremental_history
            .get(&start_day)
            .map(|day| day.record.closing_streak),
        Some(1)
    );
    assert_eq!(
        incremental_history
            .get(&gap_day)
            .map(|day| day.record.closing_streak),
        Some(0)
    );
    assert_eq!(
        incremental_history
            .get(&end_day)
            .map(|day| day.record.closing_streak),
        Some(1)
    );

    assert_eq!(
        incremental_history
            .get(&start_day)
            .unwrap()
            .record
            .completed_quests,
        rebuilt_history
            .get(&start_day)
            .unwrap()
            .record
            .completed_quests
    );
    assert_eq!(
        incremental_history
            .get(&gap_day)
            .unwrap()
            .record
            .all_completed,
        rebuilt_history.get(&gap_day).unwrap().record.all_completed
    );
    assert_eq!(
        incremental_history
            .get(&end_day)
            .unwrap()
            .record
            .closing_streak,
        rebuilt_history.get(&end_day).unwrap().record.closing_streak
    );
}

fn write_jsonl(path: &Path, lines: &[serde_json::Value]) {
    let mut output = String::new();
    for line in lines {
        output.push_str(&serde_json::to_string(line).unwrap());
        output.push('\n');
    }
    fs::write(path, output).unwrap();
}

fn count_kind(events: &[NormalizedEvent], kind: EventKind) -> usize {
    events.iter().filter(|event| event.kind == kind).count()
}

fn sum_kind(events: &[NormalizedEvent], kind: EventKind) -> u64 {
    events
        .iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.value)
        .sum()
}

fn find_day_with_kind(profile_id: &str, kind: QuestKind) -> chrono::NaiveDate {
    let mut day = Local::now().date_naive();
    for _ in 0..120 {
        if generate_daily_quests(profile_id, day)
            .unwrap()
            .into_iter()
            .any(|quest| quest.kind == kind)
        {
            return day;
        }
        day = day.checked_sub_days(Days::new(1)).unwrap();
    }
    panic!("unable to find day containing quest kind {kind:?}");
}

fn quest_threshold(profile_id: &str, day: chrono::NaiveDate, kind: QuestKind) -> u64 {
    generate_daily_quests(profile_id, day)
        .unwrap()
        .into_iter()
        .find(|quest| quest.kind == kind)
        .unwrap()
        .threshold
}

fn history_map(
    days: &[code_daily_quest_core::HistoryDay],
) -> HashMap<chrono::NaiveDate, code_daily_quest_core::HistoryDay> {
    days.iter()
        .cloned()
        .map(|day| (day.record.day, day))
        .collect()
}

fn completion_events_for_day(
    profile_id: &str,
    day: chrono::NaiveDate,
    tool_id: &str,
    session_prefix: &str,
    offset_base: i64,
) -> Vec<NormalizedEvent> {
    let mut events = Vec::new();
    let quests = generate_daily_quests(profile_id, day).unwrap();
    let mut offset = offset_base;

    for quest in quests {
        match quest.kind {
            QuestKind::ConversationTurns => {
                for index in 0..quest.threshold {
                    let id = format!("{session_prefix}-conversation-{index}");
                    events.push(synthetic_event(SyntheticEventSpec {
                        id: Box::leak(id.into_boxed_str()),
                        tool_id,
                        session_key: "session-conversation",
                        project_path: None,
                        local_day: day,
                        offset_minutes: offset,
                        kind: EventKind::ConversationTurn,
                        value: 1,
                        unit_key: None,
                    }));
                    offset += 1;
                }
                continue;
            }
            QuestKind::InputTokens => events.push(synthetic_event(SyntheticEventSpec {
                id: Box::leak(format!("{session_prefix}-input").into_boxed_str()),
                tool_id,
                session_key: "session-input",
                project_path: None,
                local_day: day,
                offset_minutes: offset,
                kind: EventKind::InputTokens,
                value: quest.threshold,
                unit_key: None,
            })),
            QuestKind::OutputTokens => events.push(synthetic_event(SyntheticEventSpec {
                id: Box::leak(format!("{session_prefix}-output").into_boxed_str()),
                tool_id,
                session_key: "session-output",
                project_path: None,
                local_day: day,
                offset_minutes: offset,
                kind: EventKind::OutputTokens,
                value: quest.threshold,
                unit_key: None,
            })),
            QuestKind::EditedFiles => {
                for index in 0..quest.threshold {
                    let id = format!("{session_prefix}-file-{index}");
                    let path = format!("/tmp/{session_prefix}/file-{index}.rs");
                    events.push(synthetic_event(SyntheticEventSpec {
                        id: Box::leak(id.into_boxed_str()),
                        tool_id,
                        session_key: "session-files",
                        project_path: None,
                        local_day: day,
                        offset_minutes: offset,
                        kind: EventKind::FileEdit,
                        value: 1,
                        unit_key: Some(Box::leak(path.into_boxed_str())),
                    }));
                    offset += 1;
                }
                continue;
            }
            QuestKind::ActiveProjects => {
                for index in 0..quest.threshold {
                    let id = format!("{session_prefix}-project-{index}");
                    let project = format!("/tmp/{session_prefix}/project-{index}");
                    events.push(synthetic_event(SyntheticEventSpec {
                        id: Box::leak(id.into_boxed_str()),
                        tool_id,
                        session_key: "session-projects",
                        project_path: Some(Box::leak(project.into_boxed_str())),
                        local_day: day,
                        offset_minutes: offset,
                        kind: EventKind::ConversationTurn,
                        value: 1,
                        unit_key: None,
                    }));
                    offset += 1;
                }
                continue;
            }
        }
        offset += 1;
    }

    events
}

struct SyntheticEventSpec<'a> {
    id: &'a str,
    tool_id: &'a str,
    session_key: &'a str,
    project_path: Option<&'a str>,
    local_day: chrono::NaiveDate,
    offset_minutes: i64,
    kind: EventKind,
    value: u64,
    unit_key: Option<&'a str>,
}

fn synthetic_event(spec: SyntheticEventSpec<'_>) -> NormalizedEvent {
    let occurred_at_utc = Utc
        .with_ymd_and_hms(
            spec.local_day.year(),
            spec.local_day.month(),
            spec.local_day.day(),
            0,
            0,
            0,
        )
        .unwrap()
        + chrono::Duration::minutes(spec.offset_minutes);
    NormalizedEvent {
        id: spec.id.to_string(),
        tool_id: spec.tool_id.to_string(),
        session_key: spec.session_key.to_string(),
        project_path: spec.project_path.map(|path| path.to_string()),
        occurred_at_utc,
        local_day: spec.local_day,
        kind: spec.kind,
        value: spec.value,
        unit_key: spec.unit_key.map(|key| key.to_string()),
        source_path: "test".to_string(),
    }
}

fn set_profile_id(path: &Path, profile_id: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "
        INSERT INTO app_meta(key, value)
        VALUES ('profile_id', ?1)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        ",
        [profile_id],
    )
    .unwrap();
}
