mod tui;
mod update;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use code_daily_quest_core::{AppPaths, NotificationTestKind, Tracker, daemon};

const DEFAULT_RETRO_DAYS: u32 = 90;

#[derive(Debug, Parser)]
#[command(name = "code-daily-quest")]
#[command(about = "Gamify Codex / Claude Code activity from local logs.")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Tui,
    Daemon,
    Doctor,
    Retro {
        #[arg(long, default_value_t = DEFAULT_RETRO_DAYS, value_parser = clap::value_parser!(u32).range(1..))]
        days: u32,
    },
    Clear,
    Notify {
        #[command(subcommand)]
        action: NotifyAction,
    },
    Update {
        #[command(subcommand)]
        action: Option<UpdateAction>,
    },
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Debug, Subcommand)]
enum UpdateAction {
    Apply,
}

#[derive(Debug, Subcommand)]
enum ServiceAction {
    Install,
    Uninstall,
    Status,
}

#[derive(Debug, Subcommand)]
enum NotifyAction {
    Test {
        #[arg(long, value_enum, default_value_t = NotifyKindArg::Quest)]
        kind: NotifyKindArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum NotifyKindArg {
    Quest,
    AllClear,
    Reminder,
    Reset,
}

impl From<NotifyKindArg> for NotificationTestKind {
    fn from(value: NotifyKindArg) -> Self {
        match value {
            NotifyKindArg::Quest => NotificationTestKind::Quest,
            NotifyKindArg::AllClear => NotificationTestKind::AllClear,
            NotifyKindArg::Reminder => NotificationTestKind::Reminder,
            NotifyKindArg::Reset => NotificationTestKind::Reset,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::discover()?;

    match cli.command {
        Command::Tui => {
            let tracker = Tracker::open(paths)?;
            let _ = tracker.rollover()?;
            tui::run_tui(tracker)?;
        }
        Command::Daemon => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(daemon::run_daemon(paths))?;
        }
        Command::Doctor => {
            let mut tracker = Tracker::open(paths)?;
            let report = tracker.doctor_rescan()?;
            println!("Database: {}", report.diagnostics.database_path);
            println!("Events: {}", report.diagnostics.event_count);
            println!("Checkpoints: {}", report.diagnostics.checkpoint_count);
            println!(
                "Last sync: {}",
                report
                    .diagnostics
                    .last_sync_at
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "never".to_string())
            );
            println!("Notifier supported: {}", report.notifier_supported);
            println!("Service supported: {}", report.service_supported);
            for adapter in report.diagnostics.adapter_sources {
                println!(
                    "- {}: {} files across {} roots",
                    adapter.tool_id,
                    adapter.discovered_files,
                    adapter.roots.len()
                );
                if let Some(error) = &adapter.discovery_error {
                    println!("    discovery error: {}", error);
                }
                for root in adapter.roots {
                    println!("    {}", root);
                }
            }
        }
        Command::Retro { days } => {
            let mut tracker = Tracker::open(paths)?;
            let summary = tracker.retro_rebuild(days)?;
            println!("retro rebuild complete for the last {days} days");
            for adapter in summary.adapter_sources {
                println!(
                    "- {}: {} files across {} roots",
                    adapter.tool_id,
                    adapter.discovered_files,
                    adapter.roots.len()
                );
                if let Some(error) = &adapter.discovery_error {
                    println!("    discovery error: {}", error);
                }
                for root in adapter.roots {
                    println!("    {}", root);
                }
            }
        }
        Command::Clear => {
            let tracker = Tracker::open(paths)?;
            tracker.clear_state()?;
            println!(
                "cleared local tracker data (database, quests, checkpoints, profile id); source logs were not modified"
            );
        }
        Command::Notify { action } => {
            let tracker = Tracker::open(paths)?;
            match action {
                NotifyAction::Test { kind } => {
                    tracker.send_test_notification(kind.into())?;
                    println!("sent test notification: {:?}", kind);
                }
            }
        }
        Command::Update { action } => match action {
            Some(UpdateAction::Apply) => {
                update::run(update::UpdateMode::Apply)?;
            }
            None => {
                update::run(update::UpdateMode::Check)?;
            }
        },
        Command::Service { action } => {
            let tracker = Tracker::open(paths)?;
            match action {
                ServiceAction::Install => {
                    let executable = current_executable()?;
                    tracker.install_service(&executable)?;
                    println!("installed service from {}", executable.display());
                }
                ServiceAction::Uninstall => {
                    tracker.uninstall_service()?;
                    println!("uninstalled service");
                }
                ServiceAction::Status => {
                    println!("{}", tracker.service_status()?);
                }
            }
        }
    }

    Ok(())
}

fn current_executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()?)
}
