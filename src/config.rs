use serde::Deserialize;
use std::{fs, path::PathBuf};

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};

pub const APP_NAME: &str = "scooter";

fn config_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find config directory!");
    strategy.config_dir().join(APP_NAME)
}

fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

#[derive(Debug, Deserialize)]
pub struct Config {
    /// The command used when pressing `o` on the search results page. Two variable are supported: `%file`, which will be replaced with the file path of the seach result, and `%line`, which will be replaced with the line number of the result.
    ///
    /// Example:
    /// ```toml
    /// editor_open_command = "vi %file +%line"
    /// ```
    pub editor_open_command: Option<String>,
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            editor_open_command: None,
        }
    }
}

pub fn load_config() -> anyhow::Result<Config> {
    let config_file = &config_file();
    if fs::exists(config_file)? {
        let contents = fs::read_to_string(config_file)?;
        let config = toml::from_str(&contents)?;
        Ok(config)
    } else {
        Ok(Config::default())
    }
}
