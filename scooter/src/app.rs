use crossterm::event::KeyEvent;
use ignore::WalkState;
use log::warn;
use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use std::{
    cmp::{max, min},
    iter::Iterator,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use std::{
    mem,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{self, JoinHandle},
};

use frep_core::{
    search::{FileSearcher, SearchResult},
    validation::{
        validate_search_configuration, SearchConfiguration, ValidationErrorHandler,
        ValidationResult,
    },
};

use crate::{
    config::{load_config, Config},
    fields::{FieldName, SearchFieldValues, SearchFields},
    replace::{self, format_replacement_results, PerformingReplacementState, ReplaceState},
    utils::ceil_div,
};

#[derive(Debug)]
pub enum Event {
    LaunchEditor((PathBuf, usize)),
    App(AppEvent),
}

#[derive(Debug)]
pub enum AppEvent {
    Rerender,
    PerformSearch,
}

#[derive(Debug)]
pub enum BackgroundProcessingEvent {
    AddSearchResults(Vec<SearchResult>),
    SearchCompleted,
    ReplacementCompleted(ReplaceState),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EventHandlingResult {
    Rerender,
    Exit(Option<String>),
    None,
}

#[derive(Debug, PartialEq, Eq)]
struct MultiSelected {
    anchor: usize,
    primary: usize,
}
impl MultiSelected {
    fn ordered(&self) -> (usize, usize) {
        if self.anchor < self.primary {
            (self.anchor, self.primary)
        } else {
            (self.primary, self.anchor)
        }
    }

    fn flip_direction(&mut self) {
        (self.anchor, self.primary) = (self.primary, self.anchor);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Selected {
    Single(usize),
    Multi(MultiSelected),
}

#[derive(Debug)]
pub struct SearchState {
    pub results: Vec<SearchResult>,

    selected: Selected,
    // TODO: make the view logic with scrolling etc. into a generic component
    pub(crate) view_offset: usize,           // Updated by UI, not app
    pub(crate) num_displayed: Option<usize>, // Updated by UI, not app

    processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,

    pub(crate) last_render: Instant,
    pub(crate) search_started: Instant,
    pub(crate) search_completed: Option<Instant>,
    pub(crate) cancelled: Arc<AtomicBool>,
}

impl SearchState {
    pub fn new(
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            results: vec![],
            selected: Selected::Single(0),
            view_offset: 0,
            num_displayed: None,
            processing_receiver,
            last_render: Instant::now(),
            search_started: Instant::now(),
            search_completed: None,
            cancelled,
        }
    }

    pub fn search_has_completed(&self) -> bool {
        self.search_completed.is_some()
    }

    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> EventHandlingResult {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j') | KeyCode::Down, _)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.move_selected_down();
            }
            (KeyCode::Char('k') | KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.move_selected_up();
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.move_selected_down_half_page();
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.move_selected_down_full_page();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.move_selected_up_half_page();
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.move_selected_up_full_page();
            }
            (KeyCode::Char('g'), _) => {
                self.move_selected_top();
            }
            (KeyCode::Char('G'), _) => {
                self.move_selected_bottom();
            }
            (KeyCode::Char(' '), _) => {
                self.toggle_selected_inclusion();
            }
            (KeyCode::Char('a'), _) => {
                self.toggle_all_selected();
            }
            (KeyCode::Char('v'), _) => {
                self.toggle_multiselect_mode();
            }
            (KeyCode::Char(';'), KeyModifiers::ALT) => {
                self.flip_multiselect_direction();
            }
            _ => return EventHandlingResult::None,
        }

        EventHandlingResult::Rerender
    }

    fn move_selected_up_by(&mut self, n: usize) {
        let primary_selected_pos = self.primary_selected_pos();
        if primary_selected_pos == 0 {
            self.selected = Selected::Single(self.results.len().saturating_sub(1));
        } else {
            self.move_primary_sel(primary_selected_pos.saturating_sub(n));
        }
    }

    fn move_selected_down_by(&mut self, n: usize) {
        let primary_selected_pos = self.primary_selected_pos();
        let end = self.results.len().saturating_sub(1);
        if primary_selected_pos >= end {
            self.selected = Selected::Single(0);
        } else {
            self.move_primary_sel(min(primary_selected_pos + n, end));
        }
    }

    fn move_selected_up(&mut self) {
        self.move_selected_up_by(1);
    }

    fn move_selected_down(&mut self) {
        self.move_selected_down_by(1);
    }

    fn move_selected_up_full_page(&mut self) {
        self.move_selected_up_by(max(self.num_displayed.unwrap(), 1));
    }

    fn move_selected_down_full_page(&mut self) {
        self.move_selected_down_by(max(self.num_displayed.unwrap(), 1));
    }

    fn move_selected_up_half_page(&mut self) {
        self.move_selected_up_by(max(ceil_div(self.num_displayed.unwrap(), 2), 1));
    }

    fn move_selected_down_half_page(&mut self) {
        self.move_selected_down_by(max(ceil_div(self.num_displayed.unwrap(), 2), 1));
    }

    fn move_selected_top(&mut self) {
        self.move_primary_sel(0);
    }

    fn move_selected_bottom(&mut self) {
        self.move_primary_sel(self.results.len().saturating_sub(1));
    }

    fn move_primary_sel(&mut self, idx: usize) {
        self.selected = match &self.selected {
            Selected::Single(_) => Selected::Single(idx),
            Selected::Multi(MultiSelected { anchor, .. }) => Selected::Multi(MultiSelected {
                anchor: *anchor,
                primary: idx,
            }),
        };
    }

    fn toggle_selected_inclusion(&mut self) {
        let all_included = self.selected_fields().iter().all(|res| res.included);
        self.selected_fields_mut().iter_mut().for_each(|selected| {
            selected.included = !all_included;
        });
    }

    fn toggle_all_selected(&mut self) {
        let all_included = self.results.iter().all(|res| res.included);
        self.results
            .iter_mut()
            .for_each(|res| res.included = !all_included);
    }

    // TODO: add tests
    fn selected_range(&self) -> (usize, usize) {
        match &self.selected {
            Selected::Single(sel) => (*sel, *sel),
            Selected::Multi(ms) => ms.ordered(),
        }
    }

    fn selected_fields(&self) -> &[SearchResult] {
        if self.results.is_empty() {
            return &[];
        }
        let (low, high) = self.selected_range();
        &self.results[low..=high]
    }

    fn selected_fields_mut(&mut self) -> &mut [SearchResult] {
        if self.results.is_empty() {
            return &mut [];
        }
        let (low, high) = self.selected_range();
        &mut self.results[low..=high]
    }

    pub(crate) fn primary_selected_field_mut(&mut self) -> &mut SearchResult {
        let sel = self.primary_selected_pos();
        &mut self.results[sel]
    }

    pub(crate) fn primary_selected_pos(&self) -> usize {
        match self.selected {
            Selected::Single(sel) => sel,
            Selected::Multi(MultiSelected { primary, .. }) => primary,
        }
    }

    fn toggle_multiselect_mode(&mut self) {
        self.selected = match &self.selected {
            Selected::Single(sel) => Selected::Multi(MultiSelected {
                anchor: *sel,
                primary: *sel,
            }),
            Selected::Multi(MultiSelected { primary, .. }) => Selected::Single(*primary),
        };
    }

    pub(crate) fn is_selected(&self, idx: usize) -> bool {
        match &self.selected {
            Selected::Single(sel) => idx == *sel,
            Selected::Multi(ms) => {
                let (low, high) = ms.ordered();
                idx >= low && idx <= high
            }
        }
    }

    fn multiselect_enabled(&self) -> bool {
        match &self.selected {
            Selected::Single(_) => false,
            Selected::Multi(_) => true,
        }
    }

    pub(crate) fn is_primary_selected(&self, idx: usize) -> bool {
        idx == self.primary_selected_pos()
    }

    fn flip_multiselect_direction(&mut self) {
        match &mut self.selected {
            Selected::Single(_) => {}
            Selected::Multi(ms) => {
                ms.flip_direction();
            }
        }
    }

    pub fn set_search_completed_now(&mut self) {
        self.search_completed = Some(Instant::now());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FocussedSection {
    SearchFields,
    SearchResults,
}

#[derive(Debug)]
pub struct SearchFieldsState {
    pub focussed_section: FocussedSection,
    pub search_state: Option<SearchState>, // Becomes Some when search begins
    pub debounce_timer: Option<JoinHandle<()>>,
}

impl Default for SearchFieldsState {
    fn default() -> Self {
        Self {
            focussed_section: FocussedSection::SearchFields,
            search_state: None,
            debounce_timer: None,
        }
    }
}

#[derive(Debug)]
pub enum Screen {
    SearchFields(SearchFieldsState),
    PerformingReplacement(PerformingReplacementState),
    Results(ReplaceState),
}

impl Screen {
    fn name(&self) -> &str {
        // TODO: is there a better way of doing this?
        match &self {
            Screen::SearchFields { .. } => "SearchFields",
            Screen::PerformingReplacement(_) => "PerformingReplacement",
            Screen::Results(_) => "Results",
        }
    }

    // TODO: delete
    // fn unwrap_search_fields_state(&self) -> &SearchFieldsState {
    //     let Screen::SearchFields(ref search_fields_state) = self else {
    //         panic!(
    //             "Expected current_screen to be SearchFields, found {}",
    //             self.name()
    //         );
    //     };
    //     search_fields_state
    // }

    fn unwrap_search_fields_state_mut(&mut self) -> &mut SearchFieldsState {
        let name = self.name().to_owned();
        let Screen::SearchFields(ref mut search_fields_state) = self else {
            panic!("Expected current_screen to be SearchFields, found {name}");
        };
        search_fields_state
    }
}

#[derive(Clone, Debug)]
pub struct AppError {
    pub name: String,
    pub long: String,
}

#[derive(Debug)]
pub enum Popup {
    Error,
    Help,
    Text { title: String, body: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppRunConfig {
    pub include_hidden: bool,
    pub advanced_regex: bool,
    pub immediate_search: bool,
    pub immediate_replace: bool,
    pub print_results: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for AppRunConfig {
    fn default() -> Self {
        Self {
            include_hidden: false,
            advanced_regex: false,
            immediate_search: false,
            immediate_replace: false,
            print_results: false,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub current_screen: Screen,
    pub search_fields: SearchFields,
    pub directory: PathBuf,
    pub config: Config,
    pub event_sender: UnboundedSender<Event>,
    errors: Vec<AppError>,
    include_hidden: bool,
    immediate_replace: bool,
    print_results: bool,
    popup: Option<Popup>,
    advanced_regex: bool,
}

impl<'a> App {
    fn new(
        directory: PathBuf,
        search_field_values: &SearchFieldValues<'a>,
        event_sender: UnboundedSender<Event>,
        app_run_config: &AppRunConfig,
    ) -> Self {
        let config = load_config().expect("Failed to read config file");

        let search_fields = SearchFields::with_values(
            search_field_values,
            config.search.disable_prepopulated_fields,
        );

        let mut app = Self {
            current_screen: Screen::SearchFields(SearchFieldsState::default()),
            search_fields,
            directory,
            include_hidden: app_run_config.include_hidden,
            config,
            errors: vec![],
            popup: None,
            event_sender,
            immediate_replace: app_run_config.immediate_replace,
            print_results: app_run_config.print_results,
            advanced_regex: app_run_config.advanced_regex,
        };

        if app_run_config.immediate_search {
            app.perform_search_if_valid();
        }

        app
    }

    pub fn new_with_receiver(
        directory: PathBuf,
        search_field_values: &SearchFieldValues<'a>,
        app_run_config: &AppRunConfig,
    ) -> (Self, UnboundedReceiver<Event>) {
        let (event_sender, app_event_receiver) = mpsc::unbounded_channel();
        let app = Self::new(directory, search_field_values, event_sender, app_run_config);
        (app, app_event_receiver)
    }

    fn cancel_search(&mut self) {
        if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(SearchState { cancelled, .. }),
            ..
        }) = &mut self.current_screen
        {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    fn cancel_replacement(&mut self) {
        if let Screen::PerformingReplacement(PerformingReplacementState { cancelled, .. }) =
            &mut self.current_screen
        {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_in_progress_tasks(&mut self) {
        self.cancel_search();
        self.cancel_replacement();
    }

    pub fn reset(&mut self) {
        self.cancel_in_progress_tasks();
        *self = Self::new(
            self.directory.clone(),
            &SearchFieldValues::default(),
            self.event_sender.clone(),
            &AppRunConfig {
                include_hidden: self.include_hidden,
                advanced_regex: self.advanced_regex,
                immediate_search: false,
                immediate_replace: self.immediate_replace,
                print_results: self.print_results,
            },
        );
    }

    pub async fn background_processing_recv(&mut self) -> Option<BackgroundProcessingEvent> {
        match &mut self.current_screen {
            Screen::SearchFields(SearchFieldsState { search_state, .. }) => {
                if let Some(search_state) = search_state {
                    search_state.processing_receiver.recv().await
                } else {
                    None
                }
            }
            Screen::PerformingReplacement(PerformingReplacementState {
                processing_receiver,
                ..
            }) => processing_receiver.recv().await,
            Screen::Results(_) => None,
        }
    }

    pub fn handle_app_event(&mut self, event: &AppEvent) -> EventHandlingResult {
        match event {
            AppEvent::Rerender => EventHandlingResult::Rerender,
            AppEvent::PerformSearch => self.perform_search_if_valid(),
        }
    }

    pub fn perform_search_if_valid(&mut self) -> EventHandlingResult {
        // TODO(autosearch): anything else to do here?

        // TODO(autosearch): only show popup if hit enter

        let (background_processing_sender, background_processing_receiver) =
            mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        if let Some(parsed_fields) = self.validate_fields().unwrap() {
            Self::spawn_update_search_results(
                parsed_fields,
                &background_processing_sender,
                self.event_sender.clone(),
                cancelled.clone(),
            );
            let Screen::SearchFields(ref mut search_fields_state) = self.current_screen else {
                return EventHandlingResult::None;
            };
            search_fields_state.search_state =
                Some(SearchState::new(background_processing_receiver, cancelled));
        } else {
            let Screen::SearchFields(ref mut search_fields_state) = self.current_screen else {
                return EventHandlingResult::None;
            };
            search_fields_state.focussed_section = FocussedSection::SearchFields;
        }

        EventHandlingResult::Rerender
    }

    pub fn trigger_replacement(&mut self) {
        let temp_placeholder = Screen::SearchFields(SearchFieldsState {
            search_state: None,
            focussed_section: FocussedSection::SearchFields,
            debounce_timer: None,
        });
        match mem::replace(
            &mut self.current_screen,
            temp_placeholder, // Will get reset if we are not on `SearchComplete` screen
        ) {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => {
                let (background_processing_sender, background_processing_receiver) =
                    mpsc::unbounded_channel();
                let cancelled = Arc::new(AtomicBool::new(false));
                let total_replacements = state.results.iter().filter(|r| r.included).count();
                let replacements_completed = Arc::new(AtomicUsize::new(0));

                replace::perform_replacement(
                    state.results,
                    background_processing_sender.clone(),
                    cancelled.clone(),
                    replacements_completed.clone(),
                    self.event_sender.clone(),
                );

                self.current_screen =
                    Screen::PerformingReplacement(PerformingReplacementState::new(
                        background_processing_receiver,
                        cancelled,
                        replacements_completed,
                        total_replacements,
                    ));
            }
            screen => self.current_screen = screen,
        }
    }

    pub fn handle_background_processing_event(
        &mut self,
        event: BackgroundProcessingEvent,
    ) -> EventHandlingResult {
        match event {
            BackgroundProcessingEvent::AddSearchResults(mut results) => {
                let mut rerender = false;
                if let Screen::SearchFields(SearchFieldsState {
                    search_state: Some(search_in_progress_state),
                    ..
                }) = &mut self.current_screen
                {
                    search_in_progress_state.results.append(&mut results);

                    // Slightly random duration so that time taken isn't a round number
                    if search_in_progress_state.last_render.elapsed() >= Duration::from_millis(92) {
                        rerender = true;
                        search_in_progress_state.last_render = Instant::now();
                    }
                }
                if rerender {
                    EventHandlingResult::Rerender
                } else {
                    EventHandlingResult::None
                }
            }
            BackgroundProcessingEvent::SearchCompleted => {
                if let Screen::SearchFields(SearchFieldsState {
                    search_state: Some(state),
                    ..
                }) = &mut self.current_screen
                {
                    state.set_search_completed_now();
                    if self.immediate_replace {
                        self.trigger_replacement();
                    }
                }
                EventHandlingResult::Rerender
            }
            BackgroundProcessingEvent::ReplacementCompleted(replace_state) => {
                if self.print_results {
                    let results = format_replacement_results(
                        replace_state.num_successes,
                        Some(replace_state.num_ignored),
                        Some(&replace_state.errors),
                    );
                    EventHandlingResult::Exit(Some(results))
                } else {
                    self.current_screen = Screen::Results(replace_state);
                    EventHandlingResult::Rerender
                }
            }
        }
    }

    fn handle_key_searching(&mut self, key: &KeyEvent) -> EventHandlingResult {
        match (key.code, key.modifiers) {
            (KeyCode::Char('u'), KeyModifiers::ALT) => {
                self.unlock_prepopulated_fields();
            }
            (KeyCode::Enter, _) => {
                let Screen::SearchFields(ref mut search_fields_state) = self.current_screen else {
                    return EventHandlingResult::None;
                };
                search_fields_state.focussed_section = FocussedSection::SearchResults;
            }
            (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::ALT) => {
                self.search_fields
                    .focus_prev(self.config.search.disable_prepopulated_fields);
            }
            (KeyCode::Tab, _) => {
                self.search_fields
                    .focus_next(self.config.search.disable_prepopulated_fields);
            }
            (code, modifiers) => {
                // TODO(autosearch): refresh replacement results only when replacing
                // - if search has completed, just iterate and replace
                // - what happens if we start typing in the replacement field mid-way through a search? Need to ensure future results have correct replacement
                // - need to ensure we keep the scroll position and all toggle state

                if let FieldName::FixedStrings = self.search_fields.highlighted_field().name {
                    // TODO: ideally this should only happen when the field is checked, but for now this will do
                    self.search_fields.search_mut().clear_error();
                }
                self.search_fields.highlighted_field_mut().handle_keys(
                    code,
                    modifiers,
                    self.config.search.disable_prepopulated_fields,
                );

                // Debounce search requests
                let Screen::SearchFields(ref mut search_fields_state) = self.current_screen else {
                    return EventHandlingResult::None;
                };
                if let Some(timer) = search_fields_state.debounce_timer.take() {
                    timer.abort();
                }
                let event_sender = self.event_sender.clone();
                search_fields_state.debounce_timer = Some(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = event_sender.send(Event::App(AppEvent::PerformSearch));
                }));
            }
        }
        EventHandlingResult::Rerender
    }

    /// Should only be called on `Screen::SearchFields`
    fn try_handle_key_search(&mut self, key: &KeyEvent) -> Option<EventHandlingResult> {
        assert!(
            matches!(self.current_screen, Screen::SearchFields(_)),
            "Expected current_screen to be SearchFields, found {}",
            self.current_screen.name()
        );

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.trigger_replacement();
                Some(EventHandlingResult::Rerender)
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.cancel_search();
                let search_fields_state = self.current_screen.unwrap_search_fields_state_mut();
                search_fields_state.focussed_section = FocussedSection::SearchFields;
                self.event_sender
                    .send(Event::App(AppEvent::Rerender))
                    .unwrap();
                Some(EventHandlingResult::Rerender)
            }
            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                self.set_popup(Popup::Text{
                    title: "Command deprecated".to_string(),
                    body: "Pressing `o` to open the selected file in your editor is deprecated.\n\nPlease use `e` instead.".to_string(),
                });
                Some(EventHandlingResult::Rerender)
            }
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                let search_fields_state = self.current_screen.unwrap_search_fields_state_mut();
                if let Some(ref mut search_in_progress_state) = search_fields_state.search_state {
                    let selected = search_in_progress_state.primary_selected_field_mut();
                    self.event_sender
                        .send(Event::LaunchEditor((
                            selected.path.clone(),
                            selected.line_number,
                        )))
                        .unwrap();
                }
                Some(EventHandlingResult::Rerender)
            }
            _ => None,
        }
    }

    pub fn handle_key_event(&mut self, key: &KeyEvent) -> EventHandlingResult {
        if key.kind == KeyEventKind::Release {
            return EventHandlingResult::Rerender;
        }

        if (key.code, key.modifiers) == (KeyCode::Char('c'), KeyModifiers::CONTROL) {
            self.reset();
            return EventHandlingResult::Exit(None);
        }

        if self.popup.is_some() {
            self.clear_popup();
            return EventHandlingResult::Rerender;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                if self.multiselect_enabled() {
                    self.toggle_multiselect_mode();
                    return EventHandlingResult::Rerender;
                } else {
                    self.reset();
                    return EventHandlingResult::Exit(None);
                }
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.reset();
                return EventHandlingResult::Rerender;
            }
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                self.set_popup(Popup::Help);
                return EventHandlingResult::Rerender;
            }
            (_, _) => {}
        }

        match &mut self.current_screen {
            Screen::SearchFields(search_fields_state) => {
                match search_fields_state.focussed_section {
                    FocussedSection::SearchFields => self.handle_key_searching(key),
                    FocussedSection::SearchResults => {
                        if let Some(res) = self.try_handle_key_search(key) {
                            res
                        } else {
                            match self
                                .current_screen
                                .unwrap_search_fields_state_mut()
                                .search_state
                            {
                                Some(ref mut state) => state.handle_key(key),
                                None => {
                                    panic!("Focussed on search results but search_state is None")
                                }
                            }
                        }
                    }
                }
            }
            Screen::PerformingReplacement(_) => EventHandlingResult::Rerender,
            Screen::Results(replace_state) => replace_state.handle_key_results(key),
        }
    }

    fn validate_fields(&mut self) -> anyhow::Result<Option<FileSearcher>> {
        let search_config = SearchConfiguration {
            search_text: self.search_fields.search().text(),
            replacement_text: self.search_fields.replace().text(),
            fixed_strings: self.search_fields.fixed_strings().checked,
            advanced_regex: self.search_fields.advanced_regex,
            include_globs: Some(self.search_fields.include_files().text()),
            exclude_globs: Some(self.search_fields.exclude_files().text()),
            match_whole_word: self.search_fields.whole_word().checked,
            match_case: self.search_fields.match_case().checked,
            include_hidden: self.include_hidden,
            directory: self.directory.clone(),
        };

        let mut error_handler = AppErrorHandler::new();
        let result = validate_search_configuration(search_config, &mut error_handler)?;
        error_handler.apply_to_app(self);

        match result {
            ValidationResult::Success(searcher) => Ok(Some(searcher)),
            ValidationResult::ValidationErrors => {
                self.set_popup(Popup::Error);
                Ok(None)
            }
        }
    }

    pub fn spawn_update_search_results(
        parsed_fields: FileSearcher,
        sender: &UnboundedSender<BackgroundProcessingEvent>,
        event_sender: UnboundedSender<Event>,
        cancelled: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let sender = sender.clone();
        tokio::spawn(async move {
            let sender_for_search = sender.clone();
            let mut search_handle = task::spawn_blocking(move || {
                parsed_fields.walk_files(Some(&cancelled), || {
                    let sender = sender_for_search.clone();
                    Box::new(move |results| {
                        // Ignore error - likely state reset, thread about to be killed
                        let _ = sender.send(BackgroundProcessingEvent::AddSearchResults(results));

                        WalkState::Continue
                    })
                });
            });

            let mut rerender_interval = tokio::time::interval(Duration::from_millis(92)); // Slightly random duration so that time taken isn't a round number
            rerender_interval.tick().await;

            loop {
                tokio::select! {
                    res = &mut search_handle => {
                        if let Err(e) = res {
                            warn!("Search thread panicked: {e}");
                        }
                        break;
                    },
                    _ = rerender_interval.tick() => {
                        let _ = event_sender.send(Event::App(AppEvent::Rerender));
                    }
                }
            }

            if let Err(err) = sender.send(BackgroundProcessingEvent::SearchCompleted) {
                // Log and ignore error: likely have gone back to previous screen
                warn!("Found error when attempting to send SearchCompleted event: {err}");
            }
        })
    }

    pub fn show_popup(&self) -> bool {
        self.popup.is_some()
    }

    pub fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    pub fn errors(&self) -> Vec<AppError> {
        let app_errors = self.errors.clone().into_iter();
        let field_errors = self.search_fields.errors().into_iter();
        app_errors.chain(field_errors).collect()
    }

    pub fn add_error(&mut self, error: AppError) {
        self.popup = Some(Popup::Error);
        self.errors.push(error);
    }

    fn clear_popup(&mut self) {
        self.popup = None;
        self.errors.clear();
    }

    fn set_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
    }

    pub fn keymaps_all(&self) -> Vec<(&str, String)> {
        self.keymaps_impl(false)
    }

    pub fn keymaps_compact(&self) -> Vec<(&str, String)> {
        self.keymaps_impl(true)
    }

    #[allow(clippy::too_many_lines)]
    fn keymaps_impl(&self, compact: bool) -> Vec<(&str, String)> {
        enum Show {
            Both,
            FullOnly,
            CompactOnly,
        }

        // TODO(autosearch): update these, including:
        // - back/forward between SearchFields and SearchResults
        // - <more>
        let current_screen_keys = match &self.current_screen {
            Screen::SearchFields(search_fields_state) => {
                match search_fields_state.focussed_section {
                    FocussedSection::SearchFields => {
                        let mut keys = vec![
                            ("<enter>", "jump to results", Show::Both),
                            ("<tab>", "focus next", Show::Both),
                            ("<S-tab>", "focus previous", Show::FullOnly),
                            ("<space>", "toggle checkbox", Show::FullOnly),
                        ];
                        if self.config.search.disable_prepopulated_fields
                            && self.search_fields.fields.iter().any(|f| f.set_by_cli)
                        {
                            keys.push(("<A-u>", "unlock pre-populated fields", Show::FullOnly));
                        }
                        keys
                    }
                    FocussedSection::SearchResults => {
                        let mut keys = vec![
                            ("<space>", "toggle", Show::Both),
                            ("a", "toggle all", Show::FullOnly),
                            ("v", "toggle multi-select mode", Show::FullOnly),
                            ("<A-;>", "flip multi-select direction", Show::FullOnly),
                            ("e", "open in editor", Show::FullOnly),
                            ("<C-o>", "back to search fields", Show::Both),
                            ("j", "up", Show::FullOnly),
                            ("k", "down", Show::FullOnly),
                            ("<C-u>", "up half a page", Show::FullOnly),
                            ("<C-d>", "down half a page", Show::FullOnly),
                            ("<C-b>", "up a full page", Show::FullOnly),
                            ("<C-f>", "down a full page", Show::FullOnly),
                            ("g", "jump to top", Show::FullOnly),
                            ("G", "jump to bottom", Show::FullOnly),
                        ];
                        if search_fields_state
                            .search_state
                            .as_ref()
                            .is_some_and(SearchState::search_has_completed)
                        {
                            keys.push(("<enter>", "replace selected", Show::Both));
                        }
                        keys
                    }
                }
            }
            Screen::PerformingReplacement(_) => vec![],
            Screen::Results(ref replace_state) => {
                if !replace_state.errors.is_empty() {
                    vec![("<j>", "down", Show::Both), ("<k>", "up", Show::Both)]
                } else {
                    vec![]
                }
            }
        };

        let on_search_results = if let Screen::SearchFields(ref s) = self.current_screen {
            s.focussed_section == FocussedSection::SearchResults
        } else {
            false
        };
        let esc_help = format!(
            "quit / close popup{}",
            if on_search_results {
                " / exit multi-select"
            } else {
                ""
            }
        );

        let additional_keys = vec![
            (
                "<C-r>",
                "reset",
                if on_search_results {
                    Show::FullOnly
                } else {
                    Show::Both
                },
            ),
            ("<C-h>", "help", Show::Both),
            (
                "<esc>",
                if self.popup.is_some() {
                    "close popup"
                } else if self.multiselect_enabled() {
                    "exit multi-select"
                } else {
                    "quit"
                },
                Show::CompactOnly,
            ),
            ("<esc>", &esc_help, Show::FullOnly),
            ("<C-c>", "quit", Show::FullOnly),
        ];

        let all_keys = current_screen_keys.into_iter().chain(additional_keys);

        all_keys
            .filter_map(move |(from, to, show)| {
                let include = match show {
                    Show::Both => true,
                    Show::CompactOnly => compact,
                    Show::FullOnly => !compact,
                };
                if include {
                    Some((from, to.to_owned()))
                } else {
                    None
                }
            })
            .collect()
    }

    fn multiselect_enabled(&self) -> bool {
        match &self.current_screen {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => state.multiselect_enabled(),
            _ => false,
        }
    }

    fn toggle_multiselect_mode(&mut self) {
        match &mut self.current_screen {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => state.toggle_multiselect_mode(),
            _ => panic!(
                "Tried to disable multi-select on {:?}",
                self.current_screen.name()
            ),
        }
    }

    fn unlock_prepopulated_fields(&mut self) {
        for field in &mut self.search_fields.fields {
            field.set_by_cli = false;
        }
    }
}

#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct AppErrorHandler {
    search_errors: Option<(String, String)>,
    include_errors: Option<(String, String)>,
    exclude_errors: Option<(String, String)>,
}

impl AppErrorHandler {
    fn new() -> Self {
        Self {
            search_errors: None,
            include_errors: None,
            exclude_errors: None,
        }
    }

    fn apply_to_app(&self, app: &mut App) {
        if let Some((error, detail)) = &self.search_errors {
            app.search_fields
                .search_mut()
                .set_error(error.clone(), detail.clone());
        }

        if let Some((error, detail)) = &self.include_errors {
            app.search_fields
                .include_files_mut()
                .set_error(error.clone(), detail.clone());
        }

        if let Some((error, detail)) = &self.exclude_errors {
            app.search_fields
                .exclude_files_mut()
                .set_error(error.clone(), detail.clone());
        }
    }
}

impl ValidationErrorHandler for AppErrorHandler {
    fn handle_search_text_error(&mut self, error: &str, detail: &str) {
        self.search_errors = Some((error.to_owned(), detail.to_string()));
    }

    fn handle_include_files_error(&mut self, error: &str, detail: &str) {
        self.include_errors = Some((error.to_owned(), detail.to_string()));
    }

    fn handle_exclude_files_error(&mut self, error: &str, detail: &str) {
        self.exclude_errors = Some((error.to_owned(), detail.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use frep_core::replace::{ReplaceResult, ReplaceStats};
    use rand::Rng;
    use std::{env::current_dir, path::Path};

    use super::*;
    use frep_core::line_reader::LineEnding;

    fn random_num() -> usize {
        let mut rng = rand::rng();
        rng.random_range(1..10000)
    }

    fn search_result(included: bool) -> SearchResult {
        SearchResult {
            path: Path::new("random/file").to_path_buf(),
            line_number: random_num(),
            line: "foo".to_owned(),
            line_ending: LineEnding::Lf,
            replacement: "bar".to_owned(),
            included,
            replace_result: None,
        }
    }

    fn build_test_results(num_results: usize) -> Vec<SearchResult> {
        (0..num_results)
            .map(|i| SearchResult {
                path: PathBuf::from(format!("test{i}.txt")),
                line_number: 1,
                line: format!("test line {i}").to_string(),
                line_ending: LineEnding::Lf,
                replacement: format!("replacement {i}").to_string(),
                included: true,
                replace_result: None,
            })
            .collect()
    }

    fn build_test_search_state(num_results: usize) -> SearchState {
        let results = build_test_results(num_results);
        build_test_search_state_with_results(results)
    }

    fn build_test_search_state_with_results(results: Vec<SearchResult>) -> SearchState {
        let (_processing_sender, processing_receiver) = mpsc::unbounded_channel();
        SearchState {
            results,
            selected: Selected::Single(0),
            view_offset: 0,
            num_displayed: Some(5),
            processing_receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
            last_render: Instant::now(),
            search_started: Instant::now(),
            search_completed: None,
        }
    }

    #[test]
    fn test_toggle_all_selected_when_all_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result(true),
            search_result(true),
            search_result(true),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![false, false, false]
        );
    }

    #[test]
    fn test_toggle_all_selected_when_none_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result(false),
            search_result(false),
            search_result(false),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![true, true, true]
        );
    }

    #[test]
    fn test_toggle_all_selected_when_some_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result(true),
            search_result(false),
            search_result(true),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![true, true, true]
        );
    }

    #[test]
    fn test_toggle_all_selected_when_no_results() {
        let mut search_state = build_test_search_state_with_results(vec![]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![] as Vec<bool>
        );
    }

    fn success_result() -> SearchResult {
        SearchResult {
            path: Path::new("random/file").to_path_buf(),
            line_number: random_num(),
            line: "foo".to_owned(),
            line_ending: LineEnding::Lf,
            replacement: "bar".to_owned(),
            included: true,
            replace_result: Some(ReplaceResult::Success),
        }
    }

    fn ignored_result() -> SearchResult {
        SearchResult {
            path: Path::new("random/file").to_path_buf(),
            line_number: random_num(),
            line: "foo".to_owned(),
            line_ending: LineEnding::Lf,
            replacement: "bar".to_owned(),
            included: false,
            replace_result: None,
        }
    }

    fn error_result() -> SearchResult {
        SearchResult {
            path: Path::new("random/file").to_path_buf(),
            line_number: random_num(),
            line: "foo".to_owned(),
            line_ending: LineEnding::Lf,
            replacement: "bar".to_owned(),
            included: true,
            replace_result: Some(ReplaceResult::Error("error".to_owned())),
        }
    }

    fn build_test_app(results: Vec<SearchResult>) -> App {
        let (event_sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(
            current_dir().unwrap(),
            &SearchFieldValues::default(),
            event_sender,
            &AppRunConfig::default(),
        );
        app.current_screen = Screen::SearchFields(SearchFieldsState {
            search_state: Some(build_test_search_state_with_results(results)),
            focussed_section: FocussedSection::SearchResults,
            last_search_request: None,
        });
        app
    }

    #[tokio::test]
    async fn test_calculate_statistics_all_success() {
        let app = build_test_app(vec![success_result(), success_result(), success_result()]);
        let stats = if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(state),
            ..
        }) = app.current_screen
        {
            let (results, _num_ignored) = scooter_core::replace::split_results(state.results);
            frep_core::replace::calculate_statistics(results)
        } else {
            panic!("Expected SearchFields with Some search_state");
        };

        assert_eq!(
            stats,
            ReplaceStats {
                num_successes: 3,
                errors: vec![],
            }
        );
    }

    #[tokio::test]
    async fn test_calculate_statistics_with_ignores_and_errors() {
        let error_result = error_result();
        let app = build_test_app(vec![
            success_result(),
            ignored_result(),
            success_result(),
            error_result.clone(),
            ignored_result(),
        ]);
        let stats = if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(state),
            ..
        }) = app.current_screen
        {
            let (results, _num_ignored) = scooter_core::replace::split_results(state.results);
            frep_core::replace::calculate_statistics(results)
        } else {
            panic!("Expected SearchFields with Some search_state");
        };

        assert_eq!(
            stats,
            ReplaceStats {
                num_successes: 2,
                errors: vec![error_result],
            }
        );
    }

    #[tokio::test]
    async fn test_search_state_toggling() {
        fn included(state: &SearchState) -> Vec<bool> {
            state.results.iter().map(|r| r.included).collect::<Vec<_>>()
        }

        let mut state = build_test_search_state(3);

        assert_eq!(included(&state), [true, true, true]);
        state.toggle_selected_inclusion();
        assert_eq!(included(&state), [false, true, true]);
        state.toggle_selected_inclusion();
        assert_eq!(included(&state), [true, true, true]);
        state.toggle_selected_inclusion();
        assert_eq!(included(&state), [false, true, true]);
        state.move_selected_down();
        state.toggle_selected_inclusion();
        assert_eq!(included(&state), [false, false, true]);
        state.toggle_selected_inclusion();
        assert_eq!(included(&state), [false, true, true]);
    }

    #[tokio::test]
    async fn test_search_state_movement_single() {
        let mut state = build_test_search_state(3);

        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(1));
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(2));
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(1));
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(2));
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(1));
    }

    #[tokio::test]
    async fn test_search_state_movement_top_bottom() {
        let mut state = build_test_search_state(3);

        state.move_selected_top();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_bottom();
        assert_eq!(state.selected, Selected::Single(2));
        state.move_selected_bottom();
        assert_eq!(state.selected, Selected::Single(2));
        state.move_selected_top();
        assert_eq!(state.selected, Selected::Single(0));
    }

    #[tokio::test]
    async fn test_search_state_movement_half_page_increments() {
        let mut state = build_test_search_state(8);

        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down_half_page();
        assert_eq!(state.selected, Selected::Single(3));
        state.move_selected_down_half_page();
        assert_eq!(state.selected, Selected::Single(6));
        state.move_selected_down_half_page();
        assert_eq!(state.selected, Selected::Single(7));
        state.move_selected_up_half_page();
        assert_eq!(state.selected, Selected::Single(4));
        state.move_selected_up_half_page();
        assert_eq!(state.selected, Selected::Single(1));
        state.move_selected_up_half_page();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_up_half_page();
        assert_eq!(state.selected, Selected::Single(7));
        state.move_selected_up_half_page();
        assert_eq!(state.selected, Selected::Single(4));
        state.move_selected_down_half_page();
        assert_eq!(state.selected, Selected::Single(7));
        state.move_selected_down_half_page();
        assert_eq!(state.selected, Selected::Single(0));
    }

    #[tokio::test]
    async fn test_search_state_movement_page_increments() {
        let mut state = build_test_search_state(12);

        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down_full_page();
        assert_eq!(state.selected, Selected::Single(5));
        state.move_selected_down_full_page();
        assert_eq!(state.selected, Selected::Single(10));
        state.move_selected_down_full_page();
        assert_eq!(state.selected, Selected::Single(11));
        state.move_selected_down_full_page();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(11));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(6));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(1));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(11));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(6));
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(5));
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(4));
        state.move_selected_up_full_page();
        assert_eq!(state.selected, Selected::Single(0));
    }

    #[test]
    fn test_selected_fields_movement() {
        let mut results = build_test_results(10);
        let mut state = build_test_search_state_with_results(results.clone());

        assert_eq!(state.selected, Selected::Single(0));
        assert_eq!(state.selected_fields(), &mut results[0..=0]);

        state.toggle_multiselect_mode();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 0,
                primary: 0,
            })
        );
        assert_eq!(state.selected_fields(), &mut results[0..=0]);

        state.move_selected_down();
        state.move_selected_down();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 0,
                primary: 2,
            })
        );
        assert_eq!(state.selected_fields(), &mut results[0..=2]);

        state.toggle_multiselect_mode();
        assert_eq!(state.selected, Selected::Single(2));
        assert_eq!(state.selected_fields(), &mut results[2..=2]);

        state.toggle_multiselect_mode();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 2,
                primary: 2,
            })
        );
        assert_eq!(state.selected_fields(), &mut results[2..=2]);
    }

    #[test]
    fn test_selected_fields_toggling() {
        let mut state = build_test_search_state(6);

        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down();
        state.move_selected_down();
        state.move_selected_down();
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(4));
        state.toggle_multiselect_mode();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 4,
                primary: 4,
            })
        );
        assert_eq!(state.selected_fields(), &state.results[4..=4]);
        state.move_selected_up();
        state.move_selected_up();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 4,
                primary: 2,
            })
        );
        assert_eq!(state.selected_fields(), &state.results[2..=4]);
        assert_eq!(
            state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![true, true, true, true, true, true]
        );
        state.toggle_selected_inclusion();
        assert_eq!(
            state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![true, true, false, false, false, true]
        );
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 4,
                primary: 2,
            })
        );
        assert_eq!(state.selected_fields(), &state.results[2..=4]);
        state.toggle_multiselect_mode();
        assert_eq!(state.selected, Selected::Single(2));
        assert_eq!(state.selected_fields(), &state.results[2..=2]);
        state.move_selected_up();
        state.move_selected_up();
        assert_eq!(state.selected, Selected::Single(0));
        assert_eq!(state.selected_fields(), &state.results[0..=0]);
        state.toggle_selected_inclusion();
        assert_eq!(
            state
                .results
                .iter()
                .map(|res| res.included)
                .collect::<Vec<_>>(),
            vec![false, true, false, false, false, true]
        );
    }

    #[test]
    fn test_flip_multi_select_direction() {
        let mut state = build_test_search_state(10);
        assert_eq!(state.selected, Selected::Single(0));
        state.flip_multiselect_direction();
        assert_eq!(state.selected, Selected::Single(0));
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(1));
        state.toggle_multiselect_mode();
        state.move_selected_down();
        state.move_selected_down();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 1,
                primary: 3,
            })
        );
        state.flip_multiselect_direction();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 3,
                primary: 1,
            })
        );
        state.move_selected_up();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 3,
                primary: 0,
            })
        );
        state.flip_multiselect_direction();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 0,
                primary: 3,
            })
        );
        state.move_selected_bottom();
        assert_eq!(
            state.selected,
            Selected::Multi(MultiSelected {
                anchor: 0,
                primary: 9,
            })
        );
        state.move_selected_down();
        assert_eq!(state.selected, Selected::Single(0));
    }
}
