use anyhow::Error;
use fancy_regex::Regex as FancyRegex;
use ignore::WalkState;
use itertools::Itertools;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use regex::Regex;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use crate::{
    event::{AppEvent, BackgroundProcessingEvent, ReplaceResult, SearchResult},
    fields::{CheckboxField, Field, FieldError, TextField},
    parsed_fields::{ParsedFields, SearchType},
    utils::relative_path_from,
    EventHandlingResult,
};

#[derive(Debug, Eq, PartialEq)]
pub struct SearchState {
    pub results: Vec<SearchResult>,
    pub selected: usize, // TODO: allow for selection of ranges
}

impl SearchState {
    pub fn move_selected_up(&mut self) {
        if self.selected == 0 {
            self.selected = self.results.len();
        }
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_selected_down(&mut self) {
        if self.selected >= self.results.len().saturating_sub(1) {
            self.selected = 0;
        } else {
            self.selected += 1;
        }
    }

    pub fn toggle_selected_inclusion(&mut self) {
        if self.selected < self.results.len() {
            let selected_result = &mut self.results[self.selected];
            selected_result.included = !selected_result.included;
        } else {
            self.selected = self.results.len().saturating_sub(1);
        }
    }

    pub fn toggle_all_selected(&mut self) {
        let all_included = self.results.iter().all(|res| res.included);
        self.results
            .iter_mut()
            .for_each(|res| res.included = !all_included);
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
    fn handle_key_results(&mut self, key: &KeyEvent) -> bool {
        let mut exit = false;
        match (key.code, key.modifiers) {
            (KeyCode::Char('j') | KeyCode::Down, _)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_down();
            }
            (KeyCode::Char('k') | KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.scroll_replacement_errors_up();
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {} // TODO
            (KeyCode::PageDown, _) => {}                      // TODO
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {} // TODO
            (KeyCode::PageUp, _) => {}                        // TODO
            (KeyCode::Enter | KeyCode::Char('q'), _) => {
                exit = true;
            }
            _ => {}
        };
        exit
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
    pub last_render: Instant,
    handle: JoinHandle<()>,
    processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
}

impl SearchInProgressState {
    fn new(
        handle: JoinHandle<()>,
        processing_sender: UnboundedSender<BackgroundProcessingEvent>,
        processing_receiver: UnboundedReceiver<BackgroundProcessingEvent>,
    ) -> Self {
        Self {
            search_state: SearchState {
                results: vec![],
                selected: 0,
            },
            last_render: Instant::now(),
            handle,
            processing_sender,
            processing_receiver,
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
    SearchComplete(SearchState),
    PerformingReplacement(PerformingReplacementState),
    Results(ReplaceState),
}

impl Screen {
    fn search_results_mut(&mut self) -> &mut SearchState {
        match self {
            Screen::SearchProgressing(SearchInProgressState { search_state, .. }) => search_state,
            Screen::SearchComplete(search_state) => search_state,
            _ => panic!(
                "Expected SearchInProgress or SearchComplete, found {:?}",
                self
            ),
        }
    }
}

#[derive(PartialEq)]
pub enum FieldName {
    Search,
    Replace,
    FixedStrings,
    PathPattern,
}

pub struct SearchField {
    pub name: FieldName,
    pub field: Arc<RwLock<Field>>,
}

pub const NUM_SEARCH_FIELDS: usize = 4;

pub struct SearchFields {
    pub fields: [SearchField; NUM_SEARCH_FIELDS],
    pub highlighted: usize,
    pub show_error_popup: bool,
    advanced_regex: bool,
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
    define_field_accessor!(search, FieldName::Search, Text, TextField);
    define_field_accessor!(replace, FieldName::Replace, Text, TextField);
    define_field_accessor!(
        fixed_strings,
        FieldName::FixedStrings,
        Checkbox,
        CheckboxField
    );
    define_field_accessor!(path_pattern, FieldName::PathPattern, Text, TextField);

    define_field_accessor_mut!(search_mut, FieldName::Search, Text, TextField);
    define_field_accessor_mut!(path_pattern_mut, FieldName::PathPattern, Text, TextField);

    pub fn with_values(
        search: impl Into<String>,
        replace: impl Into<String>,
        fixed_strings: bool,
        filename_pattern: impl Into<String>,
    ) -> Self {
        Self {
            fields: [
                SearchField {
                    name: FieldName::Search,
                    field: Arc::new(RwLock::new(Field::text(search.into()))),
                },
                SearchField {
                    name: FieldName::Replace,
                    field: Arc::new(RwLock::new(Field::text(replace.into()))),
                },
                SearchField {
                    name: FieldName::FixedStrings,
                    field: Arc::new(RwLock::new(Field::checkbox(fixed_strings))),
                },
                SearchField {
                    name: FieldName::PathPattern,
                    field: Arc::new(RwLock::new(Field::text(filename_pattern.into()))),
                },
            ],
            highlighted: 0,
            show_error_popup: false,
            advanced_regex: false,
        }
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
        self.highlighted = (self.highlighted + 1) % self.fields.len();
    }

    pub fn focus_prev(&mut self) {
        self.highlighted =
            (self.highlighted + self.fields.len().saturating_sub(1)) % self.fields.len();
    }

    pub fn errors(&self) -> Vec<(&str, FieldError)> {
        self.fields
            .iter()
            .filter_map(|field| {
                field
                    .field
                    .read()
                    .error()
                    .map(|err| (field.name.title(), err))
            })
            .collect::<Vec<_>>()
    }

    pub fn search_type(&self) -> anyhow::Result<SearchType> {
        let search = self.search();
        let search_text = search.text();
        let result = if self.fixed_strings().checked {
            SearchType::Fixed(search_text)
        } else if self.advanced_regex {
            SearchType::PatternAdvanced(FancyRegex::new(&search_text)?)
        } else {
            SearchType::Pattern(Regex::new(&search_text)?)
        };
        Ok(result)
    }

    pub fn path_pattern_parsed(&self) -> anyhow::Result<Option<SearchType>> {
        let path_patt_text = &self.path_pattern().text;
        let result = if path_patt_text.is_empty() {
            None
        } else {
            Some({
                if self.advanced_regex {
                    SearchType::PatternAdvanced(FancyRegex::new(path_patt_text)?)
                } else {
                    SearchType::Pattern(Regex::new(path_patt_text)?)
                }
            })
        };
        Ok(result)
    }
}

enum ValidatedField<T> {
    Parsed(T),
    Error,
}

pub struct App {
    pub current_screen: Screen,
    pub search_fields: SearchFields,
    directory: PathBuf,
    include_hidden: bool,

    app_event_sender: UnboundedSender<AppEvent>,
}

const BINARY_EXTENSIONS: &[&str] = &["png", "gif", "jpg", "jpeg", "ico", "svg", "pdf"];

impl App {
    pub fn new(
        directory: Option<PathBuf>,
        include_hidden: bool,
        advanced_regex: bool,
        app_event_sender: UnboundedSender<AppEvent>,
    ) -> Self {
        let directory = match directory {
            Some(d) => d,
            None => std::env::current_dir().unwrap(),
        };
        let search_fields =
            SearchFields::with_values("", "", false, "").with_advanced_regex(advanced_regex);

        Self {
            current_screen: Screen::SearchFields,
            search_fields,
            directory,
            include_hidden,

            app_event_sender,
        }
    }

    pub fn cancel_search(&mut self) {
        if let Screen::SearchProgressing(SearchInProgressState { handle, .. }) =
            &mut self.current_screen
        {
            handle.abort();
        }
        self.current_screen = Screen::SearchFields;
    }

    pub fn cancel_replacement(&mut self) {
        if let Screen::PerformingReplacement(PerformingReplacementState {
            handle: Some(ref mut handle),
            ..
        }) = &mut self.current_screen
        {
            handle.abort()
        }
    }

    pub fn reset(&mut self) {
        self.cancel_search();
        self.cancel_replacement();
        *self = Self::new(
            Some(self.directory.clone()),
            self.include_hidden,
            self.search_fields.advanced_regex,
            self.app_event_sender.clone(),
        );
    }

    pub async fn background_processing_recv(&mut self) -> Option<BackgroundProcessingEvent> {
        match &mut self.current_screen {
            Screen::SearchProgressing(SearchInProgressState {
                processing_receiver,
                ..
            }) => processing_receiver.recv().await,
            Screen::PerformingReplacement(PerformingReplacementState {
                processing_receiver,
                ..
            }) => processing_receiver.recv().await,
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn background_processing_sender(
        &mut self,
    ) -> Option<&mut UnboundedSender<BackgroundProcessingEvent>> {
        if let Screen::SearchProgressing(SearchInProgressState {
            processing_sender, ..
        }) = &mut self.current_screen
        {
            Some(processing_sender)
        } else {
            None
        }
    }

    pub async fn handle_app_event(&mut self, event: AppEvent) -> EventHandlingResult {
        match event {
            AppEvent::Rerender => EventHandlingResult {
                exit: false,
                rerender: true,
            },
            AppEvent::PerformSearch => self.perform_search_if_valid(),
        }
    }

    pub fn perform_search_if_valid(&mut self) -> EventHandlingResult {
        let (background_processing_sender, background_processing_receiver) =
            mpsc::unbounded_channel();

        match self
            .validate_fields(background_processing_sender.clone())
            .unwrap()
        {
            None => {
                self.current_screen = Screen::SearchFields;
            }
            Some(parsed_fields) => {
                let handle = Self::update_search_results(
                    parsed_fields,
                    background_processing_sender.clone(),
                );
                self.current_screen = Screen::SearchProgressing(SearchInProgressState::new(
                    handle,
                    background_processing_sender,
                    background_processing_receiver,
                ));
            }
        };

        EventHandlingResult {
            exit: false,
            rerender: true,
        }
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
            Screen::SearchComplete(search_state) => {
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
        mut search_state: SearchState,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            for (path, results) in &search_state
                .results
                .iter_mut()
                .filter(|res| res.included)
                .chunk_by(|res| res.path.clone())
            {
                let mut results = results.collect::<Vec<_>>();
                if let Err(file_err) = Self::replace_in_file(path, &mut results) {
                    results.iter_mut().for_each(|res| {
                        res.replace_result = Some(ReplaceResult::Error(file_err.to_string()))
                    });
                }
            }

            let replace_state = Self::calculate_statistics(&search_state.results);

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
                EventHandlingResult {
                    exit: false,
                    rerender,
                }
            }
            BackgroundProcessingEvent::SearchCompleted => {
                if let Screen::SearchProgressing(SearchInProgressState { search_state, .. }) =
                    mem::replace(&mut self.current_screen, Screen::SearchFields)
                {
                    self.current_screen = Screen::SearchComplete(search_state);
                }
                EventHandlingResult {
                    exit: false,
                    rerender: true,
                }
            }
            BackgroundProcessingEvent::ReplacementCompleted(replace_state) => {
                self.current_screen = Screen::Results(replace_state);
                EventHandlingResult {
                    exit: false,
                    rerender: true,
                }
            }
        }
    }

    fn handle_key_searching(&mut self, key: &KeyEvent) -> bool {
        if self.search_fields.show_error_popup {
            self.search_fields.show_error_popup = false;
        } else {
            match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => {
                    self.app_event_sender.send(AppEvent::PerformSearch).unwrap();
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
                    };
                    self.search_fields
                        .highlighted_field()
                        .write()
                        .handle_keys(code, modifiers);
                }
            }
        };
        false
    }

    fn handle_key_confirmation(&mut self, key: &KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j') | KeyCode::Down, _)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.current_screen
                    .search_results_mut()
                    .move_selected_down();
            }
            (KeyCode::Char('k') | KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                // TODO: need to fix issue where screen gets out of sync with state
                self.current_screen.search_results_mut().move_selected_up();
            }
            (KeyCode::Char(' '), _) => {
                self.current_screen
                    .search_results_mut()
                    .toggle_selected_inclusion();
            }
            (KeyCode::Char('a'), _) => {
                self.current_screen
                    .search_results_mut()
                    .toggle_all_selected();
            }
            (KeyCode::Enter, _) => {
                self.trigger_replacement();
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.cancel_search();
                self.current_screen = Screen::SearchFields;
                self.app_event_sender.send(AppEvent::Rerender).unwrap();
            }
            _ => {}
        };
        false
    }

    pub fn handle_key_events(&mut self, key: &KeyEvent) -> anyhow::Result<EventHandlingResult> {
        if key.kind == KeyEventKind::Release {
            return Ok(EventHandlingResult {
                exit: false,
                rerender: true,
            });
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                if !self.search_fields.show_error_popup =>
            {
                self.reset();
                return Ok(EventHandlingResult {
                    exit: true,
                    rerender: true,
                });
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.reset();
                return Ok(EventHandlingResult {
                    exit: false,
                    rerender: true,
                });
            }
            (_, _) => {}
        }

        let exit = match &mut self.current_screen {
            Screen::SearchFields => self.handle_key_searching(key),
            Screen::SearchProgressing(_) | Screen::SearchComplete(_) => {
                self.handle_key_confirmation(key)
            }
            Screen::PerformingReplacement(_) => false,
            Screen::Results(replace_state) => replace_state.handle_key_results(key),
        };
        Ok(EventHandlingResult {
            exit,
            rerender: true,
        })
    }

    fn is_regex_error(e: &Error) -> bool {
        e.downcast_ref::<regex::Error>().is_some()
            || e.downcast_ref::<fancy_regex::Error>().is_some()
    }

    fn validate_fields(
        &mut self,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> anyhow::Result<Option<ParsedFields>> {
        let search_pattern = match self.search_fields.search_type() {
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
            Ok(p) => ValidatedField::Parsed(p),
        };

        let path_pattern = match self.search_fields.path_pattern_parsed() {
            Err(e) => {
                self.search_fields
                    .path_pattern_mut()
                    .set_error("Couldn't parse regex".to_owned(), e.to_string());
                ValidatedField::Error
            }
            Ok(r) => ValidatedField::Parsed(r),
        };

        let (search_pattern, path_pattern) = match (search_pattern, path_pattern) {
            (ValidatedField::Parsed(s), ValidatedField::Parsed(p)) => (s, p),
            _ => {
                self.search_fields.show_error_popup = true;
                return Ok(None);
            }
        };

        Ok(Some(ParsedFields::new(
            search_pattern,
            self.search_fields.replace().text(),
            path_pattern,
            self.directory.clone(),
            self.include_hidden,
            background_processing_sender.clone(),
        )))
    }

    pub fn update_search_results(
        parsed_fields: ParsedFields,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> JoinHandle<()> {
        let walker = parsed_fields.build_walker();

        tokio::spawn(async move {
            walker.run(|| {
                let parsed_fields = parsed_fields.clone();

                Box::new(move |entry| {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => return WalkState::Continue,
                    };

                    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                        return WalkState::Continue;
                    };

                    if Self::ignore_file(entry.path()) {
                        return WalkState::Continue;
                    }

                    parsed_fields.handle_path(entry.path());

                    WalkState::Continue
                })
            });

            // Ignore error: we may have gone back to the previous screen
            let _ = background_processing_sender.send(BackgroundProcessingEvent::SearchCompleted);
        })
    }

    fn ignore_file(path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            if let Some(ext_str) = ext.to_str() {
                if BINARY_EXTENSIONS.contains(&ext_str.to_lowercase().as_str()) {
                    return true;
                }
            }
        }
        false
    }

    fn calculate_statistics(results: &[SearchResult]) -> ReplaceState {
        let mut num_successes = 0;
        let mut num_ignored = 0;
        let mut errors = vec![];

        results
            .iter()
            .for_each(|res| match (res.included, &res.replace_result) {
                (false, _) => {
                    num_ignored += 1;
                }
                (_, Some(ReplaceResult::Success)) => {
                    num_successes += 1;
                }
                (_, None) => {
                    let mut res = res.clone();
                    res.replace_result = Some(ReplaceResult::Error(
                        "Failed to find search result in file".to_owned(),
                    ));
                    errors.push(res);
                }
                (_, Some(ReplaceResult::Error(_))) => {
                    errors.push(res.clone());
                }
            });

        ReplaceState {
            num_successes,
            num_ignored,
            errors,
            replacement_errors_pos: 0,
        }
    }

    fn replace_in_file(
        file_path: PathBuf,
        results: &mut [&mut SearchResult],
    ) -> anyhow::Result<()> {
        let mut line_map: HashMap<_, _> =
            HashMap::from_iter(results.iter_mut().map(|res| (res.line_number, res)));

        let input = File::open(file_path.clone())?;
        let buffered = BufReader::new(input);

        let temp_file_path = file_path.with_extension("tmp");
        let output = File::create(temp_file_path.clone())?;
        let mut writer = BufWriter::new(output);

        for (index, line) in buffered.lines().enumerate() {
            let mut line = line?;
            if let Some(res) = line_map.get_mut(&(index + 1)) {
                if line == res.line {
                    line.clone_from(&res.replacement);
                    res.replace_result = Some(ReplaceResult::Success);
                } else {
                    res.replace_result = Some(ReplaceResult::Error(
                        "File changed since last search".to_owned(),
                    ));
                }
            }
            writeln!(writer, "{}", line)?;
        }

        writer.flush()?;
        fs::rename(temp_file_path, file_path)?;
        Ok(())
    }

    pub fn relative_path(&self, path: &Path) -> String {
        relative_path_from(&self.directory, path)
    }
}

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;
    use crate::EventHandler;

    fn random_num() -> usize {
        let mut rng = rand::thread_rng();
        rng.gen_range(1..10000)
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

    #[test]
    fn test_toggle_all_selected_when_all_selected() {
        let mut search_state = SearchState {
            results: vec![
                search_result(true),
                search_result(true),
                search_result(true),
            ],
            selected: 0,
        };
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
        let mut search_state = SearchState {
            results: vec![
                search_result(false),
                search_result(false),
                search_result(false),
            ],
            selected: 0,
        };
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
        let mut search_state = SearchState {
            results: vec![
                search_result(true),
                search_result(false),
                search_result(true),
            ],
            selected: 0,
        };
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
        let mut search_state = SearchState {
            results: vec![],
            selected: 0,
        };
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
        let event_handler = EventHandler::new();
        let mut app = App::new(None, false, false, event_handler.app_event_sender);
        app.current_screen = Screen::SearchComplete(SearchState {
            results,
            selected: 0,
        });
        app
    }

    #[tokio::test]
    async fn test_calculate_statistics_all_success() {
        let app = build_test_app(vec![success_result(), success_result(), success_result()]);
        let stats = if let Screen::SearchComplete(search_state) = &app.current_screen {
            App::calculate_statistics(&search_state.results)
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
        let stats = if let Screen::SearchComplete(search_state) = &app.current_screen {
            App::calculate_statistics(&search_state.results)
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
}
