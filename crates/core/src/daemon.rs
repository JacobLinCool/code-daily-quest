use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Days, Local, TimeZone};
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::model::DailyQuest;
use crate::paths::AppPaths;
use crate::platform::default_notifier;
use crate::tracker::Tracker;

pub async fn run_daemon(paths: AppPaths) -> Result<()> {
    let mut tracker = Tracker::open(paths)?;
    let notifier = default_notifier();
    let _ = tracker.initialize_live_tracking()?;
    let _ = tracker.rollover()?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })?;

    let report = tracker.doctor_snapshot()?;
    for adapter in report.diagnostics.adapter_sources {
        for root in adapter.roots {
            let path = std::path::PathBuf::from(root);
            if path.exists() {
                watcher.watch(&path, RecursiveMode::Recursive)?;
            }
        }
    }

    let midnight = sleep_until(next_midnight_instant());
    tokio::pin!(midnight);
    let reminder = sleep_until(next_reminder_instant(Local::now()));
    tokio::pin!(reminder);

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                let Some(first_event) = maybe_event else {
                    break;
                };
                let changed_paths = debounce_changed_paths(first_event, &mut rx).await;
                if changed_paths.is_empty() {
                    continue;
                }
                let summary = tracker.sync_changed_sources(&changed_paths, true)?;
                let completed_today = summary
                    .newly_completed
                    .iter()
                    .any(|quest| quest.day == Local::now().date_naive());
                for quest in &summary.newly_completed {
                    if notifier.is_supported() {
                        let _ = notifier.notify_quest_completed(quest);
                    }
                }
                if notifier.is_supported() && completed_today {
                    let today = tracker.today_snapshot()?;
                    if today.record.all_completed {
                        let _ = notifier.notify_all_clear(today.today, today.record.closing_streak);
                    }
                }
            }
            _ = &mut midnight => {
                let quests: Vec<DailyQuest> = tracker.rollover()?;
                if notifier.is_supported() {
                    let _ = notifier.notify_daily_reset(Local::now().date_naive(), &quests);
                }
                midnight.as_mut().reset(next_midnight_instant());
                reminder.as_mut().reset(next_reminder_instant(Local::now()));
            }
            _ = &mut reminder => {
                if notifier.is_supported() {
                    let today = tracker.today_snapshot()?;
                    if !today.record.all_completed {
                        let remaining = today
                            .quests
                            .iter()
                            .filter(|quest| !quest.is_completed())
                            .cloned()
                            .collect::<Vec<_>>();
                        if !remaining.is_empty() {
                            let _ = notifier.notify_pending_reminder(today.today, &remaining);
                        }
                    }
                }
                reminder.as_mut().reset(next_reminder_instant(Local::now()));
            }
        }
    }

    Ok(())
}

async fn debounce_changed_paths(
    first_event: Result<notify::Event, notify::Error>,
    rx: &mut mpsc::UnboundedReceiver<Result<notify::Event, notify::Error>>,
) -> BTreeSet<std::path::PathBuf> {
    let mut changed_paths = extract_paths(first_event);
    let deadline = Instant::now() + Duration::from_millis(300);

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) => {
                changed_paths.extend(extract_paths(event));
            }
            _ => break,
        }
    }

    changed_paths
}

fn extract_paths(event: Result<notify::Event, notify::Error>) -> BTreeSet<std::path::PathBuf> {
    match event {
        Ok(event) => event.paths.into_iter().collect(),
        Err(_) => BTreeSet::new(),
    }
}

fn next_midnight_instant() -> Instant {
    let now = Local::now();
    let tomorrow = now
        .date_naive()
        .checked_add_days(Days::new(1))
        .expect("tomorrow should exist");
    let midnight = now
        .timezone()
        .from_local_datetime(
            &tomorrow
                .and_hms_opt(0, 0, 0)
                .expect("midnight should exist"),
        )
        .single()
        .expect("local midnight should be unique");
    let duration = (midnight - now)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(1));
    Instant::now() + duration
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReminderSlot {
    Noon,
    Evening,
}

impl ReminderSlot {
    fn hour(self) -> u32 {
        match self {
            Self::Noon => 12,
            Self::Evening => 20,
        }
    }
}

fn next_reminder_instant(now: DateTime<Local>) -> Instant {
    let (_, target) = next_reminder_time(now);
    let duration = (target - now)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(1));
    Instant::now() + duration
}

fn next_reminder_time(now: DateTime<Local>) -> (ReminderSlot, DateTime<Local>) {
    for slot in [ReminderSlot::Noon, ReminderSlot::Evening] {
        let candidate = now
            .timezone()
            .from_local_datetime(
                &now.date_naive()
                    .and_hms_opt(slot.hour(), 0, 0)
                    .expect("valid reminder time"),
            )
            .single()
            .expect("local reminder time should be unique");
        if candidate > now {
            return (slot, candidate);
        }
    }

    let tomorrow = now
        .date_naive()
        .checked_add_days(Days::new(1))
        .expect("tomorrow should exist");
    let slot = ReminderSlot::Noon;
    let candidate = now
        .timezone()
        .from_local_datetime(
            &tomorrow
                .and_hms_opt(slot.hour(), 0, 0)
                .expect("valid noon reminder"),
        )
        .single()
        .expect("local reminder time should be unique");
    (slot, candidate)
}

#[cfg(test)]
mod tests {
    use super::{ReminderSlot, next_reminder_time};
    use chrono::{Days, Local, TimeZone, Timelike};

    #[test]
    fn next_reminder_before_noon_is_noon() {
        let now = Local.with_ymd_and_hms(2026, 4, 3, 10, 30, 0).unwrap();
        let (slot, target) = next_reminder_time(now);
        assert_eq!(slot, ReminderSlot::Noon);
        assert_eq!(target.hour(), 12);
        assert_eq!(target.minute(), 0);
        assert_eq!(target.date_naive(), now.date_naive());
    }

    #[test]
    fn next_reminder_between_noon_and_evening_is_evening() {
        let now = Local.with_ymd_and_hms(2026, 4, 3, 15, 0, 0).unwrap();
        let (slot, target) = next_reminder_time(now);
        assert_eq!(slot, ReminderSlot::Evening);
        assert_eq!(target.hour(), 20);
        assert_eq!(target.minute(), 0);
        assert_eq!(target.date_naive(), now.date_naive());
    }

    #[test]
    fn next_reminder_after_evening_rolls_to_tomorrow_noon() {
        let now = Local.with_ymd_and_hms(2026, 4, 3, 22, 0, 0).unwrap();
        let (slot, target) = next_reminder_time(now);
        assert_eq!(slot, ReminderSlot::Noon);
        assert_eq!(target.hour(), 12);
        assert_eq!(target.minute(), 0);
        assert_eq!(
            target.date_naive(),
            now.date_naive().checked_add_days(Days::new(1)).unwrap()
        );
    }
}
