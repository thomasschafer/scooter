// TODO(config): this logic needs a big refactor to merge specified config options with defaults,
// rather than having defaults specified in multiple places
use anyhow::anyhow;
use serde::{de, Deserialize, Deserializer};
use std::{fs, path::PathBuf, sync::OnceLock};
use syntect::highlighting::{Theme, ThemeSet};

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};

pub const APP_NAME: &str = "scooter";

static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();
fn get_theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        let theme_folder = themes_folder();
        if theme_folder.exists() {
            themes.add_from_folder(theme_folder).unwrap();
        };
        themes
    })
}

fn config_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find config directory!");
    strategy.config_dir().join(APP_NAME)
}

fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

fn themes_folder() -> PathBuf {
    config_dir().join("themes/")
}

fn default_true() -> bool {
    true
}

fn default_theme() -> Theme {
    load_theme("base16-eighties.dark").unwrap()
}

// TODO(config): refactor this to merge options specified with defaults
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub editor_open: Option<EditorOpenConfig>,
    pub preview: Option<PreviewConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorOpenConfig {
    /// The command used when pressing `o` on the search results page. Two variables are available: `%file`, which will be replaced
    /// with the file path of the seach result, and `%line`, which will be replaced with the line number of the result. For example:
    /// ```toml
    /// [editor_open]
    /// command = "vi %file +%line"
    /// ```
    pub command: Option<String>,
    /// Whether to exit after running the command defined by `editor_open.command`.
    #[serde(default)]
    pub exit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewConfig {
    /// Whether to apply syntax highlighting to the preview.
    #[serde(default = "default_true")]
    pub syntax_highlighting: bool,
    /// The theme to use when syntax highlighting is enabled.
    ///
    /// The default is `"base16-eighties.dark"`. Other built-in options are
    /// `"base16-mocha.dark"`, `"base16-ocean.dark"`, `"base16-ocean.light"`, `"InspiredGitHub"`, `"Solarized (dark)"` and `"Solarized (light)"`.
    ///
    /// You can use other themes by adding `.tmTheme` files to `~/.config/scooter/themes/` on Linux or macOS, or `%AppData%\scooter\themes\` on Windows,
    /// and then specifying their name here. For instance, to use Catppuccin Macchiato (from [here](https://github.com/catppuccin/bat)), on Linux or macOS run:
    /// ```sh
    /// wget -P ~/.config/scooter/themes https://github.com/catppuccin/bat/raw/main/themes/Catppuccin%20Macchiato.tmTheme
    /// ```
    /// and then set `syntax_highlighting_theme = "Catppuccin Macchiato"`.
    #[serde(deserialize_with = "deserialize_theme", default = "default_theme")]
    pub syntax_highlighting_theme: Theme,
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: true,
                syntax_highlighting_theme: default_theme(),
            }),
        }
    }
}

fn load_theme(theme_name: &str) -> anyhow::Result<Theme> {
    let themes = get_theme_set();
    match themes.themes.get(theme_name) {
        Some(theme) => Ok(theme.clone()),
        None => Err(anyhow!(
            "Could not find theme {theme_name}, found {:?}",
            themes.themes.keys()
        )),
    }
}

fn deserialize_theme<'de, D>(deserializer: D) -> Result<Theme, D::Error>
where
    D: Deserializer<'de>,
{
    let theme_name = String::deserialize(deserializer)?;
    load_theme(&theme_name).map_err(de::Error::custom)
}

impl Config {
    /// Returns `None` if the user wants syntax highlighting disabled, otherwise `Some(theme)` where `theme`
    /// is the user's selected theme or otherwise the default
    // TODO(config): make it easier to get default - refactor Config, then simplify this whole method
    pub(crate) fn get_theme(&self) -> Option<&Theme> {
        match &self.preview {
            Some(p) if p.syntax_highlighting => Some(&p.syntax_highlighting_theme),
            Some(_) => None,
            None => {
                static DEFAULT: OnceLock<Theme> = OnceLock::new();
                Some(DEFAULT.get_or_init(default_theme))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_theme_none() {
        let config = Config {
            editor_open: None,
            preview: None,
        };
        assert_eq!(config.get_theme(), Some(&default_theme()),);
    }

    #[test]
    fn test_get_theme_disabled() {
        let config = Config {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: false,
                syntax_highlighting_theme: load_theme("base16-ocean.dark").unwrap(),
            }),
        };
        assert_eq!(config.get_theme(), None);
    }

    #[test]
    fn test_get_theme_enabled_with_theme() {
        let config = Config {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: true,
                syntax_highlighting_theme: load_theme("base16-ocean.dark").unwrap(),
            }),
        };
        assert_eq!(
            config.get_theme(),
            Some(&load_theme("base16-ocean.dark").unwrap())
        );
    }
}
