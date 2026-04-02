use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::Serialize;

use crate::model::DailyQuest;
use crate::paths::AppPaths;

pub trait Notifier: Send + Sync {
    fn is_supported(&self) -> bool;
    fn notify_quest_completed(&self, quest: &DailyQuest) -> Result<()>;
    fn notify_all_clear(&self, day: NaiveDate, streak: usize) -> Result<()>;
    fn notify_pending_reminder(&self, day: NaiveDate, remaining: &[DailyQuest]) -> Result<()>;
    fn notify_daily_reset(&self, day: NaiveDate, quests: &[DailyQuest]) -> Result<()>;
}

pub trait AutostartInstaller: Send + Sync {
    fn is_supported(&self) -> bool;
    fn install(&self, executable: &Path) -> Result<()>;
    fn uninstall(&self) -> Result<()>;
    fn status(&self) -> Result<String>;
}

pub fn default_notifier() -> Box<dyn Notifier> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacNotifier)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(UnsupportedNotifier)
    }
}

pub fn default_autostart_installer(paths: &AppPaths) -> Box<dyn AutostartInstaller> {
    #[cfg(target_os = "macos")]
    {
        Box::new(LaunchdInstaller::new(paths.clone()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(UnsupportedInstaller)
    }
}

#[cfg(target_os = "macos")]
struct MacNotifier;

#[cfg(not(target_os = "macos"))]
struct UnsupportedNotifier;

#[cfg(target_os = "macos")]
impl Notifier for MacNotifier {
    fn is_supported(&self) -> bool {
        true
    }

    fn notify_quest_completed(&self, quest: &DailyQuest) -> Result<()> {
        let subtitle = quest
            .completed_by_tool_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let message = format!(
            "{} {} reached {} {} via {}",
            quest.difficulty.label(),
            quest.kind.label(),
            quest.threshold,
            quest.kind.unit_label(),
            subtitle
        );
        mac_notification_sys::send_notification(
            "Code Daily Quest",
            Some("Quest Completed"),
            &message,
            None,
        )?;
        Ok(())
    }

    fn notify_all_clear(&self, day: NaiveDate, streak: usize) -> Result<()> {
        let streak_suffix = if streak > 0 {
            format!(" Streak: {streak}.")
        } else {
            String::new()
        };
        mac_notification_sys::send_notification(
            "Code Daily Quest",
            Some("All Clear"),
            &format!("{day}: all 3 quests completed.{streak_suffix}"),
            None,
        )?;
        Ok(())
    }

    fn notify_pending_reminder(&self, day: NaiveDate, remaining: &[DailyQuest]) -> Result<()> {
        let summary = remaining
            .iter()
            .map(|quest| {
                format!(
                    "{} {}/{}",
                    quest.kind.label(),
                    quest.progress_total,
                    quest.threshold
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        let count = remaining.len();
        let noun = if count == 1 { "quest" } else { "quests" };
        mac_notification_sys::send_notification(
            "Code Daily Quest",
            Some("Still Not All Clear"),
            &format!("{day}: {count} {noun} left. {summary}"),
            None,
        )?;
        Ok(())
    }

    fn notify_daily_reset(&self, day: NaiveDate, quests: &[DailyQuest]) -> Result<()> {
        let summary = quests
            .iter()
            .map(|quest| {
                format!(
                    "{} {} {}",
                    quest.difficulty.label(),
                    quest.kind.label(),
                    quest.threshold
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        mac_notification_sys::send_notification(
            "Code Daily Quest",
            Some("New Daily Quests"),
            &format!("{day}: {summary}"),
            None,
        )?;
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
impl Notifier for UnsupportedNotifier {
    fn is_supported(&self) -> bool {
        false
    }

    fn notify_quest_completed(&self, _quest: &DailyQuest) -> Result<()> {
        bail!("notifications are unsupported on this platform")
    }

    fn notify_all_clear(&self, _day: NaiveDate, _streak: usize) -> Result<()> {
        bail!("notifications are unsupported on this platform")
    }

    fn notify_pending_reminder(&self, _day: NaiveDate, _remaining: &[DailyQuest]) -> Result<()> {
        bail!("notifications are unsupported on this platform")
    }

    fn notify_daily_reset(&self, _day: NaiveDate, _quests: &[DailyQuest]) -> Result<()> {
        bail!("notifications are unsupported on this platform")
    }
}

#[cfg(target_os = "macos")]
struct LaunchdInstaller {
    paths: AppPaths,
}

#[cfg(target_os = "macos")]
impl LaunchdInstaller {
    fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

#[cfg(target_os = "macos")]
impl AutostartInstaller for LaunchdInstaller {
    fn is_supported(&self) -> bool {
        true
    }

    fn install(&self, executable: &Path) -> Result<()> {
        let parent = self
            .paths
            .launch_agent_path
            .parent()
            .context("launch agent directory missing")?;
        fs::create_dir_all(parent)?;

        let plist = LaunchAgentPlist::new(executable);
        fs::write(&self.paths.launch_agent_path, plist.to_xml()).with_context(|| {
            format!("unable to write {}", self.paths.launch_agent_path.display())
        })?;

        let _ = self.unload();
        self.run_launchctl([
            "bootstrap",
            &format!("gui/{}", unsafe { libc::geteuid() }),
            self.paths.launch_agent_path.to_string_lossy().as_ref(),
        ])?;
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        let _ = self.unload();
        if self.paths.launch_agent_path.exists() {
            fs::remove_file(&self.paths.launch_agent_path)?;
        }
        Ok(())
    }

    fn status(&self) -> Result<String> {
        if !self.paths.launch_agent_path.exists() {
            return Ok("not installed".to_string());
        }

        let label = "com.jacoblincool.code-daily-quest";
        let uid = unsafe { libc::geteuid() };
        let output = self
            .launchctl_output(["print", &format!("gui/{uid}/{label}")])
            .context("unable to query launchctl")?;
        if output.status.success() {
            Ok("installed (loaded)".to_string())
        } else {
            Ok("installed".to_string())
        }
    }
}

#[cfg(target_os = "macos")]
impl LaunchdInstaller {
    fn unload(&self) -> Result<()> {
        self.run_launchctl([
            "bootout",
            &format!("gui/{}", unsafe { libc::geteuid() }),
            self.paths.launch_agent_path.to_string_lossy().as_ref(),
        ])
    }

    fn run_launchctl<const N: usize>(&self, args: [&str; N]) -> Result<()> {
        let output = self.launchctl_output(args)?;
        if !output.status.success() {
            bail!(
                "launchctl command failed: {}",
                summarize_command_failure(&output)
            );
        }
        Ok(())
    }

    fn launchctl_output<const N: usize>(&self, args: [&str; N]) -> Result<Output> {
        Command::new("launchctl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("unable to execute launchctl")
    }
}

#[cfg(not(target_os = "macos"))]
struct UnsupportedInstaller;

#[cfg(not(target_os = "macos"))]
impl AutostartInstaller for UnsupportedInstaller {
    fn is_supported(&self) -> bool {
        false
    }

    fn install(&self, _executable: &Path) -> Result<()> {
        bail!("autostart service is unsupported on this platform")
    }

    fn uninstall(&self) -> Result<()> {
        bail!("autostart service is unsupported on this platform")
    }

    fn status(&self) -> Result<String> {
        Ok("unsupported".to_string())
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Serialize)]
struct LaunchAgentPlist {
    label: &'static str,
    program_arguments: Vec<String>,
    run_at_load: bool,
    keep_alive: bool,
    standard_out_path: String,
    standard_error_path: String,
    working_directory: String,
}

#[cfg(target_os = "macos")]
impl LaunchAgentPlist {
    fn new(executable: &Path) -> Self {
        let log_path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Library/Logs/code-daily-quest.log");
        Self {
            label: "com.jacoblincool.code-daily-quest",
            program_arguments: vec![
                executable.to_string_lossy().into_owned(),
                "daemon".to_string(),
            ],
            run_at_load: true,
            keep_alive: true,
            standard_out_path: log_path.to_string_lossy().into_owned(),
            standard_error_path: log_path.to_string_lossy().into_owned(),
            working_directory: executable
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .to_string_lossy()
                .into_owned(),
        }
    }

    fn to_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
  <key>WorkingDirectory</key>
  <string>{}</string>
</dict>
</plist>
"#,
            self.label,
            self.program_arguments[0],
            self.program_arguments[1],
            self.standard_out_path,
            self.standard_error_path,
            self.working_directory
        )
    }
}

#[cfg(target_os = "macos")]
fn summarize_command_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}
