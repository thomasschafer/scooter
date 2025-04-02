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

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub editor_open: Option<EditorOpenConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorOpenConfig {
    /// The command used when pressing `o` on the search results page. Two variables are available: `%file`, which will be replaced with the file path of the seach result, and `%line`, which will be replaced with the line number of the result. For example:
    /// ```toml
    /// [editor_open]
    /// command = "vi %file +%line"
    /// ```
    pub command: Option<String>,
    /// Whether to exit after running the command defined by `editor_open.command`.
    #[serde(default)]
    pub exit: bool,

    /// Whether to disable fields set by cli flags.
    #[serde(default = "default_true")]
    pub disable_populated_fields: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self { editor_open: None }
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
