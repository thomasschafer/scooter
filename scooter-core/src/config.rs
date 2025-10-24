use anyhow::anyhow;
use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use syntect::highlighting::{Theme, ThemeSet};

use crate::keyboard::{KeyCode, KeyEvent, KeyModifiers};

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

#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
#[derive(Default)]
pub struct EditorOpenConfig {
    /// The command used when pressing `e` on the search results page. Two variables are available: `%file`, which will be replaced
    /// with the file path of the search result, and `%line`, which will be replaced with the line number of the result. For example:
    /// ```toml
    /// [editor_open]
    /// command = "vi %file +%line"
    /// ```
    /// If not set explicitly, scooter will attempt to use the editor set by the `$EDITOR` environment variable.
    pub command: Option<String>,
    /// Whether to exit scooter after running the command defined by `editor_open.command`. Defaults to `false`.
    pub exit: bool,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PreviewConfig {
    /// Whether to apply syntax highlighting to the preview. Defaults to `true`.
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
    #[serde(deserialize_with = "deserialize_syntax_highlighting_theme")]
    pub syntax_highlighting_theme: Theme,
    /// Wrap text onto the next line if it is wider than the preview window. Defaults to `false`. (Can be toggled in the UI using `ctrl+l`.)
    pub wrap_text: bool,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            syntax_highlighting: true,
            syntax_highlighting_theme: load_theme("base16-eighties.dark").unwrap(),
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
#[serde(deny_unknown_fields, default)]
pub struct StyleConfig {
    /// Force enable or disable true color. `true` forces true color (supported by most modern terminals but not e.g. Apple Terminal), while `false` forces 256 colors (supported by almost all terminals including Apple Terminal).
    /// If omitted, scooter will attempt to determine whether the terminal being used supports true color.
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
#[serde(deny_unknown_fields, default)]
pub struct SearchConfig {
    /// Whether to disable fields set by CLI flags. Set to `false` to allow editing of these pre-populated fields. Defaults to `true`.
    pub disable_prepopulated_fields: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            disable_prepopulated_fields: true,
        }
    }
}

impl<'de> Deserialize<'de> for KeyEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Deserialize either a single `KeyEvent` or a Vec of `KeyEvents` into a Vec
fn deserialize_key_or_keys<'de, D>(deserializer: D) -> Result<Vec<KeyEvent>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(KeyEvent),
        Many(Vec<KeyEvent>),
    }

    match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(key) => Ok(vec![key]),
        OneOrMany::Many(keys) => Ok(keys),
    }
}

/// Serialize a Vec of `KeyEvent`s, using a single value if the vec has one element
fn serialize_key_or_keys<S>(keys: &Vec<KeyEvent>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if keys.len() == 1 {
        keys[0].serialize(serializer)
    } else {
        keys.serialize(serializer)
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeysConfig {
    #[serde(default)]
    pub general: KeysGeneral,
    #[serde(default)]
    pub search: KeysSearch,
    #[serde(default)]
    pub performing_replacement: KeysPerformingReplacement,
    #[serde(default)]
    pub results: KeysResults,
}

// TODO(key-remap): remove duplication of deserialize_key_or_keys

/// Commands available on all screens
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysGeneral {
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub quit: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub reset: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub show_help_menu: Vec<KeyEvent>,
}

impl Default for KeysGeneral {
    fn default() -> Self {
        Self {
            quit: vec![KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)],
            reset: vec![KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)],
            show_help_menu: vec![KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)],
        }
    }
}

/// Commands available on the search screen
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearch {
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub toggle_preview_wrapping: Vec<KeyEvent>,
    #[serde(default)]
    pub fields: KeysSearchFocusFields,
    #[serde(default)]
    pub results: KeysSearchFocusResults,
}

impl Default for KeysSearch {
    fn default() -> Self {
        Self {
            toggle_preview_wrapping: vec![KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)],
            fields: KeysSearchFocusFields::default(),
            results: KeysSearchFocusResults::default(),
        }
    }
}

/// Commands available on the search screen, when the search fields are focussed
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearchFocusFields {
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub unlock_prepopulated_fields: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub trigger_search: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub focus_next_field: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub focus_previous_field: Vec<KeyEvent>,
}

impl Default for KeysSearchFocusFields {
    fn default() -> Self {
        Self {
            unlock_prepopulated_fields: vec![KeyEvent::new(KeyCode::Char('u'), KeyModifiers::ALT)],
            trigger_search: vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            focus_next_field: vec![KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)],
            focus_previous_field: vec![KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)],
        }
    }
}

/// Commands available on the search screen, when the search results are focussed
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearchFocusResults {
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub trigger_replacement: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub back_to_fields: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub open_in_editor: Vec<KeyEvent>,

    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_down: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_up: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_down_half_page: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_down_full_page: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_up_half_page: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_up_full_page: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_top: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub move_selected_bottom: Vec<KeyEvent>,

    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub toggle_selected_inclusion: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub toggle_all_selected: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub toggle_multiselect_mode: Vec<KeyEvent>,

    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub flip_multiselect_direction: Vec<KeyEvent>,
}

impl Default for KeysSearchFocusResults {
    fn default() -> Self {
        Self {
            trigger_replacement: vec![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            back_to_fields: vec![
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            ],
            open_in_editor: vec![KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)],

            move_selected_down: vec![
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            ],
            move_selected_up: vec![
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            ],
            move_selected_down_half_page: vec![KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
            )],
            move_selected_down_full_page: vec![
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            ],
            move_selected_up_half_page: vec![KeyEvent::new(
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
            )],
            move_selected_up_full_page: vec![
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            ],
            move_selected_top: vec![KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)],
            move_selected_bottom: vec![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE)],

            toggle_selected_inclusion: vec![KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)],
            toggle_all_selected: vec![KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)],
            toggle_multiselect_mode: vec![KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)],

            flip_multiselect_direction: vec![KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT)],
        }
    }
}

/// Commands available on the replacement-in-progress screen
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
#[derive(Default)]
pub struct KeysPerformingReplacement {}

/// Commands available on the results screen
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysResults {
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub scroll_errors_down: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub scroll_errors_up: Vec<KeyEvent>,
    #[serde(
        deserialize_with = "deserialize_key_or_keys",
        serialize_with = "serialize_key_or_keys"
    )]
    pub quit: Vec<KeyEvent>,
}

impl Default for KeysResults {
    fn default() -> Self {
        Self {
            scroll_errors_down: vec![
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            ],
            scroll_errors_up: vec![
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            ],
            quit: vec![
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            ],
        }
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
            PreviewConfig::default().syntax_highlighting_theme.name
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
                keys: KeysConfig::default(),
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
            Some(&PreviewConfig::default().syntax_highlighting_theme)
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
            keys: KeysConfig::default(),
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
            keys: KeysConfig::default(),
        };
        assert_eq!(
            config.get_theme(),
            Some(&load_theme("base16-ocean.dark").unwrap())
        );
    }

    #[test]
    fn test_unknown_keys_field_rejected() {
        let result: Result<Config, _> = toml::from_str(
            r#"
[keys.search.this_doesnt_exist]
trigger_search = "a"

[keys.search.fields]
trigger_search = "S-tab"
"#,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown field `this_doesnt_exist`"));
    }
}
