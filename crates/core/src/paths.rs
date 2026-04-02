use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub launch_agent_path: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("", "jacoblincool", "code-daily-quest")
            .context("unable to resolve platform directories")?;
        let data_dir = project_dirs.data_local_dir().to_path_buf();
        fs::create_dir_all(&data_dir).context("unable to create data directory")?;

        let launch_agent_path = dirs_home()
            .join("Library")
            .join("LaunchAgents")
            .join("com.jacoblincool.code-daily-quest.plist");

        Ok(Self {
            db_path: data_dir.join("tracker.sqlite3"),
            data_dir,
            launch_agent_path,
        })
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}
