// TODO(config): this logic needs a big refactor to merge specified config options with defaults,
// rather than having defaults specified in multiple places
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
    /// The command used when pressing `o` on the search results page. Two variables are available: `%file`, which will be replaced with the file path of the seach result, and `%line`, which will be replaced with the line number of the result. For example:
    /// ```toml
    /// [editor_open]
    /// command = "vi %file +%line"
    /// ```
    pub command: Option<String>,
    /// Whether to exit after running the command defined by `editor_open.command`.
    #[serde(default)]
    pub exit: bool,
}

// TODO: test with some present, others not
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewConfig {
    /// Whether to apply syntax highlighting to the preview.
    #[serde(default = "default_true")]
    pub syntax_highlighting: bool,
    /// The theme to use when syntax highlighting is enabled. Default is `base16_eighties_dark`, other options are
    /// `base16_ocean_dark`, `base16_mocha_dark`, `base16_ocean_light`, `inspired_github`, `solarized_dark` or `solarized_light`.
    pub syntax_highlighting_theme: Option<SyntaxHighlightingTheme>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxHighlightingTheme {
    Base16EightiesDark,
    Base16OceanDark,
    Base16MochaDark,
    Base16OceanLight,
    InspiredGithub,
    SolarizedDark,
    SolarizedLight,
}

impl SyntaxHighlightingTheme {
    pub fn to_theme_string(&self) -> String {
        match self {
            SyntaxHighlightingTheme::Base16EightiesDark => "base16-eighties.dark",
            SyntaxHighlightingTheme::Base16OceanDark => "base16-ocean.dark",
            SyntaxHighlightingTheme::Base16MochaDark => "base16-mocha.dark",
            SyntaxHighlightingTheme::Base16OceanLight => "base16-ocean.light",
            SyntaxHighlightingTheme::InspiredGithub => "InspiredGitHub",
            SyntaxHighlightingTheme::SolarizedDark => "Solarized (dark)",
            SyntaxHighlightingTheme::SolarizedLight => "Solarized (light)",
        }
        .to_string()
    }
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: true,
                syntax_highlighting_theme: Some(SyntaxHighlightingTheme::Base16EightiesDark),
            }),
        }
    }
}

impl Config {
    /// Returns `None` if the user wants syntax highlighting disabled, otherwise `Some(theme)` where `theme`
    /// is the user's selected theme or otherwise the default
    // TODO(config): make it easier to get default - refactor Config, then delete this whole method?
    pub(crate) fn get_theme(&self) -> Option<SyntaxHighlightingTheme> {
        let default = Config::default()
            .preview
            .unwrap()
            .syntax_highlighting_theme
            .unwrap();
        if let Some(p) = &self.preview {
            if p.syntax_highlighting {
                if let Some(theme) = &p.syntax_highlighting_theme {
                    Some(theme.clone())
                } else {
                    Some(default)
                }
            } else {
                None
            }
        } else {
            Some(default)
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
        assert_eq!(
            config.get_theme(),
            Some(SyntaxHighlightingTheme::Base16EightiesDark)
        );
    }

    #[test]
    fn test_get_theme_disabled() {
        let config = Config {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: false,
                syntax_highlighting_theme: None,
            }),
        };
        assert_eq!(config.get_theme(), None);
    }

    #[test]
    fn test_get_theme_disabled_with_theme() {
        let config = Config {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: false,
                syntax_highlighting_theme: Some(SyntaxHighlightingTheme::Base16OceanDark),
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
                syntax_highlighting_theme: Some(SyntaxHighlightingTheme::Base16OceanDark),
            }),
        };
        assert_eq!(
            config.get_theme(),
            Some(SyntaxHighlightingTheme::Base16OceanDark)
        );
    }

    #[test]
    fn test_get_theme_enabled_with_no_theme() {
        let config = Config {
            editor_open: None,
            preview: Some(PreviewConfig {
                syntax_highlighting: true,
                syntax_highlighting_theme: None,
            }),
        };
        assert_eq!(
            config.get_theme(),
            Some(SyntaxHighlightingTheme::Base16EightiesDark)
        );
    }
}
