use anyhow::Error;
use crossterm::event::KeyEvent;
use fancy_regex::Regex as FancyRegex;
use futures::future;
use ignore::{
    overrides::{Override, OverrideBuilder},
    WalkState,
};
use log::warn;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use regex::Regex;
use std::{
    cmp::{max, min},
    iter::Iterator,
};
use std::{
    collections::HashMap,
    env::current_dir,
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        Semaphore,
    },
    task::JoinHandle,
};

use crate::{
    config::{load_config, Config},
    fields::{CheckboxField, Field, TextField},
    replace::{ParsedFields, SearchType},
    utils::ceil_div,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceResult {
    Success,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_number: usize,
    /// 1-indexed
    pub line: String,
    pub replacement: String,
    pub included: bool,
    pub replace_result: Option<ReplaceResult>,
}

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
    AddSearchResult(SearchResult),
    SearchCompleted,
    ReplacementCompleted(ReplaceState),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EventHandlingResult {
    Rerender,
    Exit,
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
    // TODO: make the view logic with scrolling etc. into a generic component
    pub results: Vec<SearchResult>,
    selected: Selected,
    pub(crate) view_offset: usize,           // Updated by UI, not app
    pub(crate) num_displayed: Option<usize>, // Updated by UI, not app
    processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
}

impl SearchState {
    pub fn new(processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>) -> Self {
        Self {
            results: vec![],
            selected: Selected::Single(0),
            view_offset: 0,
            num_displayed: None,
            processing_receiver,
        }
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
        let (low, high) = self.selected_range();
        &self.results[low..=high]
    }

    fn selected_fields_mut(&mut self) -> &mut [SearchResult] {
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
}

#[derive(Debug, Eq, PartialEq)]
pub struct ReplaceState {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: Vec<SearchResult>,
    pub replacement_errors_pos: usize,
}

impl ReplaceState {
    fn handle_key_results(&mut self, key: &KeyEvent) -> EventHandlingResult {
        #[allow(clippy::match_same_arms)]
        match (key.code, key.modifiers) {
            (KeyCode::Char('j') | KeyCode::Down, _)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_down();
            }
            (KeyCode::Char('k') | KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_up();
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {} // TODO: scroll down half a page
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {} // TODO: scroll down a full page
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {} // TODO: scroll up half a page
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {} // TODO: scroll up a full page
            (KeyCode::Enter | KeyCode::Char('q'), _) => return EventHandlingResult::Exit,
            _ => return EventHandlingResult::None,
        }
        EventHandlingResult::Rerender
    }

    pub fn scroll_replacement_errors_up(&mut self) {
        if self.replacement_errors_pos == 0 {
            self.replacement_errors_pos = self.errors.len();
        }
        self.replacement_errors_pos = self.replacement_errors_pos.saturating_sub(1);
    }

    pub fn scroll_replacement_errors_down(&mut self) {
        if self.replacement_errors_pos >= self.errors.len().saturating_sub(1) {
            self.replacement_errors_pos = 0;
        } else {
            self.replacement_errors_pos += 1;
        }
    }
}

#[derive(Debug)]
pub struct SearchInProgressState {
    pub search_state: SearchState,
    pub(crate) last_render: Instant,
    pub(crate) search_started: Instant,
    handle: JoinHandle<()>,
}

impl SearchInProgressState {
    pub fn new(
        handle: JoinHandle<()>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    ) -> Self {
        Self {
            search_state: SearchState::new(processing_receiver),
            last_render: Instant::now(),
            search_started: Instant::now(),
            handle,
        }
    }
}

#[derive(Debug)]
pub struct SearchCompleteState {
    pub search_state: SearchState,
    pub(crate) search_time_taken: Duration,
}

impl SearchCompleteState {
    pub fn new(search_state: SearchState, search_started: Instant) -> Self {
        Self {
            search_state,
            search_time_taken: search_started.elapsed(),
        }
    }
}

#[derive(Debug)]
pub struct PerformingReplacementState {
    handle: Option<JoinHandle<()>>,
    #[allow(dead_code)]
    processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
}

impl PerformingReplacementState {
    pub fn new(
        handle: Option<JoinHandle<()>>,
        processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    ) -> Self {
        Self {
            handle,
            processing_sender,
            processing_receiver,
        }
    }

    fn set_handle(&mut self, handle: JoinHandle<()>) {
        self.handle = Some(handle);
    }
}

#[derive(Debug)]
pub enum Screen {
    SearchFields,
    SearchProgressing(SearchInProgressState),
    SearchComplete(SearchCompleteState),
    PerformingReplacement(PerformingReplacementState),
    Results(ReplaceState),
}

impl Screen {
    fn search_results_mut(&mut self) -> &mut SearchState {
        match self {
            Screen::SearchProgressing(SearchInProgressState { search_state, .. })
            | Screen::SearchComplete(SearchCompleteState { search_state, .. }) => search_state,
            _ => panic!("Expected SearchInProgress or SearchComplete, found {self:?}",),
        }
    }

    fn name(&self) -> &str {
        // TODO: is there a better way of doing this?
        match &self {
            Screen::SearchFields => "SearchFields",
            Screen::SearchProgressing(_) => "SearchProgressing",
            Screen::SearchComplete(_) => "SearchComplete",
            Screen::PerformingReplacement(_) => "PerformingReplacement",
            Screen::Results(_) => "Results",
        }
    }
}

#[derive(PartialEq)]
pub enum FieldName {
    Search,
    Replace,
    FixedStrings,
    WholeWord,
    MatchCase,
    IncludeFiles,
    ExcludeFiles,
}

#[derive(Clone)]
pub struct FieldValue<T> {
    pub value: T,
    pub set_by_cli: bool,
}

impl<T> FieldValue<T> {
    pub fn new(value: T, set_by_cli: bool) -> Self {
        Self { value, set_by_cli }
    }
}

impl<T: Default> Default for FieldValue<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            set_by_cli: false,
        }
    }
}

#[derive(Clone)]
pub struct SearchFieldValues<'a> {
    pub search: FieldValue<&'a str>,
    pub replace: FieldValue<&'a str>,
    pub fixed_strings: FieldValue<bool>,
    pub match_whole_word: FieldValue<bool>,
    pub match_case: FieldValue<bool>,
    pub include_files: FieldValue<&'a str>,
    pub exclude_files: FieldValue<&'a str>,
}
impl<'a> Default for SearchFieldValues<'a> {
    fn default() -> SearchFieldValues<'a> {
        Self {
            search: FieldValue::new(Self::DEFAULT_SEARCH, false),
            replace: FieldValue::new(Self::DEFAULT_REPLACE, false),
            fixed_strings: FieldValue::new(Self::DEFAULT_FIXED_STRINGS, false),
            match_whole_word: FieldValue::new(Self::DEFAULT_WHOLE_WORD, false),
            match_case: FieldValue::new(Self::DEFAULT_MATCH_CASE, false),
            include_files: FieldValue::new(Self::DEFAULT_INCLUDE_FILES, false),
            exclude_files: FieldValue::new(Self::DEFAULT_EXCLUDE_FILES, false),
        }
    }
}

impl SearchFieldValues<'_> {
    const DEFAULT_SEARCH: &'static str = "";
    const DEFAULT_REPLACE: &'static str = "";
    const DEFAULT_FIXED_STRINGS: bool = false;
    const DEFAULT_WHOLE_WORD: bool = false;
    const DEFAULT_MATCH_CASE: bool = true;
    const DEFAULT_INCLUDE_FILES: &'static str = "";
    const DEFAULT_EXCLUDE_FILES: &'static str = "";

    pub fn whole_word_default() -> bool {
        Self::DEFAULT_WHOLE_WORD
    }

    pub fn match_case_default() -> bool {
        Self::DEFAULT_MATCH_CASE
    }
}

pub struct SearchField {
    pub name: FieldName,
    pub field: Arc<RwLock<Field>>,
    pub set_by_cli: bool,
}

pub const NUM_SEARCH_FIELDS: usize = 7;

pub struct SearchFields {
    pub fields: [SearchField; NUM_SEARCH_FIELDS],
    pub highlighted: usize,
    advanced_regex: bool,
    pub disable_populated_fields: bool,
}

macro_rules! define_field_accessor {
    ($method_name:ident, $field_name:expr, $field_variant:ident, $return_type:ty) => {
        pub fn $method_name(&self) -> MappedRwLockReadGuard<'_, $return_type> {
            let field = self
                .fields
                .iter()
                .find(|SearchField { name, .. }| *name == $field_name)
                .expect("Couldn't find field");

            RwLockReadGuard::map(field.field.read(), |f| {
                if let Field::$field_variant(ref inner) = f {
                    inner
                } else {
                    panic!("Incorrect field type")
                }
            })
        }
    };
}

macro_rules! define_field_accessor_mut {
    ($method_name:ident, $field_name:expr, $field_variant:ident, $return_type:ty) => {
        pub fn $method_name(&self) -> MappedRwLockWriteGuard<'_, $return_type> {
            let field = self
                .fields
                .iter()
                .find(|SearchField { name, .. }| *name == $field_name)
                .expect("Couldn't find field");

            RwLockWriteGuard::map(field.field.write(), |f| {
                if let Field::$field_variant(ref mut inner) = f {
                    inner
                } else {
                    panic!("Incorrect field type")
                }
            })
        }
    };
}

impl SearchFields {
    // TODO: generate these automatically?
    define_field_accessor!(search, FieldName::Search, Text, TextField);
    define_field_accessor!(replace, FieldName::Replace, Text, TextField);
    define_field_accessor!(
        fixed_strings,
        FieldName::FixedStrings,
        Checkbox,
        CheckboxField
    );
    define_field_accessor!(whole_word, FieldName::WholeWord, Checkbox, CheckboxField);
    define_field_accessor!(match_case, FieldName::MatchCase, Checkbox, CheckboxField);
    define_field_accessor!(include_files, FieldName::IncludeFiles, Text, TextField);
    define_field_accessor!(exclude_files, FieldName::ExcludeFiles, Text, TextField);

    define_field_accessor_mut!(search_mut, FieldName::Search, Text, TextField);
    define_field_accessor_mut!(include_files_mut, FieldName::IncludeFiles, Text, TextField);
    define_field_accessor_mut!(exclude_files_mut, FieldName::ExcludeFiles, Text, TextField);

    #[allow(clippy::needless_pass_by_value)]
    pub fn with_values(search_field_values: SearchFieldValues<'_>) -> Self {
        let fields = [
            SearchField {
                name: FieldName::Search,
                field: Arc::new(RwLock::new(Field::text(search_field_values.search.value))),
                set_by_cli: search_field_values.search.set_by_cli,
            },
            SearchField {
                name: FieldName::Replace,
                field: Arc::new(RwLock::new(Field::text(search_field_values.replace.value))),
                set_by_cli: search_field_values.replace.set_by_cli,
            },
            SearchField {
                name: FieldName::FixedStrings,
                field: Arc::new(RwLock::new(Field::checkbox(
                    search_field_values.fixed_strings.value,
                ))),
                set_by_cli: search_field_values.fixed_strings.set_by_cli,
            },
            SearchField {
                name: FieldName::WholeWord,
                field: Arc::new(RwLock::new(Field::checkbox(
                    search_field_values.match_whole_word.value,
                ))),
                set_by_cli: search_field_values.match_whole_word.set_by_cli,
            },
            SearchField {
                name: FieldName::MatchCase,
                field: Arc::new(RwLock::new(Field::checkbox(
                    search_field_values.match_case.value,
                ))),
                set_by_cli: search_field_values.match_case.set_by_cli,
            },
            SearchField {
                name: FieldName::IncludeFiles,
                field: Arc::new(RwLock::new(Field::text(
                    search_field_values.include_files.value,
                ))),
                set_by_cli: search_field_values.include_files.set_by_cli,
            },
            SearchField {
                name: FieldName::ExcludeFiles,
                field: Arc::new(RwLock::new(Field::text(
                    search_field_values.exclude_files.value,
                ))),
                set_by_cli: search_field_values.exclude_files.set_by_cli,
            },
        ];

        Self {
            highlighted: Self::initial_highlight_position(&fields),
            fields,
            show_error_popup: false,
            advanced_regex: false,
            disable_populated_fields: false,
        }
    }

    fn initial_highlight_position(fields: &[SearchField]) -> usize {
        let mut highlighted = 0;
        for (index, field) in fields.iter().enumerate() {
            if !field.set_by_cli {
                highlighted = index;
                break;
            }
        }
        highlighted
    }

    fn all_populated(&self) -> bool {
        self.fields.iter().all(|f| f.set_by_cli)
    }

    pub fn with_disable_populated_fields(mut self, disable_populated_fields: bool) -> Self {
        self.disable_populated_fields = disable_populated_fields;
        self
    }

    pub fn with_advanced_regex(mut self, advanced_regex: bool) -> Self {
        self.advanced_regex = advanced_regex;
        self
    }

    fn highlighted_field_impl(&self) -> &SearchField {
        &self.fields[self.highlighted]
    }

    pub fn highlighted_field(&self) -> &Arc<RwLock<Field>> {
        &self.highlighted_field_impl().field
    }

    pub fn highlighted_field_name(&self) -> &FieldName {
        &self.highlighted_field_impl().name
    }

    pub fn focus_next(&mut self) {
        if self.all_populated() {
            return;
        }
        let mut next = (self.highlighted + 1) % self.fields.len();
        if self.disable_populated_fields {
            loop {
                if self.fields[next].set_by_cli {
                    next = (next + 1) % self.fields.len();
                    continue;
                }
                self.highlighted = next;
                break;
            }
            return;
        }
        self.highlighted = (self.highlighted + 1) % self.fields.len();
    }

    pub fn focus_prev(&mut self) {
        if self.all_populated() {
            return;
        }
        let mut prev = (self.highlighted + self.fields.len().saturating_sub(1)) % self.fields.len();
        if self.disable_populated_fields {
            loop {
                if self.fields[prev].set_by_cli {
                    prev = (prev + self.fields.len().saturating_sub(1)) % self.fields.len();
                    continue;
                }
                self.highlighted = prev;
                break;
            }
            return;
        }
        self.highlighted =
            (self.highlighted + self.fields.len().saturating_sub(1)) % self.fields.len();
    }

    pub fn errors(&self) -> Vec<AppError> {
        self.fields
            .iter()
            .filter_map(|field| {
                field.field.read().error().map(|err| AppError {
                    name: field.name.title().to_string(),
                    long: err.long,
                })
            })
            .collect::<Vec<_>>()
    }

    pub fn search_type(&self) -> anyhow::Result<SearchType> {
        let search = self.search();
        let search_text = search.text();
        let result = if self.fixed_strings().checked {
            SearchType::Fixed(search_text.to_string())
        } else if self.advanced_regex {
            SearchType::PatternAdvanced(FancyRegex::new(search_text)?)
        } else {
            SearchType::Pattern(Regex::new(search_text)?)
        };
        Ok(result)
    }
}

enum ValidatedField<T> {
    Parsed(T),
    Error,
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

pub struct App {
    pub current_screen: Screen,
    pub search_fields: SearchFields,
    pub directory: PathBuf,
    pub config: Config,
    pub event_sender: UnboundedSender<Event>,
    errors: Vec<AppError>,
    include_hidden: bool,
    popup: Option<Popup>,
}

const BINARY_EXTENSIONS: &[&str] = &["png", "gif", "jpg", "jpeg", "ico", "svg", "pdf"];

impl<'a> App {
    fn new(
        directory: Option<PathBuf>,
        include_hidden: bool,
        advanced_regex: bool,
        search_field_values: SearchFieldValues<'a>,
        event_sender: UnboundedSender<Event>,
    ) -> Self {
        let config = load_config().expect("Failed to read config file");

        let directory = match directory {
            Some(d) => d,
            None => current_dir().unwrap(),
        };

        let mut disable_populated_fields = false;
        if config
            .search
            .as_ref()
            .map(|c| c.disable_populated_fields)
            .unwrap_or(false)
        {
            disable_populated_fields = true
        }

        let search_fields = SearchFields::with_values(search_field_values.clone())
            .with_disable_populated_fields(disable_populated_fields)
            .with_advanced_regex(advanced_regex);

        Self {
            current_screen: Screen::SearchFields,
            search_fields,
            directory,
            include_hidden,
            config,
            errors: vec![],
            popup: None,
            event_sender,
        }
    }

    pub fn new_with_receiver(
        directory: Option<PathBuf>,
        include_hidden: bool,
        advanced_regex: bool,
        search_field_values: SearchFieldValues<'a>,
    ) -> (Self, UnboundedReceiver<Event>) {
        let (event_sender, app_event_receiver) = mpsc::unbounded_channel();
        let app = Self::new(
            directory,
            include_hidden,
            advanced_regex,
            search_field_values,
            event_sender,
        );
        (app, app_event_receiver)
    }

    pub fn cancel_search(&mut self) {
        if let Screen::SearchProgressing(SearchInProgressState { handle, .. }) =
            &mut self.current_screen
        {
            handle.abort();
        }
    }

    pub fn cancel_replacement(&mut self) {
        if let Screen::PerformingReplacement(PerformingReplacementState {
            handle: Some(ref mut handle),
            ..
        }) = &mut self.current_screen
        {
            handle.abort();
        }
    }

    pub fn reset(&mut self) {
        self.cancel_search();
        self.cancel_replacement();
        *self = Self::new(
            Some(self.directory.clone()),
            self.include_hidden,
            self.search_fields.advanced_regex,
            SearchFieldValues::default(),
            self.event_sender.clone(),
        );
    }

    pub async fn background_processing_recv(&mut self) -> Option<BackgroundProcessingEvent> {
        match &mut self.current_screen {
            Screen::SearchProgressing(SearchInProgressState {
                search_state:
                    SearchState {
                        processing_receiver,
                        ..
                    },
                ..
            })
            | Screen::SearchComplete(SearchCompleteState {
                search_state:
                    SearchState {
                        processing_receiver,
                        ..
                    },
                ..
            })
            | Screen::PerformingReplacement(PerformingReplacementState {
                processing_receiver,
                ..
            }) => processing_receiver.recv().await,
            _ => None,
        }
    }

    pub fn handle_app_event(&mut self, event: &AppEvent) -> EventHandlingResult {
        match event {
            AppEvent::Rerender => EventHandlingResult::Rerender,
            AppEvent::PerformSearch => self.perform_search_if_valid(),
        }
    }

    pub fn perform_search_if_valid(&mut self) -> EventHandlingResult {
        let (background_processing_sender, background_processing_receiver) =
            mpsc::unbounded_channel();

        match self.validate_fields(&background_processing_sender).unwrap() {
            None => {
                self.current_screen = Screen::SearchFields;
            }
            Some(parsed_fields) => {
                let handle = Self::update_search_results(
                    parsed_fields,
                    background_processing_sender.clone(),
                    self.event_sender.clone(),
                );
                self.current_screen = Screen::SearchProgressing(SearchInProgressState::new(
                    handle,
                    background_processing_receiver,
                ));
            }
        }

        EventHandlingResult::Rerender
    }

    pub fn trigger_replacement(&mut self) {
        let (background_processing_sender, background_processing_receiver) =
            mpsc::unbounded_channel();

        match mem::replace(
            &mut self.current_screen,
            Screen::PerformingReplacement(PerformingReplacementState::new(
                None,
                background_processing_sender.clone(),
                background_processing_receiver,
            )),
        ) {
            Screen::SearchComplete(SearchCompleteState { search_state, .. }) => {
                let handle = Self::perform_replacement(search_state, background_processing_sender);
                if let Screen::PerformingReplacement(ref mut state) = &mut self.current_screen {
                    state.set_handle(handle);
                } else {
                    panic!(
                        "Expected screen to be PerformingReplacement, found {:?}",
                        self.current_screen
                    );
                }
            }
            screen => {
                self.current_screen = screen;
            }
        }
    }

    pub fn perform_replacement(
        search_state: SearchState,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut path_groups: HashMap<PathBuf, Vec<SearchResult>> = HashMap::new();
            let (included, num_ignored) = split_results(search_state.results);
            for res in included {
                path_groups.entry(res.path.clone()).or_default().push(res);
            }

            let semaphore = Arc::new(Semaphore::new(8));
            let mut file_tasks = vec![];

            for (path, mut results) in path_groups {
                let semaphore = semaphore.clone();
                let task = tokio::spawn(async move {
                    let permit = semaphore.clone().acquire_owned().await.unwrap();
                    if let Err(file_err) = Self::replace_in_file(path, &mut results).await {
                        for res in &mut results {
                            res.replace_result = Some(ReplaceResult::Error(file_err.to_string()));
                        }
                    }
                    drop(permit);
                    results
                });
                file_tasks.push(task);
            }

            let replacement_results = future::join_all(file_tasks)
                .await
                .into_iter()
                .flat_map(Result::unwrap);
            let replace_state = Self::calculate_statistics(replacement_results, num_ignored);

            // Ignore error: we may have gone back to the previous screen
            let _ = background_processing_sender.send(
                BackgroundProcessingEvent::ReplacementCompleted(replace_state),
            );
        })
    }

    pub fn handle_background_processing_event(
        &mut self,
        event: BackgroundProcessingEvent,
    ) -> EventHandlingResult {
        match event {
            BackgroundProcessingEvent::AddSearchResult(result) => {
                let mut rerender = false;
                if let Screen::SearchProgressing(search_in_progress_state) =
                    &mut self.current_screen
                {
                    search_in_progress_state.search_state.results.push(result);

                    if search_in_progress_state.last_render.elapsed() >= Duration::from_millis(100)
                    {
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
                if let Screen::SearchProgressing(SearchInProgressState {
                    search_state,
                    search_started,
                    ..
                }) = mem::replace(&mut self.current_screen, Screen::SearchFields)
                {
                    self.current_screen = Screen::SearchComplete(SearchCompleteState::new(
                        search_state,
                        search_started,
                    ));
                }
                EventHandlingResult::Rerender
            }
            BackgroundProcessingEvent::ReplacementCompleted(replace_state) => {
                self.current_screen = Screen::Results(replace_state);
                EventHandlingResult::Rerender
            }
        }
    }

    fn handle_key_searching(&mut self, key: &KeyEvent) -> EventHandlingResult {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.event_sender
                    .send(Event::App(AppEvent::PerformSearch))
                    .unwrap();
            }
            (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::ALT) => {
                self.search_fields.focus_prev();
            }
            (KeyCode::Tab, _) => {
                self.search_fields.focus_next();
            }
            (code, modifiers) => {
                if let FieldName::FixedStrings = self.search_fields.highlighted_field_name() {
                    // TODO: ideally this should only happen when the field is checked, but for now this will do
                    self.search_fields.search_mut().clear_error();
                }
                self.search_fields
                    .highlighted_field()
                    .write()
                    .handle_keys(code, modifiers);
            }
        }
        EventHandlingResult::Rerender
    }

    /// Should only be called on `Screen::SearchProgressing` or `Screen::SearchComplete`
    fn try_handle_key_search(&mut self, key: &KeyEvent) -> Option<EventHandlingResult> {
        if !matches!(
            self.current_screen,
            Screen::SearchProgressing(_) | Screen::SearchComplete(_)
        ) {
            panic!(
                "Expected current_screen to be SearchProgressing or SearchComplete, found {:?}",
                self.current_screen
            );
        }

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.trigger_replacement();
                Some(EventHandlingResult::Rerender)
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.cancel_search();
                self.current_screen = Screen::SearchFields;
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
                let selected = self
                    .current_screen
                    .search_results_mut()
                    .primary_selected_field_mut();
                self.event_sender
                    .send(Event::LaunchEditor((
                        selected.path.clone(),
                        selected.line_number,
                    )))
                    .unwrap();
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
            return EventHandlingResult::Exit;
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
                    return EventHandlingResult::Exit;
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
            Screen::SearchFields => self.handle_key_searching(key),
            Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
                if let Some(res) = self.try_handle_key_search(key) {
                    res
                } else {
                    match &mut self.current_screen {
                        Screen::SearchProgressing(SearchInProgressState { search_state, .. }) |
                            Screen::SearchComplete(SearchCompleteState { search_state, .. }) => search_state.handle_key(key),
                        screen => panic!(
                            "Expected current_screen to be SearchProgressing or SearchComplete, found {screen:?}",
                        ),
                    }
                }
            }
            Screen::PerformingReplacement(_) => EventHandlingResult::Rerender,
            Screen::Results(replace_state) => replace_state.handle_key_results(key),
        }
    }

    fn is_regex_error(e: &Error) -> bool {
        e.downcast_ref::<regex::Error>().is_some()
            || e.downcast_ref::<fancy_regex::Error>().is_some()
    }

    fn validate_fields(
        &mut self,
        background_processing_sender: &UnboundedSender<BackgroundProcessingEvent>,
    ) -> anyhow::Result<Option<ParsedFields>> {
        let search_pattern = match self.search_fields.search_type() {
            Ok(p) => ValidatedField::Parsed(p),
            Err(e) => {
                if Self::is_regex_error(&e) {
                    self.search_fields
                        .search_mut()
                        .set_error("Couldn't parse regex".to_owned(), e.to_string());
                    ValidatedField::Error
                } else {
                    return Err(e);
                }
            }
        };

        let overrides = self.validate_overrides()?;

        if let (ValidatedField::Parsed(search_pattern), ValidatedField::Parsed(overrides)) =
            (search_pattern, overrides)
        {
            Ok(Some(ParsedFields::new(
                search_pattern,
                self.search_fields.replace().text().to_string(),
                self.search_fields.whole_word().checked,
                self.search_fields.match_case().checked,
                overrides,
                self.directory.clone(),
                self.include_hidden,
                background_processing_sender.clone(),
            )))
        } else {
            self.set_popup(Popup::Error);
            Ok(None)
        }
    }

    fn add_overrides(
        overrides: &mut OverrideBuilder,
        files: &str,
        prefix: &str,
    ) -> anyhow::Result<()> {
        for file in files.split(',') {
            let file = file.trim();
            if !file.is_empty() {
                overrides.add(&format!("{prefix}{file}"))?;
            }
        }
        Ok(())
    }

    fn validate_overrides(&mut self) -> anyhow::Result<ValidatedField<Override>> {
        let mut overrides = OverrideBuilder::new(self.directory.clone());
        let mut success = true;

        let include_res = Self::add_overrides(
            &mut overrides,
            self.search_fields.include_files().text(),
            "",
        );
        if let Err(e) = include_res {
            self.search_fields
                .include_files_mut()
                .set_error("Couldn't parse glob pattern".to_string(), e.to_string());
            success = false;
        }

        let exlude_res = Self::add_overrides(
            &mut overrides,
            self.search_fields.exclude_files().text(),
            "!",
        );
        if let Err(e) = exlude_res {
            self.search_fields
                .exclude_files_mut()
                .set_error("Couldn't parse glob pattern".to_string(), e.to_string());
            success = false;
        }

        if success {
            let overrides = overrides.build()?;
            Ok(ValidatedField::Parsed(overrides))
        } else {
            Ok(ValidatedField::Error)
        }
    }

    pub fn update_search_results(
        parsed_fields: ParsedFields,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        event_sender: UnboundedSender<Event>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let walker = parsed_fields.build_walker();

            tokio::spawn(async move {
                walker.run(|| {
                    Box::new(|result| {
                        let Ok(entry) = result else {
                            return WalkState::Continue;
                        };

                        let is_file = entry.file_type().is_some_and(|ft| ft.is_file());
                        #[allow(clippy::collapsible_if)]
                        if is_file && !Self::ignore_file(entry.path()) {
                            if tx.send(entry.path().to_owned()).is_err() {
                                return WalkState::Quit;
                            }
                        }

                        WalkState::Continue
                    })
                });
            });

            let mut rerender_interval = tokio::time::interval(Duration::from_millis(393)); // Slightly random duration so that time taken isn't a round number
            rerender_interval.tick().await;

            loop {
                tokio::select! {
                    maybe_path = rx.recv() => {
                        match maybe_path {
                            Some(path) => parsed_fields.handle_path(&path).await,
                            None => break,
                        }
                    },
                    _ = rerender_interval.tick() => {
                        let _ = event_sender.send(Event::App(AppEvent::Rerender));
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

    fn ignore_file(path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            if let Some(ext_str) = ext.to_str() {
                if BINARY_EXTENSIONS.contains(&ext_str) {
                    return true;
                }
            }
        }
        false
    }

    fn calculate_statistics<I>(results: I, num_ignored: usize) -> ReplaceState
    where
        I: IntoIterator<Item = SearchResult>,
    {
        let mut num_successes = 0;
        let mut errors = vec![];

        results.into_iter().for_each(|res| {
            assert!(
                res.included,
                "Expected only included results, found {res:?}"
            );
            match &res.replace_result {
                Some(ReplaceResult::Success) => {
                    num_successes += 1;
                }
                None => {
                    let mut res = res.clone();
                    res.replace_result = Some(ReplaceResult::Error(
                        "Failed to find search result in file".to_owned(),
                    ));
                    errors.push(res);
                }
                Some(ReplaceResult::Error(_)) => {
                    errors.push(res.clone());
                }
            }
        });

        ReplaceState {
            num_successes,
            num_ignored,
            errors,
            replacement_errors_pos: 0,
        }
    }

    async fn replace_in_file(
        file_path: PathBuf,
        results: &mut [SearchResult],
    ) -> anyhow::Result<()> {
        let mut line_map: HashMap<_, _> = results
            .iter_mut()
            .map(|res| (res.line_number, res))
            .collect();

        let parent_dir = file_path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot create temp file: target path '{}' has no parent directory",
                file_path.display()
            )
        })?;
        let temp_output_file = NamedTempFile::new_in(parent_dir)?;

        // Scope the file operations so they're closed before rename
        {
            let input = File::open(&file_path).await?;
            let reader = BufReader::new(input);

            let output = File::create(temp_output_file.path()).await?;
            let mut writer = BufWriter::new(output);

            let mut lines = reader.lines();
            let mut line_number = 0;
            while let Some(mut line) = lines.next_line().await? {
                if let Some(res) = line_map.get_mut(&(line_number + 1)) {
                    if line == res.line {
                        line.clone_from(&res.replacement);
                        res.replace_result = Some(ReplaceResult::Success);
                    } else {
                        res.replace_result = Some(ReplaceResult::Error(
                            "File changed since last search".to_owned(),
                        ));
                    }
                }
                line.push('\n');
                writer.write_all(line.as_bytes()).await?;
                line_number += 1;
            }

            writer.flush().await?;
        }

        temp_output_file.persist(&file_path)?;
        Ok(())
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

    fn keymaps_impl(&self, compact: bool) -> Vec<(&str, String)> {
        enum Show {
            Both,
            FullOnly,
            CompactOnly,
        }

        let current_screen_keys = match self.current_screen {
            Screen::SearchFields => {
                vec![
                    ("<enter>", "search", Show::Both),
                    ("<tab>", "focus next", Show::Both),
                    ("<S-tab>", "focus previous", Show::FullOnly),
                    ("<space>", "toggle checkbox", Show::FullOnly),
                ]
            }
            Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
                let mut keys = if let Screen::SearchComplete(_) = self.current_screen {
                    vec![("<enter>", "replace selected", Show::Both)]
                } else {
                    vec![]
                };
                keys.append(&mut vec![
                    ("<space>", "toggle", Show::Both),
                    ("a", "toggle all", Show::FullOnly),
                    ("v", "toggle multi-select mode", Show::FullOnly),
                    ("<A-;>", "flip multi-select direction", Show::FullOnly),
                    ("e", "open in editor", Show::FullOnly),
                    ("<C-o>", "back", Show::Both),
                    ("j", "up", Show::FullOnly),
                    ("k", "down", Show::FullOnly),
                    ("<C-u>", "up half a page", Show::FullOnly),
                    ("<C-d>", "down half a page", Show::FullOnly),
                    ("<C-b>", "up a full page", Show::FullOnly),
                    ("<C-f>", "down a full page", Show::FullOnly),
                    ("g", "jump to top", Show::FullOnly),
                    ("G", "jump to bottom", Show::FullOnly),
                ]);
                keys
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

        let is_search_screen = matches!(
            self.current_screen,
            Screen::SearchProgressing(_) | Screen::SearchComplete(_)
        );
        let esc_help = format!(
            "quit / close popup{}",
            if is_search_screen {
                " / exit multi-select"
            } else {
                ""
            }
        );

        let additional_keys = vec![
            (
                "<C-r>",
                "reset",
                if is_search_screen {
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
            Screen::SearchProgressing(SearchInProgressState { search_state, .. })
            | Screen::SearchComplete(SearchCompleteState { search_state, .. }) => {
                search_state.multiselect_enabled()
            }
            _ => false,
        }
    }

    fn toggle_multiselect_mode(&mut self) {
        match &mut self.current_screen {
            Screen::SearchProgressing(SearchInProgressState { search_state, .. })
            | Screen::SearchComplete(SearchCompleteState { search_state, .. }) => {
                search_state.toggle_multiselect_mode();
            }
            _ => panic!(
                "Tried to disable multi-select on {:?}",
                self.current_screen.name()
            ),
        }
    }
}

fn split_results(results: Vec<SearchResult>) -> (Vec<SearchResult>, usize) {
    let (included, excluded): (Vec<_>, Vec<_>) = results.into_iter().partition(|res| res.included);
    let num_ignored = excluded.len();
    (included, num_ignored)
}

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;

    fn random_num() -> usize {
        let mut rng = rand::rng();
        rng.random_range(1..10000)
    }

    fn search_result(included: bool) -> SearchResult {
        SearchResult {
            path: Path::new("random/file").to_path_buf(),
            line_number: random_num(),
            line: "foo".to_owned(),
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
            replacement: "bar".to_owned(),
            included: true,
            replace_result: Some(ReplaceResult::Error("error".to_owned())),
        }
    }

    fn build_test_app(results: Vec<SearchResult>) -> App {
        let (event_sender, _) = mpsc::unbounded_channel();
        let mut app = App::new(
            None,
            false,
            false,
            SearchFieldValues::default(),
            event_sender,
        );
        app.current_screen = Screen::SearchComplete(SearchCompleteState::new(
            build_test_search_state_with_results(results),
            Instant::now(),
        ));
        app
    }

    #[tokio::test]
    async fn test_calculate_statistics_all_success() {
        let app = build_test_app(vec![success_result(), success_result(), success_result()]);
        let stats = if let Screen::SearchComplete(search_complete_state) = app.current_screen {
            let (results, num_ignored) = split_results(search_complete_state.search_state.results);
            App::calculate_statistics(results, num_ignored)
        } else {
            panic!("Expected SearchComplete");
        };

        assert_eq!(
            stats,
            ReplaceState {
                num_successes: 3,
                num_ignored: 0,
                errors: vec![],
                replacement_errors_pos: 0,
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
        let stats = if let Screen::SearchComplete(search_complete_state) = app.current_screen {
            let (results, num_ignored) = split_results(search_complete_state.search_state.results);
            App::calculate_statistics(results, num_ignored)
        } else {
            panic!("Expected SearchComplete");
        };

        assert_eq!(
            stats,
            ReplaceState {
                num_successes: 2,
                num_ignored: 2,
                errors: vec![error_result],
                replacement_errors_pos: 0,
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
