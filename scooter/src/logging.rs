use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use log::{LevelFilter, info};
use std::path::{Path, PathBuf};

use crate::config::APP_NAME;

pub const DEFAULT_LOG_LEVEL: &str = "error";

pub fn cache_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Error when finding cache directory");
    let mut path = strategy.cache_dir();
    path.push(APP_NAME);
    path
}

pub fn default_log_file() -> PathBuf {
    cache_dir().join(format!("{APP_NAME}.log"))
}

fn make_parent_dir(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn setup_logging(level: LevelFilter) -> anyhow::Result<()> {
    let log_path = default_log_file();
    make_parent_dir(&log_path)?;

    let _ = simple_log::file(log_path.to_str().unwrap(), level.as_str(), 100, 10);

    info!("Logging initialized at {}", log_path.display());
    Ok(())
}
