use anyhow::anyhow;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use serde::{Deserialize, Deserializer, de};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use syntect::highlighting::{Theme, ThemeSet};

pub const APP_NAME: &str = "scooter";

static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();
fn get_theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        let theme_folder = themes_folder();
        if theme_folder.exists() {
            themes.add_from_folder(theme_folder).unwrap();
        }
        themes
    })
}

static CONFIG_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn set_config_dir_override(dir: &Path) {
    CONFIG_DIR_OVERRIDE
        .set(dir.to_path_buf())
        .expect("Config dir override should only be set once");
}

fn config_dir() -> PathBuf {
    if let Some(dir) = CONFIG_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    let strategy = choose_base_strategy().expect("Unable to find config directory!");
    strategy.config_dir().join(APP_NAME)
}

fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

fn themes_folder() -> PathBuf {
    config_dir().join("themes/")
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub editor_open: EditorOpenConfig,
    #[serde(default)]
    pub preview: PreviewConfig,
    #[serde(default)]
    pub style: StyleConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub keys: KeysConfig,
}

impl Config {
    /// Returns `None` if the user wants syntax highlighting disabled, otherwise `Some(theme)` where `theme`
    /// is the user's selected theme or otherwise the default
    pub fn get_theme(&self) -> Option<&Theme> {
        if self.preview.syntax_highlighting {
            Some(&self.preview.syntax_highlighting_theme)
        } else {
            None
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

impl Default for Config {
    fn default() -> Self {
        toml::from_str("").expect("Empty config should deserialize with defaults")
    }
}

fn default_exit() -> bool {
    false
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EditorOpenConfig {
    /// The command used when pressing `e` on the search results page. Two variables are available: `%file`, which will be replaced
    /// with the file path of the search result, and `%line`, which will be replaced with the line number of the result. For example:
    /// ```toml
    /// [editor_open]
    /// command = "vi %file +%line"
    /// ```
    /// If not set explicitly, scooter will attempt to use the editor set by the `$EDITOR` environment variable.
    #[serde(default)]
    pub command: Option<String>,
    /// Whether to exit scooter after running the command defined by `editor_open.command`. Defaults to `false`.
    #[serde(default = "default_exit")]
    pub exit: bool,
}

impl Default for EditorOpenConfig {
    fn default() -> Self {
        Self {
            command: None,
            exit: default_exit(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreviewConfig {
    /// Whether to apply syntax highlighting to the preview. Defaults to `true`.
    #[serde(default = "default_syntax_highlighting")]
    pub syntax_highlighting: bool,
    /// The theme to use when syntax highlighting is enabled.
    ///
    /// The default is `"base16-eighties.dark"`. Other built-in options are
    /// `"base16-mocha.dark"`, `"base16-ocean.dark"`, `"base16-ocean.light"`, `"InspiredGitHub"`, `"Solarized (dark)"` and `"Solarized (light)"`.
    ///
    /// You can use other themes by adding `.tmTheme` files to `<scooter-config-dir>/themes` and then specifying their name here.
    /// By default, `<scooter-config-dir>` is `~/.config/scooter/` on Linux or macOS, or `%AppData%\scooter\` on Windows, and can be overridden with the `--config-dir` flag.
    ///
    /// For instance, to use Catppuccin Macchiato (from [here](https://github.com/catppuccin/bat)), on Linux or macOS run:
    /// ```sh
    /// wget -P ~/.config/scooter/themes https://github.com/catppuccin/bat/raw/main/themes/Catppuccin%20Macchiato.tmTheme
    /// ```
    /// and then set `syntax_highlighting_theme = "Catppuccin Macchiato"`.
    #[serde(
        deserialize_with = "deserialize_syntax_highlighting_theme",
        default = "default_syntax_highlighting_theme"
    )]
    pub syntax_highlighting_theme: Theme,
    /// Wrap text onto the next line if it is wider than the preview window. Defaults to `false`. (Can be toggled in the UI using `ctrl+l`.)
    #[serde(default)]
    pub wrap_text: bool,
}

fn default_syntax_highlighting() -> bool {
    true
}

fn default_syntax_highlighting_theme() -> Theme {
    load_theme("base16-eighties.dark").unwrap()
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            syntax_highlighting: default_syntax_highlighting(),
            syntax_highlighting_theme: default_syntax_highlighting_theme(),
            wrap_text: false,
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

fn deserialize_syntax_highlighting_theme<'de, D>(deserializer: D) -> Result<Theme, D::Error>
where
    D: Deserializer<'de>,
{
    let theme_name = String::deserialize(deserializer)?;
    load_theme(&theme_name).map_err(de::Error::custom)
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StyleConfig {
    /// Force enable or disable true color. `true` forces true color (supported by most modern terminals but not e.g. Apple Terminal), while `false` forces 256 colors (supported by almost all terminals including Apple Terminal).
    /// If omitted, scooter will attempt to determine whether the terminal being used supports true color.
    #[serde(default = "detect_true_colour")]
    pub true_color: bool,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            true_color: detect_true_colour(),
        }
    }
}

#[cfg(windows)]
fn detect_true_colour() -> bool {
    true
}

// Copied from Helix
#[cfg(not(windows))]
fn detect_true_colour() -> bool {
    if matches!(
        std::env::var("COLORTERM").map(|v| matches!(v.as_str(), "truecolor" | "24bit")),
        Ok(true)
    ) {
        return true;
    }

    match termini::TermInfo::from_env() {
        Ok(t) => {
            t.extended_cap("RGB").is_some()
                || t.extended_cap("Tc").is_some()
                || (t.extended_cap("setrgbf").is_some() && t.extended_cap("setrgbb").is_some())
        }
        Err(_) => false,
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SearchConfig {
    /// Whether to disable fields set by CLI flags. Set to `false` to allow editing of these pre-populated fields. Defaults to `true`.
    #[serde(default = "default_disable_prepopulated_fields")]
    pub disable_prepopulated_fields: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            disable_prepopulated_fields: default_disable_prepopulated_fields(),
        }
    }
}

fn default_disable_prepopulated_fields() -> bool {
    true
}

#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
pub struct KeysConfig {
    #[serde(default)]
    pub general: KeysGeneral,
    pub search_fields: KeysSearchFields,
    pub performing_replacement: KeysPerformingReplacement,
    pub results: KeysResults,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct KeysGeneral {}

impl Default for KeysGeneral {
    fn default() -> Self {
        Self {}
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct KeysSearchFields {}

impl Default for KeysSearchFields {
    fn default() -> Self {
        Self {}
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct KeysPerformingReplacement {}

impl Default for KeysPerformingReplacement {
    fn default() -> Self {
        Self {}
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct KeysResults {}

impl Default for KeysResults {
    fn default() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_file() -> anyhow::Result<()> {
        let config: Config = toml::from_str("")?;
        let default_config = Config::default();
        assert_eq!(config, default_config);

        Ok(())
    }

    #[test]
    fn test_partial_config_editor_only() -> anyhow::Result<()> {
        let config: Config = toml::from_str(
            r#"
[editor_open]
command = "vim %file +%line"
"#,
        )?;

        assert_eq!(
            config.editor_open.command,
            Some("vim %file +%line".to_string())
        );
        assert!(!config.editor_open.exit);

        let default_preview = PreviewConfig::default();
        assert_eq!(
            config.preview.syntax_highlighting,
            default_preview.syntax_highlighting
        );

        Ok(())
    }

    #[test]
    fn test_partial_config_preview_only() -> anyhow::Result<()> {
        let config: Config = toml::from_str(
            r#"
[preview]
syntax_highlighting = false
"#,
        )?;

        assert!(!config.preview.syntax_highlighting);
        assert_eq!(
            config.preview.syntax_highlighting_theme.name,
            default_syntax_highlighting_theme().name
        );

        let default_editor_open = EditorOpenConfig::default();
        assert_eq!(config.editor_open.command, default_editor_open.command);
        assert_eq!(config.editor_open.exit, default_editor_open.exit);

        Ok(())
    }

    #[test]
    fn test_full_config() -> anyhow::Result<()> {
        let config: Config = toml::from_str(
            r#"
[editor_open]
command = "nvim %file +%line"
exit = true

[preview]
syntax_highlighting = false
syntax_highlighting_theme = "Solarized (light)"
wrap_text = true

[style]
true_color = false

[search]
disable_prepopulated_fields = false
"#,
        )?;

        assert_eq!(
            config.editor_open.command,
            Some("nvim %file +%line".to_string())
        );
        assert!(config.editor_open.exit);
        assert!(!config.preview.syntax_highlighting);
        assert_eq!(
            config.preview.syntax_highlighting_theme.name,
            Some("Solarized (light)".to_string())
        );
        assert_eq!(
            config,
            Config {
                editor_open: EditorOpenConfig {
                    command: Some("nvim %file +%line".to_owned()),
                    exit: true,
                },
                preview: PreviewConfig {
                    syntax_highlighting: false,
                    syntax_highlighting_theme: load_theme("Solarized (light)").unwrap(),
                    wrap_text: true,
                },
                style: StyleConfig { true_color: false },
                search: SearchConfig {
                    disable_prepopulated_fields: false,
                },
            }
        );

        Ok(())
    }

    #[test]
    fn test_missing_editor_exit_field() -> anyhow::Result<()> {
        let config: Config = toml::from_str(
            r#"
[editor_open]
command = "vim %file +%line"
"#,
        )?;

        assert!(!config.editor_open.exit);
        Ok(())
    }

    #[test]
    fn test_get_theme_none() {
        let config = Config::default();
        assert_eq!(
            config.get_theme(),
            Some(&default_syntax_highlighting_theme())
        );
    }

    #[test]
    fn test_get_theme_disabled() {
        let config = Config {
            editor_open: EditorOpenConfig::default(),
            preview: PreviewConfig {
                syntax_highlighting: false,
                syntax_highlighting_theme: load_theme("base16-ocean.dark").unwrap(),
                wrap_text: false,
            },
            style: StyleConfig::default(),
            search: SearchConfig::default(),
        };
        assert_eq!(config.get_theme(), None);
    }

    #[test]
    fn test_get_theme_enabled_with_theme() {
        let config = Config {
            editor_open: EditorOpenConfig::default(),
            preview: PreviewConfig {
                syntax_highlighting: true,
                syntax_highlighting_theme: load_theme("base16-ocean.dark").unwrap(),
                wrap_text: false,
            },
            style: StyleConfig::default(),
            search: SearchConfig::default(),
        };
        assert_eq!(
            config.get_theme(),
            Some(&load_theme("base16-ocean.dark").unwrap())
        );
    }
}
