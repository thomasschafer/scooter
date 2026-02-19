use std::{
    cmp::{max, min},
    io::Cursor,
    iter::{self, Iterator},
    mem,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use ignore::WalkState;
use log::{debug, warn};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{self, JoinHandle},
};

use crate::{
    commands::{
        Command, CommandGeneral, CommandSearchFields, CommandSearchFocusFields,
        CommandSearchFocusResults, KeyMap, display_conflict_errors,
    },
    config::Config,
    errors::AppError,
    fields::{FieldName, SearchFieldValues, SearchFields},
    keyboard::{KeyCode, KeyEvent, KeyModifiers},
    line_reader::{BufReadExt, LineEnding},
    replace::{self, PerformingReplacementState, ReplaceState},
    replace::{add_replacement, replacement_if_match},
    search::Searcher,
    search::{FileSearcher, ParsedSearchConfig, SearchResult, SearchResultWithReplacement},
    utils::{Either, Either::Left, Either::Right, ceil_div},
    validation::{
        DirConfig, SearchConfig, ValidationErrorHandler, ValidationResult,
        validate_search_configuration,
    },
};

#[derive(Debug, Clone)]
pub enum InputSource {
    Directory(PathBuf),
    Stdin(Arc<String>),
}

#[derive(Debug)]
pub enum ExitState {
    Stats(ReplaceState),
    StdinState(ExitAndReplaceState),
}

#[derive(Debug)]
pub enum EventHandlingResult {
    Rerender,
    Exit(Option<Box<ExitState>>),
    None,
}

impl EventHandlingResult {
    pub(crate) fn new_exit_stats(stats: ReplaceState) -> EventHandlingResult {
        Self::new_exit(ExitState::Stats(stats))
    }

    fn new_exit(exit_state: ExitState) -> EventHandlingResult {
        EventHandlingResult::Exit(Some(Box::new(exit_state)))
    }
}

#[derive(Debug)]
pub enum BackgroundProcessingEvent {
    AddSearchResult(SearchResult),
    AddSearchResults(Vec<SearchResult>),
    SearchCompleted,
    ReplacementCompleted(ReplaceState),
    UpdateReplacements {
        start: usize,
        end: usize,
        cancelled: Arc<AtomicBool>,
    },
    UpdateAllReplacements {
        cancelled: Arc<AtomicBool>,
    },
}

#[derive(Debug)]
pub enum AppEvent {
    PerformSearch,
    DismissToast { generation: u64 },
}

#[derive(Debug)]
pub enum InternalEvent {
    App(AppEvent),
    Background(BackgroundProcessingEvent),
}

#[derive(Debug)]
pub struct ExitAndReplaceState {
    pub stdin: Arc<String>,
    pub search_config: ParsedSearchConfig,
    pub replace_results: Vec<SearchResultWithReplacement>,
}

#[derive(Debug)]
pub enum Event {
    LaunchEditor((PathBuf, usize)),
    ExitAndReplace(ExitAndReplaceState),
    Rerender,
    Internal(InternalEvent),
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
    pub results: Vec<SearchResultWithReplacement>,

    selected: Selected,
    // TODO: make the view logic with scrolling etc. into a generic component
    pub view_offset: usize,           // Updated by UI, not app
    pub num_displayed: Option<usize>, // Updated by UI, not app

    processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    processing_sender: UnboundedSender<BackgroundProcessingEvent>,

    pub last_render: Instant,
    pub search_started: Instant,
    pub search_completed: Option<Instant>,
    pub cancelled: Arc<AtomicBool>,
}

impl SearchState {
    pub fn new(
        processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            results: vec![],
            selected: Selected::Single(0),
            view_offset: 0,
            num_displayed: None,
            processing_sender,
            processing_receiver,
            last_render: Instant::now(),
            search_started: Instant::now(),
            search_completed: None,
            cancelled,
        }
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
        let all_included = self
            .selected_fields()
            .iter()
            .all(|res| res.search_result.included);
        self.selected_fields_mut().iter_mut().for_each(|selected| {
            selected.search_result.included = !all_included;
        });
    }

    fn toggle_all_selected(&mut self) {
        let all_included = self.results.iter().all(|res| res.search_result.included);
        self.results
            .iter_mut()
            .for_each(|res| res.search_result.included = !all_included);
    }

    // TODO: add tests
    fn selected_range(&self) -> (usize, usize) {
        match &self.selected {
            Selected::Single(sel) => (*sel, *sel),
            Selected::Multi(ms) => ms.ordered(),
        }
    }

    fn selected_fields(&self) -> &[SearchResultWithReplacement] {
        if self.results.is_empty() {
            return &[];
        }
        let (low, high) = self.selected_range();
        &self.results[low..=high]
    }

    fn selected_fields_mut(&mut self) -> &mut [SearchResultWithReplacement] {
        if self.results.is_empty() {
            return &mut [];
        }
        let (low, high) = self.selected_range();
        &mut self.results[low..=high]
    }

    pub fn primary_selected_field_mut(&mut self) -> Option<&mut SearchResultWithReplacement> {
        let sel = self.primary_selected_pos();
        if !self.results.is_empty() {
            Some(&mut self.results[sel])
        } else {
            None
        }
    }

    pub fn primary_selected_pos(&self) -> usize {
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

    pub fn is_selected(&self, idx: usize) -> bool {
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

    pub fn is_primary_selected(&self, idx: usize) -> bool {
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
pub struct PreviewUpdateStatus {
    replace_debounce_timer: JoinHandle<()>,
    update_replacement_cancelled: Arc<AtomicBool>,
    replacements_updated: usize,
    total_replacements_to_update: usize,
}

impl PreviewUpdateStatus {
    fn new(
        replace_debounce_timer: JoinHandle<()>,
        update_replacement_cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            replace_debounce_timer,
            update_replacement_cancelled,
            replacements_updated: 0,
            total_replacements_to_update: 0,
        }
    }
}

#[derive(Debug)]
pub struct SearchFieldsState {
    pub focussed_section: FocussedSection,
    pub search_state: Option<SearchState>, // Becomes Some when search begins
    pub search_debounce_timer: Option<JoinHandle<()>>,
    pub preview_update_state: Option<PreviewUpdateStatus>,
}

impl Default for SearchFieldsState {
    fn default() -> Self {
        Self {
            focussed_section: FocussedSection::SearchFields,
            search_state: None,
            search_debounce_timer: None,
            preview_update_state: None,
        }
    }
}

impl SearchFieldsState {
    pub fn replacements_in_progress(&self) -> Option<(usize, usize)> {
        self.preview_update_state.as_ref().and_then(|p| {
            if p.replacements_updated != p.total_replacements_to_update {
                Some((p.replacements_updated, p.total_replacements_to_update))
            } else {
                None
            }
        })
    }

    pub fn cancel_preview_updates(&mut self) {
        if let Some(ref mut state) = self.preview_update_state {
            state.replace_debounce_timer.abort();
            state
                .update_replacement_cancelled
                .store(true, Ordering::Relaxed);
        }
        self.preview_update_state = None;
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
            Screen::SearchFields(_) => "SearchFields",
            Screen::PerformingReplacement(_) => "PerformingReplacement",
            Screen::Results(_) => "Results",
        }
    }

    fn unwrap_search_fields_state_mut(&mut self) -> &mut SearchFieldsState {
        let name = self.name().to_owned();
        let Screen::SearchFields(search_fields_state) = self else {
            panic!("Expected current_screen to be SearchFields, found {name}");
        };
        search_fields_state
    }
}

#[derive(Debug)]
pub enum Popup {
    Error,
    Help,
    Text { title: String, body: String },
}

#[derive(Debug, Clone)]
struct Toast {
    message: String,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppRunConfig {
    pub include_hidden: bool,
    pub include_git_folders: bool,
    pub advanced_regex: bool,
    pub immediate_search: bool,
    pub immediate_replace: bool,
    pub print_results: bool,
    pub print_on_exit: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for AppRunConfig {
    fn default() -> Self {
        Self {
            include_hidden: false,
            include_git_folders: false,
            advanced_regex: false,
            immediate_search: false,
            immediate_replace: false,
            print_results: false,
            print_on_exit: false,
        }
    }
}

#[derive(Debug)]
pub struct EventChannels {
    pub sender: UnboundedSender<Event>,
    receiver: UnboundedReceiver<Event>,
}

impl EventChannels {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self { sender, receiver }
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
}

impl Default for EventChannels {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct UIState {
    pub current_screen: Screen,
    pub popup: Option<Popup>,
    toast: Option<Toast>,
    errors: Vec<AppError>,
}

impl UIState {
    pub fn new(current_screen: Screen) -> Self {
        Self {
            current_screen,
            popup: None,
            toast: None,
            errors: Vec::new(),
        }
    }

    pub fn add_error(&mut self, error: AppError) {
        self.errors.push(error);
    }

    pub fn errors(&self) -> &[AppError] {
        &self.errors
    }

    pub fn clear_errors(&mut self) {
        self.errors.clear();
    }
}

#[derive(Debug)]
pub struct App {
    pub config: Config,
    key_map: KeyMap,
    pub search_fields: SearchFields,
    pub searcher: Option<Searcher>,
    pub input_source: InputSource,
    pub run_config: AppRunConfig,
    pub event_channels: EventChannels,
    pub ui_state: UIState,
}

#[derive(Debug)]
enum SearchStrategy {
    Files(FileSearcher),
    Text {
        haystack: Arc<String>,
        config: ParsedSearchConfig,
    },
}

fn generate_escape_deprecation_message(quit_keymap: Option<KeyEvent>) -> String {
    let quit_keymap_str = quit_keymap.map_or("".to_string(), |keymap| {
        let optional_help = if let KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
        } = keymap
        {
            // Add some additional help text when using the default
            " (i.e. `ctrl + c`)"
        } else {
            ""
        };
        format!(": use `{keymap}`{optional_help} instead")
    });

    format!(
        "Pressing escape to quit is no longer enabled by default{quit_keymap_str}.\n\nYou can remap this in your scooter config.",
    )
}

// Macro to get the background processing receiver from current_screen, needed because
// methods can't express split borrows but macros can
macro_rules! get_bg_receiver {
    ($self:expr) => {
        match &mut $self.ui_state.current_screen {
            Screen::SearchFields(SearchFieldsState { search_state, .. }) => {
                search_state.as_mut().map(|s| &mut s.processing_receiver)
            }
            Screen::PerformingReplacement(PerformingReplacementState {
                processing_receiver,
                ..
            }) => Some(processing_receiver),
            Screen::Results(_) => None,
        }
    };
}

macro_rules! recv_optional {
    ($opt_receiver:expr) => {
        async {
            match $opt_receiver {
                Some(r) => r.recv().await,
                None => None,
            }
        }
    };
}

impl<'a> App {
    pub fn new(
        input_source: InputSource,
        search_field_values: &SearchFieldValues<'a>,
        app_run_config: AppRunConfig,
        config: Config,
    ) -> anyhow::Result<Self> {
        let search_fields = SearchFields::with_values(
            search_field_values,
            config.search.disable_prepopulated_fields,
        );

        let mut search_fields_state = SearchFieldsState::default();
        if app_run_config.immediate_search {
            search_fields_state.focussed_section = FocussedSection::SearchResults;
        }

        let key_map = KeyMap::from_config(&config.keys).map_err(display_conflict_errors)?;

        let search_immediately =
            app_run_config.immediate_search || !search_field_values.search.value.is_empty();

        let mut app = Self {
            config,
            key_map,
            search_fields,
            searcher: None,
            input_source,
            run_config: app_run_config,
            event_channels: EventChannels::new(),
            ui_state: UIState::new(Screen::SearchFields(search_fields_state)),
        };

        if search_immediately {
            app.perform_search_background();
        }

        Ok(app)
    }

    pub fn handle_internal_event(&mut self, event: InternalEvent) -> EventHandlingResult {
        match event {
            InternalEvent::App(app_event) => self.handle_app_event(app_event),
            InternalEvent::Background(bg_event) => {
                self.handle_background_processing_event(bg_event)
            }
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_app_event(&mut self, app_event: AppEvent) -> EventHandlingResult {
        match app_event {
            AppEvent::PerformSearch => {
                self.perform_search_already_validated();
                EventHandlingResult::Rerender
            }
            AppEvent::DismissToast { generation } => {
                self.dismiss_toast_if_generation_matches(generation);
                EventHandlingResult::Rerender
            }
        }
    }

    fn cancel_search(&mut self) {
        if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(SearchState { cancelled, .. }),
            ..
        }) = &mut self.ui_state.current_screen
        {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    fn cancel_replacement(&mut self) {
        if let Screen::PerformingReplacement(PerformingReplacementState { cancelled, .. }) =
            &mut self.ui_state.current_screen
        {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_in_progress_tasks(&mut self) {
        self.cancel_search();
        self.cancel_replacement();

        if let Screen::SearchFields(ref mut search_fields_state) = self.ui_state.current_screen {
            search_fields_state.cancel_preview_updates();
        }
    }

    pub fn reset(&mut self) {
        self.cancel_in_progress_tasks();
        let mut run_config = self.run_config.clone();
        run_config.immediate_search = false;

        *self = Self::new(
            self.input_source.clone(), // TODO: avoid cloning
            &SearchFieldValues::default(),
            run_config,
            std::mem::take(&mut self.config),
        )
        .expect("App initialisation errors should have been detected on initial construction");
    }

    pub async fn event_recv(&mut self) -> Event {
        tokio::select! {
            Some(event) = self.event_channels.recv() => event,
            Some(bg_event) = recv_optional!(get_bg_receiver!(self)) => {
                Event::Internal(InternalEvent::Background(bg_event))
            }
        }
    }

    pub fn background_processing_reciever(
        &mut self,
    ) -> Option<&mut UnboundedReceiver<BackgroundProcessingEvent>> {
        get_bg_receiver!(self)
    }

    /// Called when searching explicitly: shows error popup if there have been validation failures
    //
    /// NOTE: validation should have been performed (with `validate_fields`) before calling
    // TODO: how can we enforce validation by type system?
    fn perform_search_foreground(&mut self) {
        if !matches!(self.ui_state.current_screen, Screen::SearchFields(_)) {
            log::warn!(
                "Called perform_search_with_error_popup on screen {}",
                self.ui_state.current_screen.name()
            );
            return;
        }

        if !self.errors().is_empty() {
            self.set_popup(Popup::Error);
        } else if self.search_fields.search().text().is_empty() {
            self.add_error(AppError {
                name: "Search field must not be empty".to_string(),
                long: "Please enter some search text".to_string(),
            });
        } else {
            let Screen::SearchFields(ref mut search_fields_state) = self.ui_state.current_screen
            else {
                panic!(
                    "Expected SearchFields, found {:?}",
                    self.ui_state.current_screen.name()
                );
            };
            search_fields_state.focussed_section = FocussedSection::SearchResults;
            // Check if search has been performed
            if search_fields_state.search_state.is_some() {
                if self.run_config.immediate_replace && self.search_has_completed() {
                    self.perform_replacement();
                }
            } else {
                self.perform_search_background();
            }
        }
    }

    /// Called when searching in the background e.g. when entering chars into the search field: does not show
    /// error popup if there are validation errors
    pub fn perform_search_background(&mut self) {
        if !matches!(self.ui_state.current_screen, Screen::SearchFields(_)) {
            log::warn!(
                "Called perform_search_if_valid on screen {}",
                self.ui_state.current_screen.name()
            );
            return;
        }

        let Some(search_config) = self.validate_fields().unwrap() else {
            return;
        };
        self.searcher = Some(search_config);
        self.perform_search_already_validated();
    }

    /// NOTE: validation should have been performed (with `validate_fields`) before calling
    // TODO: how can we enforce validation by type system - e.g. pass in searcher?
    fn perform_search_already_validated(&mut self) {
        self.cancel_search();
        let Screen::SearchFields(ref mut search_fields_state) = self.ui_state.current_screen else {
            log::warn!(
                "Called perform_search_unwrap on screen {}",
                self.ui_state.current_screen.name()
            );
            return;
        };
        search_fields_state.cancel_preview_updates();
        if let Some(timer) = search_fields_state.search_debounce_timer.take() {
            timer.abort();
        }

        if self.search_fields.search().text().is_empty() {
            search_fields_state.search_state = None;
        }

        let (background_processing_sender, background_processing_receiver) =
            mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let search_state = SearchState::new(
            background_processing_sender.clone(),
            background_processing_receiver,
            cancelled.clone(),
        );

        let strategy = match &self.searcher {
            Some(Searcher::FileSearcher(file_searcher)) => {
                SearchStrategy::Files(file_searcher.clone())
            }
            Some(Searcher::TextSearcher { search_config }) => {
                let InputSource::Stdin(ref stdin) = self.input_source else {
                    panic!("Expected InputSource::Stdin, found {:?}", self.input_source);
                };
                SearchStrategy::Text {
                    haystack: Arc::clone(stdin),
                    config: search_config.clone(),
                }
            }
            None => {
                panic!("Fields should have been parsed")
            }
        };

        Self::spawn_search_task(
            strategy,
            background_processing_sender.clone(),
            self.event_channels.sender.clone(),
            cancelled,
        );

        search_fields_state.search_state = Some(search_state);
    }

    #[allow(clippy::needless_pass_by_value)]
    fn update_all_replacements(&mut self, cancelled: Arc<AtomicBool>) -> EventHandlingResult {
        if cancelled.load(Ordering::Relaxed) {
            return EventHandlingResult::None;
        }
        let Screen::SearchFields(SearchFieldsState {
            search_state: Some(search_state),
            preview_update_state: Some(preview_update_state),
            ..
        }) = &mut self.ui_state.current_screen
        else {
            return EventHandlingResult::None;
        };

        preview_update_state.total_replacements_to_update = search_state.results.len();

        #[allow(clippy::items_after_statements)]
        static STEP: usize = 7919; // Slightly random so that increments seem more natural in UI

        let num_results = search_state.results.len();
        for start in (0..num_results).step_by(STEP) {
            let end = (start + STEP - 1).min(num_results.saturating_sub(1));
            let _ = search_state.processing_sender.send(
                BackgroundProcessingEvent::UpdateReplacements {
                    start,
                    end,
                    cancelled: cancelled.clone(),
                },
            );
        }

        EventHandlingResult::Rerender
    }

    #[allow(clippy::needless_pass_by_value)]
    fn update_replacements(
        &mut self,
        start: usize,
        end: usize,
        cancelled: Arc<AtomicBool>,
    ) -> EventHandlingResult {
        if cancelled.load(Ordering::Relaxed) {
            return EventHandlingResult::None;
        }
        let Screen::SearchFields(SearchFieldsState {
            search_state: Some(search_state),
            preview_update_state: Some(preview_update_state),
            ..
        }) = &mut self.ui_state.current_screen
        else {
            return EventHandlingResult::None;
        };
        let file_searcher = self
            .searcher
            .as_ref()
            .expect("Fields should have been parsed");
        for res in &mut search_state.results[start..=end] {
            match replacement_if_match(
                &res.search_result.line,
                file_searcher.search(),
                file_searcher.replace(),
            ) {
                Some(replacement) => res.replacement = replacement,
                None => return EventHandlingResult::Rerender, // TODO: can we handle this better?
            }
        }
        preview_update_state.replacements_updated += end - start + 1;

        EventHandlingResult::Rerender
    }

    pub fn perform_replacement(&mut self) {
        if !self.ready_to_replace() {
            return;
        }

        let temp_placeholder = Screen::SearchFields(SearchFieldsState::default());
        match mem::replace(
            &mut self.ui_state.current_screen,
            temp_placeholder, // Will get reset if we are not on `SearchComplete` screen
        ) {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => {
                let (background_processing_sender, background_processing_receiver) =
                    mpsc::unbounded_channel();
                let cancelled = Arc::new(AtomicBool::new(false));
                let total_replacements = state
                    .results
                    .iter()
                    .filter(|r| r.search_result.included)
                    .count();
                let replacements_completed = Arc::new(AtomicUsize::new(0));

                let Some(searcher) = self.validate_fields().unwrap() else {
                    panic!("Attempted to replace with invalid fields");
                };
                match searcher {
                    Searcher::FileSearcher(file_searcher) => {
                        replace::perform_replacement(
                            state.results,
                            background_processing_sender.clone(),
                            cancelled.clone(),
                            replacements_completed.clone(),
                            self.event_channels.sender.clone(),
                            Some(file_searcher),
                        );
                    }
                    Searcher::TextSearcher { search_config } => {
                        let InputSource::Stdin(ref stdin) = self.input_source else {
                            panic!("Expected stdin input source, found {:?}", self.input_source)
                        };
                        self.event_channels
                            .sender
                            .send(Event::ExitAndReplace(ExitAndReplaceState {
                                stdin: Arc::clone(stdin),
                                replace_results: state.results,
                                search_config,
                            }))
                            .expect("Failed to send ExitAndReplace event");
                    }
                }

                self.ui_state.current_screen =
                    Screen::PerformingReplacement(PerformingReplacementState::new(
                        background_processing_receiver,
                        cancelled,
                        replacements_completed,
                        total_replacements,
                    ));
            }
            screen => self.ui_state.current_screen = screen,
        }
    }

    fn ready_to_replace(&mut self) -> bool {
        if !self.search_has_completed() {
            self.add_error(AppError {
                name: "Search still in progress".to_string(),
                long: "Try again when search is complete".to_string(),
            });
            return false;
        } else if !self.is_preview_updated() {
            self.add_error(AppError {
                name: "Updating replacement preview".to_string(),
                long: "Try again when complete".to_string(),
            });
            return false;
        } else if !self
            .background_processing_reciever()
            .is_some_and(|r| r.is_empty())
        {
            self.add_error(AppError {
                name: "Background processing in progress".to_string(),
                long: "Try again in a moment".to_string(),
            });
            return false;
        }
        true
    }

    pub fn handle_background_processing_event(
        &mut self,
        event: BackgroundProcessingEvent,
    ) -> EventHandlingResult {
        match event {
            BackgroundProcessingEvent::AddSearchResult(result) => {
                self.add_search_results(iter::once(result))
            }
            BackgroundProcessingEvent::AddSearchResults(results) => {
                self.add_search_results(results)
            }
            BackgroundProcessingEvent::SearchCompleted => {
                if let Screen::SearchFields(SearchFieldsState {
                    search_state: Some(state),
                    focussed_section,
                    ..
                }) = &mut self.ui_state.current_screen
                {
                    state.set_search_completed_now();
                    if self.run_config.immediate_replace
                        && *focussed_section == FocussedSection::SearchResults
                    {
                        self.perform_replacement();
                    }
                }
                EventHandlingResult::Rerender
            }
            BackgroundProcessingEvent::ReplacementCompleted(replace_state) => {
                if self.run_config.print_results {
                    EventHandlingResult::new_exit_stats(replace_state)
                } else {
                    self.ui_state.current_screen = Screen::Results(replace_state);
                    EventHandlingResult::Rerender
                }
            }
            BackgroundProcessingEvent::UpdateAllReplacements { cancelled } => {
                self.update_all_replacements(cancelled)
            }
            BackgroundProcessingEvent::UpdateReplacements {
                start,
                end,
                cancelled,
            } => self.update_replacements(start, end, cancelled),
        }
    }

    fn add_search_results<I>(&mut self, results: I) -> EventHandlingResult
    where
        I: IntoIterator<Item = SearchResult>,
    {
        let mut rerender = false;
        if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(search_in_progress_state),
            ..
        }) = &mut self.ui_state.current_screen
        {
            let mut results_with_replacements = Vec::new();
            let searcher = self
                .searcher
                .as_ref()
                .expect("searcher should not be None when adding search results");
            for res in results {
                let updated = add_replacement(res, searcher.search(), searcher.replace());
                if let Some(updated) = updated {
                    results_with_replacements.push(updated);
                }
            }
            search_in_progress_state
                .results
                .append(&mut results_with_replacements);

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

    /// Should only be called on `Screen::SearchFields`, and when focussed section is `FocussedSection::SearchFields`
    #[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
    fn handle_command_search_fields(
        &mut self,
        event: CommandSearchFocusFields,
    ) -> EventHandlingResult {
        match event {
            CommandSearchFocusFields::UnlockPrepopulatedFields => {
                self.unlock_prepopulated_fields();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusFields::TriggerSearch => {
                self.perform_search_foreground();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusFields::FocusPreviousField => {
                self.search_fields
                    .focus_prev(self.config.search.disable_prepopulated_fields);
                EventHandlingResult::Rerender
            }
            CommandSearchFocusFields::FocusNextField => {
                self.search_fields
                    .focus_next(self.config.search.disable_prepopulated_fields);
                EventHandlingResult::Rerender
            }
            CommandSearchFocusFields::EnterChars(key_code, key_modifiers) => {
                self.enter_chars_into_field(key_code, key_modifiers)
            }
        }
    }

    fn enter_chars_into_field(
        &mut self,
        key_code: KeyCode,
        key_modifiers: KeyModifiers,
    ) -> EventHandlingResult {
        let Screen::SearchFields(ref mut search_fields_state) = self.ui_state.current_screen else {
            return EventHandlingResult::None;
        };
        if let FieldName::FixedStrings = self.search_fields.highlighted_field().name {
            // TODO: ideally this should only happen when the field is checked, but for now this will do
            self.search_fields.search_mut().clear_error();
        }

        search_fields_state.cancel_preview_updates();

        self.search_fields.highlighted_field_mut().handle_keys(
            key_code,
            key_modifiers,
            self.config.search.disable_prepopulated_fields,
        );
        if let Some(search_config) = self.validate_fields().unwrap() {
            self.searcher = Some(search_config);
        } else {
            return EventHandlingResult::Rerender;
        }
        let Screen::SearchFields(ref mut search_fields_state) = self.ui_state.current_screen else {
            return EventHandlingResult::None;
        };
        let file_searcher = self
            .searcher
            .as_ref()
            .expect("Fields should have been parsed");

        if let FieldName::Replace = self.search_fields.highlighted_field().name {
            if let Some(ref mut state) = search_fields_state.search_state {
                // Immediately update replacement on selected fields - the remainder will be updated async
                if let Some(highlighted) = state.primary_selected_field_mut()
                    && let Some(updated) = replacement_if_match(
                        &highlighted.search_result.line,
                        file_searcher.search(),
                        file_searcher.replace(),
                    )
                {
                    highlighted.replacement = updated;
                }

                // Debounce replacement requests
                let sender = state.processing_sender.clone();
                let cancelled = Arc::new(AtomicBool::new(false));
                let cancelled_clone = cancelled.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = sender.send(BackgroundProcessingEvent::UpdateAllReplacements {
                        cancelled: cancelled_clone,
                    });
                });
                // Note that cancel_preview_updates is called above, which cancels any existing preview updates
                search_fields_state.preview_update_state =
                    Some(PreviewUpdateStatus::new(handle, cancelled));
            }
        } else {
            // Debounce search requests
            if let Some(timer) = search_fields_state.search_debounce_timer.take() {
                timer.abort();
            }
            let event_sender = self.event_channels.sender.clone();
            search_fields_state.search_debounce_timer = Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ =
                    event_sender.send(Event::Internal(InternalEvent::App(AppEvent::PerformSearch)));
            }));
        }
        EventHandlingResult::Rerender
    }

    fn get_search_state_unwrap(&mut self) -> &mut SearchState {
        self.ui_state
            .current_screen
            .unwrap_search_fields_state_mut()
            .search_state
            .as_mut()
            .expect("Focussed on search results but search_state is None")
    }

    /// Should only be called on `Screen::SearchFields`, and when focussed section is `FocussedSection::SearchResults`
    #[allow(clippy::needless_pass_by_value)]
    fn handle_command_search_results(
        &mut self,
        event: CommandSearchFocusResults,
    ) -> EventHandlingResult {
        assert!(
            matches!(self.ui_state.current_screen, Screen::SearchFields(_)),
            "Expected current_screen to be SearchFields, found {}",
            self.ui_state.current_screen.name()
        );

        match event {
            CommandSearchFocusResults::TriggerReplacement => {
                self.perform_replacement();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::BackToFields => {
                self.cancel_search();
                let search_fields_state = self
                    .ui_state
                    .current_screen
                    .unwrap_search_fields_state_mut();
                search_fields_state.focussed_section = FocussedSection::SearchFields;
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::OpenInEditor => {
                let search_fields_state = self
                    .ui_state
                    .current_screen
                    .unwrap_search_fields_state_mut();
                if let Some(ref mut search_in_progress_state) = search_fields_state.search_state {
                    let selected = search_in_progress_state
                        .primary_selected_field_mut()
                        .expect("Expected to find selected field");
                    if let Some(ref path) = selected.search_result.path {
                        self.event_channels
                            .sender
                            .send(Event::LaunchEditor((
                                path.clone(),
                                selected.search_result.line_number,
                            )))
                            .expect("Failed to send event");
                    }
                }
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveDown => {
                self.get_search_state_unwrap().move_selected_down();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveUp => {
                self.get_search_state_unwrap().move_selected_up();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveDownHalfPage => {
                self.get_search_state_unwrap()
                    .move_selected_down_half_page();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveDownFullPage => {
                self.get_search_state_unwrap()
                    .move_selected_down_full_page();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveUpHalfPage => {
                self.get_search_state_unwrap().move_selected_up_half_page();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveUpFullPage => {
                self.get_search_state_unwrap().move_selected_up_full_page();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveTop => {
                self.get_search_state_unwrap().move_selected_top();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::MoveBottom => {
                self.get_search_state_unwrap().move_selected_bottom();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::ToggleSelectedInclusion => {
                self.get_search_state_unwrap().toggle_selected_inclusion();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::ToggleAllSelected => {
                self.get_search_state_unwrap().toggle_all_selected();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::ToggleMultiselectMode => {
                self.get_search_state_unwrap().toggle_multiselect_mode();
                EventHandlingResult::Rerender
            }
            CommandSearchFocusResults::FlipMultiselectDirection => {
                self.get_search_state_unwrap().flip_multiselect_direction();
                EventHandlingResult::Rerender
            }
        }
    }

    pub fn handle_key_event(&mut self, key_event: KeyEvent) -> EventHandlingResult {
        let command = match self.handle_special_cases(key_event) {
            Left(command) => command,
            Right(event_handling_result) => return event_handling_result,
        };

        // Note that general commands are looked up after screen-specific commands in `.lookup`, so this if will only be hit
        // if there are no screen-specific commands
        if let Command::General(command) = command {
            match command {
                CommandGeneral::Quit => {
                    self.reset();
                    return EventHandlingResult::Exit(None);
                }
                CommandGeneral::Reset => {
                    self.reset();
                    return EventHandlingResult::Rerender;
                }
                CommandGeneral::ShowHelpMenu => {
                    self.set_popup(Popup::Help);
                    return EventHandlingResult::Rerender;
                }
            }
        }

        match &mut self.ui_state.current_screen {
            Screen::SearchFields(search_fields_state) => {
                let Command::SearchFields(command) = command else {
                    panic!("Expected SearchFields command, found {command:?}");
                };

                match command {
                    CommandSearchFields::TogglePreviewWrapping => {
                        self.config.preview.wrap_text = !self.config.preview.wrap_text;
                        self.show_toggle_toast("Text wrapping", self.config.preview.wrap_text);
                        EventHandlingResult::Rerender
                    }
                    CommandSearchFields::ToggleHiddenFiles => {
                        if matches!(self.input_source, InputSource::Stdin(_)) {
                            return EventHandlingResult::None;
                        }
                        self.run_config.include_hidden = !self.run_config.include_hidden;
                        self.show_toggle_toast("Hidden files", self.run_config.include_hidden);
                        self.perform_search_background();
                        EventHandlingResult::Rerender
                    }
                    CommandSearchFields::SearchFocusFields(command) => {
                        if !matches!(
                            search_fields_state.focussed_section,
                            FocussedSection::SearchFields
                        ) {
                            panic!(
                                "Expected FocussedSection::SearchFields, found {:?}",
                                search_fields_state.focussed_section
                            );
                        }
                        self.handle_command_search_fields(command)
                    }
                    CommandSearchFields::SearchFocusResults(command) => {
                        if !matches!(
                            search_fields_state.focussed_section,
                            FocussedSection::SearchResults
                        ) {
                            panic!(
                                "Expected FocussedSection::SearchResults, found {:?}",
                                search_fields_state.focussed_section
                            );
                        }
                        self.handle_command_search_results(command)
                    }
                }
            }
            Screen::PerformingReplacement(_) => EventHandlingResult::None,
            Screen::Results(replace_state) => {
                let Command::Results(command) = command else {
                    panic!("Expected SearchFields event, found {command:?}");
                };
                replace_state.handle_command_results(command)
            }
        }
    }

    fn handle_special_cases(
        &mut self,
        key_event: KeyEvent,
    ) -> Either<Command, EventHandlingResult> {
        let maybe_event = self
            .key_map
            .lookup(&self.ui_state.current_screen, key_event);

        // Quit should take precedent over closing popup etc.
        if !matches!(maybe_event, Some(Command::General(CommandGeneral::Quit))) {
            if self.ui_state.popup.is_some() {
                self.clear_popup();
                return Right(EventHandlingResult::Rerender);
            }
            if key_event.code == KeyCode::Esc && self.multiselect_enabled() {
                self.toggle_multiselect_mode();
                return Right(EventHandlingResult::Rerender);
            }
        }

        let event = if let Some(event) = maybe_event {
            event
        } else {
            if key_event.code == KeyCode::Esc {
                let quit_keymap = self.config.keys.general.quit.first().copied();
                self.set_popup(Popup::Text {
                    title: "Key mapping deprecated".to_string(),
                    body: generate_escape_deprecation_message(quit_keymap),
                });
                return Right(EventHandlingResult::Rerender);
            }

            // If we're in SearchFields focus, treat unmatched keys as text input
            if let Screen::SearchFields(state) = &self.ui_state.current_screen {
                if state.focussed_section == FocussedSection::SearchFields {
                    Command::SearchFields(CommandSearchFields::SearchFocusFields(
                        CommandSearchFocusFields::EnterChars(key_event.code, key_event.modifiers),
                    ))
                } else {
                    return Right(EventHandlingResult::None);
                }
            } else {
                return Right(EventHandlingResult::None);
            }
        };
        Left(event)
    }

    pub fn validate_fields(&mut self) -> anyhow::Result<Option<Searcher>> {
        let search_config = SearchConfig {
            search_text: self.search_fields.search().text(),
            replacement_text: self.search_fields.replace().text(),
            fixed_strings: self.search_fields.fixed_strings().checked,
            advanced_regex: self.run_config.advanced_regex,
            match_whole_word: self.search_fields.whole_word().checked,
            match_case: self.search_fields.match_case().checked,
        };
        let dir_config = match &self.input_source {
            InputSource::Directory(directory) => Some(DirConfig {
                include_globs: Some(self.search_fields.include_files().text()),
                exclude_globs: Some(self.search_fields.exclude_files().text()),
                include_hidden: self.run_config.include_hidden,
                include_git_folders: self.run_config.include_git_folders,
                directory: directory.clone(),
            }),
            InputSource::Stdin(_) => None,
        };

        let mut error_handler = AppErrorHandler::new();
        let result = validate_search_configuration(search_config, dir_config, &mut error_handler)?;
        error_handler.apply_to_app(self);

        let maybe_searcher = match result {
            ValidationResult::Success((search_config, dir_config)) => match &self.input_source {
                InputSource::Directory(_) => {
                    let file_searcher = FileSearcher::new(
                        search_config,
                        dir_config.expect("Found None dir_config when searching through files"),
                    );
                    Some(Searcher::FileSearcher(file_searcher))
                }
                InputSource::Stdin(_) => Some(Searcher::TextSearcher { search_config }),
            },
            ValidationResult::ValidationErrors => None,
        };
        Ok(maybe_searcher)
    }

    fn spawn_search_task(
        strategy: SearchStrategy,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        event_sender: UnboundedSender<Event>,
        cancelled: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let sender_for_search = background_processing_sender.clone();
            let mut search_handle = task::spawn_blocking(move || {
                match strategy {
                    SearchStrategy::Files(file_searcher) => {
                        file_searcher.walk_files(Some(&cancelled), || {
                            let sender = sender_for_search.clone();
                            Box::new(move |results| {
                                // Ignore error - likely state reset, thread about to be killed
                                let _ = sender
                                    .send(BackgroundProcessingEvent::AddSearchResults(results));
                                WalkState::Continue
                            })
                        });
                    }
                    SearchStrategy::Text { haystack, config } => {
                        let cursor = Cursor::new(haystack.as_bytes());
                        for (idx, line_result) in cursor.lines_with_endings().enumerate() {
                            if cancelled.load(Ordering::Relaxed) {
                                break;
                            }

                            let (line_ending, line) = match read_line(line_result) {
                                Ok(res) => res,
                                Err(e) => {
                                    debug!("Error when reading line {idx}: {e}");
                                    continue;
                                }
                            };
                            if replacement_if_match(&line, &config.search, &config.replace)
                                .is_some()
                            {
                                let result = SearchResult {
                                    path: None,
                                    line_number: idx + 1,
                                    line,
                                    line_ending,
                                    included: true,
                                };
                                // Ignore error - likely state reset, thread about to be killed
                                let _ = sender_for_search
                                    .send(BackgroundProcessingEvent::AddSearchResult(result));
                            }
                        }
                    }
                }
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
                        let _ = event_sender.send(Event::Rerender);
                    }
                }
            }

            if let Err(err) =
                background_processing_sender.send(BackgroundProcessingEvent::SearchCompleted)
            {
                // Log and ignore error: likely have gone back to previous screen
                warn!("Found error when attempting to send SearchCompleted event: {err}");
            }
        })
    }

    pub fn show_popup(&self) -> bool {
        self.ui_state.popup.is_some()
    }

    pub fn popup(&self) -> Option<&Popup> {
        self.ui_state.popup.as_ref()
    }

    pub fn errors(&self) -> Vec<AppError> {
        let app_errors = self.ui_state.errors().iter().cloned();
        let field_errors = self.search_fields.errors().into_iter();
        app_errors.chain(field_errors).collect()
    }

    pub fn add_error(&mut self, error: AppError) {
        self.ui_state.popup = Some(Popup::Error);
        self.ui_state.add_error(error);
    }

    fn clear_popup(&mut self) {
        self.ui_state.popup = None;
        self.ui_state.clear_errors();
    }

    fn set_popup(&mut self, popup: Popup) {
        self.ui_state.popup = Some(popup);
    }

    pub fn toast_message(&self) -> Option<&str> {
        self.ui_state.toast.as_ref().map(|t| t.message.as_str())
    }

    fn show_toast(&mut self, message: String) {
        let generation = self.ui_state.toast.as_ref().map_or(1, |t| t.generation + 1);
        self.ui_state.toast = Some(Toast {
            message,
            generation,
        });

        let toast_timeout_ms = 1500;

        let event_sender = self.event_channels.sender.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(toast_timeout_ms)).await;
            let _ = event_sender.send(Event::Internal(InternalEvent::App(
                AppEvent::DismissToast { generation },
            )));
        });
    }

    fn show_toggle_toast(&mut self, name: &str, enabled: bool) {
        let status = if enabled { "ON" } else { "OFF" };
        self.show_toast(format!("{name}: {status}"));
    }

    fn dismiss_toast_if_generation_matches(&mut self, generation: u64) {
        if let Some(toast) = &self.ui_state.toast
            && toast.generation == generation
        {
            self.ui_state.toast = None;
        }
    }

    pub fn keymaps_all(&self) -> Vec<(String, String)> {
        self.keymaps_impl(false)
    }

    pub fn keymaps_compact(&self) -> Vec<(String, String)> {
        self.keymaps_impl(true)
    }

    #[allow(clippy::too_many_lines)]
    fn keymaps_impl(&self, compact: bool) -> Vec<(String, String)> {
        enum Show {
            Both,
            FullOnly,
            #[allow(dead_code)]
            CompactOnly,
        }

        macro_rules! keymap {
            ($($path:tt).+, $name:expr, $show:expr $(,)?) => {
                (
                    format!("<{}>", self.config.keys.$($path).+.first()
                        .map_or_else(|| "n/a".to_string(), std::string::ToString::to_string)),
                    $name,
                    $show,
                )
            };
        }

        let current_screen_keys = match &self.ui_state.current_screen {
            Screen::SearchFields(search_fields_state) => {
                let mut keys = vec![];
                match search_fields_state.focussed_section {
                    FocussedSection::SearchFields => {
                        keys.extend([
                            keymap!(search.fields.trigger_search, "jump to results", Show::Both),
                            keymap!(search.fields.focus_next_field, "focus next", Show::Both),
                            keymap!(
                                search.fields.focus_previous_field,
                                "focus previous",
                                Show::FullOnly,
                            ),
                            ("<space>".to_string(), "toggle checkbox", Show::FullOnly), // TODO(key-remap): add to config?
                        ]);
                        if self.config.search.disable_prepopulated_fields {
                            keys.push(keymap!(
                                search.fields.unlock_prepopulated_fields,
                                "unlock pre-populated fields",
                                if self.search_fields.fields.iter().any(|f| f.set_by_cli) {
                                    Show::Both
                                } else {
                                    Show::FullOnly
                                },
                            ));
                        }
                    }
                    FocussedSection::SearchResults => {
                        keys.extend([
                            keymap!(
                                search.results.toggle_selected_inclusion,
                                "toggle",
                                Show::Both,
                            ),
                            keymap!(
                                search.results.toggle_all_selected,
                                "toggle all",
                                Show::FullOnly,
                            ),
                            keymap!(
                                search.results.toggle_multiselect_mode,
                                "toggle multi-select mode",
                                Show::FullOnly,
                            ),
                            keymap!(
                                search.results.flip_multiselect_direction,
                                "flip multi-select direction",
                                Show::FullOnly,
                            ),
                            keymap!(
                                search.results.open_in_editor,
                                "open in editor",
                                Show::FullOnly,
                            ),
                            keymap!(
                                search.results.back_to_fields,
                                "back to search fields",
                                Show::Both,
                            ),
                            keymap!(search.results.move_down, "down", Show::FullOnly),
                            keymap!(search.results.move_up, "up", Show::FullOnly),
                            keymap!(
                                search.results.move_up_half_page,
                                "up half a page",
                                Show::FullOnly
                            ),
                            keymap!(
                                search.results.move_down_half_page,
                                "down half a page",
                                Show::FullOnly
                            ),
                            keymap!(
                                search.results.move_up_full_page,
                                "up a full page",
                                Show::FullOnly
                            ),
                            keymap!(
                                search.results.move_down_full_page,
                                "down a full page",
                                Show::FullOnly
                            ),
                            keymap!(search.results.move_top, "jump to top", Show::FullOnly),
                            keymap!(search.results.move_bottom, "jump to bottom", Show::FullOnly),
                        ]);
                        if self.search_has_completed() {
                            keys.push(keymap!(
                                search.results.trigger_replacement,
                                "replace selected",
                                Show::Both,
                            ));
                        }
                    }
                }
                keys.push(keymap!(
                    search.toggle_preview_wrapping,
                    "toggle text wrapping in preview",
                    Show::FullOnly,
                ));
                if matches!(self.input_source, InputSource::Directory(_)) {
                    keys.push(keymap!(
                        search.toggle_hidden_files,
                        "toggle hidden files",
                        Show::FullOnly,
                    ));
                }
                keys
            }
            Screen::PerformingReplacement(_) => vec![],
            Screen::Results(replace_state) => {
                if !replace_state.errors.is_empty() {
                    vec![
                        keymap!(results.scroll_errors_down, "down", Show::Both),
                        keymap!(results.scroll_errors_up, "up", Show::Both),
                    ]
                } else {
                    vec![]
                }
            }
        };

        let on_search_results = if let Screen::SearchFields(ref s) = self.ui_state.current_screen {
            s.focussed_section == FocussedSection::SearchResults
        } else {
            false
        };

        let esc_help = format!(
            "close popup{}",
            if on_search_results {
                " / exit multi-select"
            } else {
                ""
            }
        );

        let additional_keys = vec![
            keymap!(
                general.reset,
                "reset",
                if on_search_results {
                    Show::FullOnly
                } else {
                    Show::Both
                },
            ),
            keymap!(general.show_help_menu, "help", Show::Both),
            ("<esc>".to_string(), esc_help.as_str(), Show::FullOnly),
            keymap!(general.quit, "quit", Show::Both),
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
        match &self.ui_state.current_screen {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => state.multiselect_enabled(),
            _ => false,
        }
    }

    fn toggle_multiselect_mode(&mut self) {
        match &mut self.ui_state.current_screen {
            Screen::SearchFields(SearchFieldsState {
                search_state: Some(state),
                ..
            }) => state.toggle_multiselect_mode(),
            _ => panic!(
                "Tried to disable multi-select on {:?}",
                self.ui_state.current_screen.name()
            ),
        }
    }

    fn unlock_prepopulated_fields(&mut self) {
        for field in &mut self.search_fields.fields {
            field.set_by_cli = false;
        }
    }

    pub fn search_has_completed(&self) -> bool {
        if let Screen::SearchFields(SearchFieldsState {
            search_state: Some(state),
            search_debounce_timer,
            ..
        }) = &self.ui_state.current_screen
        {
            state.search_completed.is_some()
                && search_debounce_timer
                    .as_ref()
                    .is_none_or(tokio::task::JoinHandle::is_finished)
        } else {
            false
        }
    }

    pub fn is_preview_updated(&self) -> bool {
        if let Screen::SearchFields(SearchFieldsState {
            search_state:
                Some(SearchState {
                    processing_receiver,
                    ..
                }),
            preview_update_state,
            ..
        }) = &self.ui_state.current_screen
        {
            processing_receiver.is_empty()
                && preview_update_state
                    .as_ref()
                    .is_none_or(|p| p.replace_debounce_timer.is_finished())
        } else {
            false
        }
    }
}

fn read_line(
    line_result: Result<(Vec<u8>, LineEnding), std::io::Error>,
) -> anyhow::Result<(LineEnding, String)> {
    let (line_bytes, line_ending) = line_result?;
    let line = String::from_utf8(line_bytes)?;
    Ok((line_ending, line))
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
    use crate::{
        line_reader::LineEnding,
        replace::{ReplaceResult, ReplaceStats},
        search::{SearchResult, SearchResultWithReplacement},
    };
    use rand::RngExt;

    use super::*;

    fn random_num() -> usize {
        let mut rng = rand::rng();
        rng.random_range(1..10000)
    }

    fn search_result_with_replacement(included: bool) -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("random/file")),
                line_number: random_num(),
                line: "foo".to_owned(),
                line_ending: LineEnding::Lf,
                included,
            },
            replacement: "bar".to_owned(),
            replace_result: None,
        }
    }

    fn build_test_results(num_results: usize) -> Vec<SearchResultWithReplacement> {
        (0..num_results)
            .map(|i| SearchResultWithReplacement {
                search_result: SearchResult {
                    path: Some(PathBuf::from(format!("test{i}.txt"))),
                    line_number: 1,
                    line: format!("test line {i}").to_string(),
                    line_ending: LineEnding::Lf,
                    included: true,
                },
                replacement: format!("replacement {i}").to_string(),
                replace_result: None,
            })
            .collect()
    }

    fn build_test_search_state(num_results: usize) -> SearchState {
        let results = build_test_results(num_results);
        build_test_search_state_with_results(results)
    }

    fn build_test_search_state_with_results(
        results: Vec<SearchResultWithReplacement>,
    ) -> SearchState {
        let (processing_sender, processing_receiver) = mpsc::unbounded_channel();
        SearchState {
            results,
            selected: Selected::Single(0),
            view_offset: 0,
            num_displayed: Some(5),
            processing_receiver,
            processing_sender,
            cancelled: Arc::new(AtomicBool::new(false)),
            last_render: Instant::now(),
            search_started: Instant::now(),
            search_completed: None,
        }
    }

    #[test]
    fn test_toggle_all_selected_when_all_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result_with_replacement(true),
            search_result_with_replacement(true),
            search_result_with_replacement(true),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.search_result.included)
                .collect::<Vec<_>>(),
            vec![false, false, false]
        );
    }

    #[test]
    fn test_toggle_all_selected_when_none_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result_with_replacement(false),
            search_result_with_replacement(false),
            search_result_with_replacement(false),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.search_result.included)
                .collect::<Vec<_>>(),
            vec![true, true, true]
        );
    }

    #[test]
    fn test_toggle_all_selected_when_some_selected() {
        let mut search_state = build_test_search_state_with_results(vec![
            search_result_with_replacement(true),
            search_result_with_replacement(false),
            search_result_with_replacement(true),
        ]);
        search_state.toggle_all_selected();
        assert_eq!(
            search_state
                .results
                .iter()
                .map(|res| res.search_result.included)
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
                .map(|res| res.search_result.included)
                .collect::<Vec<_>>(),
            vec![] as Vec<bool>
        );
    }

    fn success_result() -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("random/file")),
                line_number: random_num(),
                line: "foo".to_owned(),
                line_ending: LineEnding::Lf,
                included: true,
            },
            replacement: "bar".to_owned(),
            replace_result: Some(ReplaceResult::Success),
        }
    }

    fn ignored_result() -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("random/file")),
                line_number: random_num(),
                line: "foo".to_owned(),
                line_ending: LineEnding::Lf,
                included: false,
            },
            replacement: "bar".to_owned(),
            replace_result: None,
        }
    }

    fn error_result() -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("random/file")),
                line_number: random_num(),
                line: "foo".to_owned(),
                line_ending: LineEnding::Lf,
                included: true,
            },
            replacement: "bar".to_owned(),
            replace_result: Some(ReplaceResult::Error("error".to_owned())),
        }
    }

    #[tokio::test]
    async fn test_calculate_statistics_all_success() {
        let search_results_with_replacements =
            vec![success_result(), success_result(), success_result()];

        let (results, _num_ignored) =
            crate::replace::split_results(search_results_with_replacements);
        let stats = crate::replace::calculate_statistics(results);

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
        let search_results_with_replacements = vec![
            success_result(),
            ignored_result(),
            success_result(),
            error_result.clone(),
            ignored_result(),
        ];

        let (results, _num_ignored) =
            crate::replace::split_results(search_results_with_replacements);
        let stats = crate::replace::calculate_statistics(results);

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
            state
                .results
                .iter()
                .map(|r| r.search_result.included)
                .collect::<Vec<_>>()
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
                .map(|res| res.search_result.included)
                .collect::<Vec<_>>(),
            vec![true, true, true, true, true, true]
        );
        state.toggle_selected_inclusion();
        assert_eq!(
            state
                .results
                .iter()
                .map(|res| res.search_result.included)
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
                .map(|res| res.search_result.included)
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

    #[test]
    fn test_key_handling_quit_takes_precedent() {
        let mut app = App::new(
            InputSource::Directory(std::env::current_dir().unwrap()),
            &SearchFieldValues::default(),
            AppRunConfig::default(),
            Config::default(),
        )
        .unwrap();
        app.set_popup(Popup::Text {
            title: "Error title".to_owned(),
            body: "some text in the body".to_owned(),
        });
        let res = app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(res, EventHandlingResult::Exit(None)));
    }

    #[test]
    fn test_key_handling_unmapped_key_closes_popup() {
        let mut app = App::new(
            InputSource::Directory(std::env::current_dir().unwrap()),
            &SearchFieldValues::default(),
            AppRunConfig::default(),
            Config::default(),
        )
        .unwrap();
        app.set_popup(Popup::Text {
            title: "Error title".to_owned(),
            body: "some text in the body".to_owned(),
        });
        let res = app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert!(matches!(res, EventHandlingResult::Rerender));
        assert!(app.popup().is_none());
    }

    #[test]
    fn test_escape_deprecation_message_with_default() {
        let keymap = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let message = generate_escape_deprecation_message(Some(keymap));
        assert_eq!(
            message,
            "Pressing escape to quit is no longer enabled by default: use `C-c` \
             (i.e. `ctrl + c`) instead.\n\nYou can remap this in your scooter config."
        );
    }

    #[test]
    fn test_escape_deprecation_message_with_no_mapping() {
        let message = generate_escape_deprecation_message(None);
        assert_eq!(
            message,
            "Pressing escape to quit is no longer enabled by default.\n\n\
             You can remap this in your scooter config."
        );
    }

    #[test]
    fn test_escape_deprecation_message_with_f_key() {
        let keymap = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        let message = generate_escape_deprecation_message(Some(keymap));
        assert_eq!(
            message,
            "Pressing escape to quit is no longer enabled by default: use `F1` instead.\n\n\
             You can remap this in your scooter config."
        );
    }

    #[test]
    fn test_escape_deprecation_message_with_ctrl_alt_q_keymap() {
        let keymap = KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        let message = generate_escape_deprecation_message(Some(keymap));
        assert_eq!(
            message,
            "Pressing escape to quit is no longer enabled by default: use `C-A-q` instead.\n\n\
             You can remap this in your scooter config."
        );
    }
}
