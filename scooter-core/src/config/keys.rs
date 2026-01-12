use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::keyboard::{KeyCode, KeyEvent, KeyModifiers};

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
        One(String),
        Many(Vec<String>),
    }

    let keys = match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    };

    keys.into_iter()
        .map(|s| {
            s.parse::<KeyEvent>()
                .map_err(|e| de::Error::custom(format!("Invalid key binding '{s}': {e}")))
        })
        .collect()
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

/// Wrapper type for key bindings that can be specified as either a single key or multiple keys
#[derive(Debug, Clone, PartialEq)]
pub struct Keys(Vec<KeyEvent>);

impl Keys {
    pub fn new(keys: Vec<KeyEvent>) -> Self {
        Self(keys)
    }
}

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_key_or_keys(deserializer).map(Keys)
    }
}

impl Serialize for Keys {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_key_or_keys(&self.0, serializer)
    }
}

impl std::ops::Deref for Keys {
    type Target = Vec<KeyEvent>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> IntoIterator for &'a Keys {
    type Item = &'a KeyEvent;
    type IntoIter = std::slice::Iter<'a, KeyEvent>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Helper macro for creating `Keys` from a vec of `KeyEvent`s
#[macro_export]
macro_rules! keys {
    ($($item:expr),* $(,)?) => {
        $crate::config::Keys::new(vec![$($item),*])
    };
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeysConfig {
    #[serde(default)]
    /// Commands available on all screens
    pub general: KeysGeneral,
    #[serde(default)]
    /// Commands available on the search screen
    pub search: KeysSearch,
    #[serde(default)]
    /// Commands available on the replacement-in-progress screen
    pub performing_replacement: KeysPerformingReplacement,
    #[serde(default)]
    /// Commands available on the results screen
    pub results: KeysResults,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysGeneral {
    /// Exit scooter
    pub quit: Keys,
    /// Cancel in-progress operations, reset fields to default values and return to search screen
    pub reset: Keys,
    /// Show the help menu containing keymaps
    pub show_help_menu: Keys,
}

impl Default for KeysGeneral {
    fn default() -> Self {
        Self {
            quit: keys![KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)],
            reset: keys![KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)],
            show_help_menu: keys![KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearch {
    /// Toggle wrapping of lines that don't fit within the width of the preview
    pub toggle_preview_wrapping: Keys,
    /// Toggle inclusion of hidden files and directories, such as those whose name starts with a dot (.)
    pub toggle_hidden_files: Keys,
    #[serde(default)]
    /// Commands available on the search screen, when the search fields are focussed
    pub fields: KeysSearchFocusFields,
    #[serde(default)]
    /// Commands available on the search screen, when the search results are focussed
    pub results: KeysSearchFocusResults,
}

impl Default for KeysSearch {
    fn default() -> Self {
        Self {
            toggle_preview_wrapping: keys![KeyEvent::new(
                KeyCode::Char('l'),
                KeyModifiers::CONTROL
            )],
            toggle_hidden_files: keys![KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)],
            fields: KeysSearchFocusFields::default(),
            results: KeysSearchFocusResults::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearchFocusFields {
    /// Allow editing of fields that were populated using CLI args, such as `--search_text foo`. (Note that you can use the `disable_prepopulated_fields` config option to change the default behaviour.)
    pub unlock_prepopulated_fields: Keys,
    /// Trigger a search
    pub trigger_search: Keys,
    /// Focus on the next field
    pub focus_next_field: Keys,
    /// Focus on the previous field
    pub focus_previous_field: Keys,
}

impl Default for KeysSearchFocusFields {
    fn default() -> Self {
        Self {
            unlock_prepopulated_fields: keys![KeyEvent::new(KeyCode::Char('u'), KeyModifiers::ALT)],
            trigger_search: keys![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            focus_next_field: keys![KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)],
            focus_previous_field: keys![KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysSearchFocusResults {
    /// Trigger a replacement
    pub trigger_replacement: Keys,
    /// Move focus back to the search fields
    pub back_to_fields: Keys,
    /// Open the currently selected search result in your editor. The editor command can be overriden using the `editor_open` section of your config.
    pub open_in_editor: Keys,

    /// Navigate to the search result below
    pub move_down: Keys,
    /// Navigate to the search result above
    pub move_up: Keys,
    /// Navigate to the search result half a page below
    pub move_down_half_page: Keys,
    /// Navigate to the search result half a page above
    pub move_up_half_page: Keys,
    /// Navigate to the search result a page below
    pub move_down_full_page: Keys,
    /// Navigate to the search result a page above
    pub move_up_full_page: Keys,
    /// Navigate to the first search result
    pub move_top: Keys,
    /// Navigate to the last search result
    pub move_bottom: Keys,

    /// Toggle whether the currently highlighted result will be replaced or ignored
    pub toggle_selected_inclusion: Keys,
    /// Toggle whether all results will be replaced or ignored
    pub toggle_all_selected: Keys,
    /// Toggle whether multiselect mode is enabled
    pub toggle_multiselect_mode: Keys,

    /// Flip the direction of the multiselect selection
    pub flip_multiselect_direction: Keys,
}

impl Default for KeysSearchFocusResults {
    fn default() -> Self {
        Self {
            trigger_replacement: keys![KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)],
            back_to_fields: keys![
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            ],
            open_in_editor: keys![KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)],

            move_down: keys![
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            ],
            move_up: keys![
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            ],
            move_down_half_page: keys![KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)],
            move_down_full_page: keys![
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            ],
            move_up_half_page: keys![KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)],
            move_up_full_page: keys![
                KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            ],
            move_top: keys![KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)],
            move_bottom: keys![KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE)],

            toggle_selected_inclusion: keys![KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)],
            toggle_all_selected: keys![KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)],
            toggle_multiselect_mode: keys![KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)],

            flip_multiselect_direction: keys![KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT)],
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
#[derive(Default)]
pub struct KeysPerformingReplacement {}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct KeysResults {
    /// Navigate to the error below
    pub scroll_errors_down: Keys,
    /// Navigate to the error above
    pub scroll_errors_up: Keys,
    /// Exit scooter. This is in addition to the `quit` command in the `general` section.
    pub quit: Keys,
}

impl Default for KeysResults {
    fn default() -> Self {
        Self {
            scroll_errors_down: keys![
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            ],
            scroll_errors_up: keys![
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            ],
            quit: keys![
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            ],
        }
    }
}
