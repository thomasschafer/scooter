use std::collections::HashMap;

use crate::{
    app::{FocussedSection, Screen},
    config::KeysConfig,
    keyboard::{KeyCode, KeyEvent, KeyModifiers},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    General(CommandGeneral),
    SearchFields(CommandSearchFields),
    PerformingReplacement(CommandPerformingReplacement),
    Results(CommandResults),
}

// Events applicable to all screens
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandGeneral {
    Quit,
    Reset,
    ShowHelpMenu,
}

// Events applicable only to `SearchFields` screen
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandSearchFields {
    TogglePreviewWrapping,
    SearchFocusFields(CommandSearchFocusFields),
    SearchFocusResults(CommandSearchFocusResults),
}

// Events applicable only to `Screen::SearchFields` screen when focussed section is `FocussedSection::SearchFields`
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandSearchFocusFields {
    UnlockPrepopulatedFields,
    TriggerSearch,
    FocusNextField,
    FocusPreviousField,
    EnterChars(KeyCode, KeyModifiers),
}

// Events applicable only to `Screen::SearchFields` screen when focussed section is `FocussedSection::SearchFields`
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandSearchFocusResults {
    TriggerReplacement,
    BackToFields,
    OpenInEditor,

    MoveSelectedDown,
    MoveSelectedUp,
    MoveSelectedDownHalfPage,
    MoveSelectedDownFullPage,
    MoveSelectedUpHalfPage,
    MoveSelectedUpFullPage,
    MoveSelectedTop,
    MoveSelectedBottom,

    ToggleSelectedInclusion,
    ToggleAllSelected,
    ToggleMultiselectMode,

    FlipMultiselectDirection,
}

// TODO(key-remap): should we add anything here?
// Events applicable only to `PerformingReplacement` screen
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandPerformingReplacement {}

// Events applicable only to `Results` screen
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandResults {
    ScrollErrorsDown,
    ScrollErrorsUp,
    Quit,
}

#[derive(Debug)]
pub(crate) struct KeyMap {
    general: HashMap<KeyEvent, CommandGeneral>,
    search_fields: HashMap<KeyEvent, CommandSearchFocusFields>,
    search_results: HashMap<KeyEvent, CommandSearchFocusResults>,
    search_common: HashMap<KeyEvent, CommandSearchFields>,
    #[allow(clippy::zero_sized_map_values)]
    performing_replacement: HashMap<KeyEvent, CommandPerformingReplacement>,
    results: HashMap<KeyEvent, CommandResults>,
}

/// Represents a key binding conflict detected during `KeyMap` construction
#[derive(Debug)]
pub(crate) struct KeyConflict {
    pub(crate) key: KeyEvent,
    pub(crate) context: String,
    pub(crate) commands: Vec<String>,
}

impl KeyMap {
    /// Build a `KeyMap` from `KeysConfig`, detecting any conflicts
    #[allow(clippy::too_many_lines)]
    pub(crate) fn from_config(keys_config: &KeysConfig) -> Result<Self, Vec<KeyConflict>> {
        macro_rules! build_map {
            ($config:expr, $context:expr, $conflicts:expr, [
                $(($field:ident, $command:expr)),* $(,)?
            ]) => {{
                let mut map = HashMap::new();
                $(
                    for key in &$config.$field {
                        Self::insert_and_detect(&mut map, *key, $command, $context, $conflicts);
                    }
                )*
                map
            }};
        }

        let mut conflicts = Vec::new();

        let general = build_map!(
            &keys_config.general,
            "general",
            &mut conflicts,
            [
                (quit, CommandGeneral::Quit),
                (reset, CommandGeneral::Reset),
                (show_help_menu, CommandGeneral::ShowHelpMenu),
            ]
        );

        let search_common = build_map!(
            &keys_config.search,
            "search_fields",
            &mut conflicts,
            [(
                toggle_preview_wrapping,
                CommandSearchFields::TogglePreviewWrapping
            ),]
        );

        let search_fields = build_map!(
            &keys_config.search.fields,
            "search_focus_fields",
            &mut conflicts,
            [
                (
                    unlock_prepopulated_fields,
                    CommandSearchFocusFields::UnlockPrepopulatedFields
                ),
                (trigger_search, CommandSearchFocusFields::TriggerSearch),
                (focus_next_field, CommandSearchFocusFields::FocusNextField),
                (
                    focus_previous_field,
                    CommandSearchFocusFields::FocusPreviousField
                ),
            ]
        );

        let search_results = build_map!(
            &keys_config.search.results,
            "search_focus_results",
            &mut conflicts,
            [
                (
                    trigger_replacement,
                    CommandSearchFocusResults::TriggerReplacement
                ),
                (back_to_fields, CommandSearchFocusResults::BackToFields),
                (open_in_editor, CommandSearchFocusResults::OpenInEditor),
                (
                    move_selected_down,
                    CommandSearchFocusResults::MoveSelectedDown
                ),
                (move_selected_up, CommandSearchFocusResults::MoveSelectedUp),
                (
                    move_selected_down_half_page,
                    CommandSearchFocusResults::MoveSelectedDownHalfPage
                ),
                (
                    move_selected_down_full_page,
                    CommandSearchFocusResults::MoveSelectedDownFullPage
                ),
                (
                    move_selected_up_half_page,
                    CommandSearchFocusResults::MoveSelectedUpHalfPage
                ),
                (
                    move_selected_up_full_page,
                    CommandSearchFocusResults::MoveSelectedUpFullPage
                ),
                (
                    move_selected_top,
                    CommandSearchFocusResults::MoveSelectedTop
                ),
                (
                    move_selected_bottom,
                    CommandSearchFocusResults::MoveSelectedBottom
                ),
                (
                    toggle_selected_inclusion,
                    CommandSearchFocusResults::ToggleSelectedInclusion
                ),
                (
                    toggle_all_selected,
                    CommandSearchFocusResults::ToggleAllSelected
                ),
                (
                    toggle_multiselect_mode,
                    CommandSearchFocusResults::ToggleMultiselectMode
                ),
                (
                    flip_multiselect_direction,
                    CommandSearchFocusResults::FlipMultiselectDirection
                ),
            ]
        );

        let results = build_map!(
            &keys_config.results,
            "results",
            &mut conflicts,
            [
                (scroll_errors_down, CommandResults::ScrollErrorsDown),
                (scroll_errors_up, CommandResults::ScrollErrorsUp),
                (quit, CommandResults::Quit),
            ]
        );

        #[allow(clippy::zero_sized_map_values)]
        let performing_replacement = HashMap::new();

        if conflicts.is_empty() {
            Ok(Self {
                general,
                search_fields,
                search_results,
                search_common,
                performing_replacement,
                results,
            })
        } else {
            Err(conflicts)
        }
    }

    /// Insert a key binding and detect conflicts
    fn insert_and_detect<T: std::fmt::Debug>(
        map: &mut HashMap<KeyEvent, T>,
        key: KeyEvent,
        command: T,
        context: &str,
        conflicts: &mut Vec<KeyConflict>,
    ) {
        if let Some(existing) = map.insert(key, command) {
            // Convert snake_case Debug names to human-readable format
            let format_command = |cmd: &T| -> String {
                let debug_str = format!("{cmd:?}");
                // Convert PascalCase to snake_case
                debug_str
                    .chars()
                    .enumerate()
                    .flat_map(|(i, c)| {
                        if i > 0 && c.is_uppercase() {
                            vec!['_', c]
                        } else {
                            vec![c]
                        }
                    })
                    .collect::<String>()
                    .to_lowercase()
            };

            conflicts.push(KeyConflict {
                key,
                context: context.to_string(),
                commands: vec![
                    format_command(&existing),
                    format_command(map.get(&key).unwrap()),
                ],
            });
        }
    }

    /// Look up a command for the given key event and screen context
    pub(crate) fn lookup(&self, screen: &Screen, key_event: KeyEvent) -> Option<Command> {
        // Check screen-specific commands
        if let Some(cmd) = match screen {
            Screen::SearchFields(state) => {
                // Check common SearchFields commands first
                if let Some(cmd) = self.search_common.get(&key_event) {
                    return Some(Command::SearchFields(cmd.clone()));
                }
                // Then check focus-specific commands
                match state.focussed_section {
                    FocussedSection::SearchFields => {
                        self.search_fields.get(&key_event).map(|cmd| {
                            Command::SearchFields(CommandSearchFields::SearchFocusFields(
                                cmd.clone(),
                            ))
                        })
                    }
                    FocussedSection::SearchResults => {
                        self.search_results.get(&key_event).map(|cmd| {
                            Command::SearchFields(CommandSearchFields::SearchFocusResults(
                                cmd.clone(),
                            ))
                        })
                    }
                }
            }
            Screen::PerformingReplacement(_) => self
                .performing_replacement
                .get(&key_event)
                .map(|cmd| Command::PerformingReplacement(cmd.clone())),
            Screen::Results(_) => self
                .results
                .get(&key_event)
                .map(|cmd| Command::Results(cmd.clone())),
        } {
            return Some(cmd);
        }

        // Check general commands - must happen after looking up screen-specific commands
        if let Some(cmd) = self.general.get(&key_event) {
            return Some(Command::General(cmd.clone()));
        }
        None
    }
}

pub(crate) fn display_conflict_errors(conflicts: Vec<KeyConflict>) -> anyhow::Error {
    use std::fmt::Write;

    let mut error_msg = String::from("Key binding conflict detected!\n\n");
    for conflict in conflicts {
        writeln!(
            &mut error_msg,
            "The key '{}' is bound to multiple commands in [keys.{}]:",
            conflict.key, conflict.context
        )
        .unwrap();
        for (i, cmd) in conflict.commands.iter().enumerate() {
            writeln!(&mut error_msg, "  {}. {}", i + 1, cmd).unwrap();
        }
        error_msg.push_str("\nPlease update your config to use unique key bindings.");
    }
    anyhow::anyhow!(error_msg)
}
