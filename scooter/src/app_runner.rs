use anyhow::bail;
use crossterm::event::{self, Event as CrosstermEvent};
use futures::Stream;
use futures::StreamExt;
use ignore::WalkState;
use log::error;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use scooter_core::search::SearchResult;
use std::env;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::logging::DEFAULT_LOG_LEVEL;
use crate::{
    app::{App, AppError, AppRunConfig, Event, EventHandlingResult},
    fields::SearchFieldValues,
    logging::setup_logging,
    replace::{calculate_statistics, format_replacement_results},
    tui::Tui,
    utils::validate_dir_or_default,
};

use crate::validation::{
    validate_search_configuration, SearchConfiguration, SimpleErrorHandler, ValidationResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig<'a> {
    pub directory: Option<String>,
    pub log_level: LevelFilter,
    pub search_field_values: SearchFieldValues<'a>,
    pub app_run_config: AppRunConfig,
}

impl Default for AppConfig<'_> {
    fn default() -> Self {
        Self {
            directory: None,
            log_level: LevelFilter::from_str(DEFAULT_LOG_LEVEL).unwrap(),
            search_field_values: SearchFieldValues::default(),
            app_run_config: AppRunConfig::default(),
        }
    }
}

impl TryFrom<AppConfig<'_>> for SearchConfiguration {
    type Error = anyhow::Error;

    fn try_from(config: AppConfig<'_>) -> anyhow::Result<Self> {
        Ok(SearchConfiguration {
            search_text: config.search_field_values.search.value.to_string(),
            replacement_text: config.search_field_values.replace.value.to_string(),
            fixed_strings: config.search_field_values.fixed_strings.value,
            advanced_regex: config.app_run_config.advanced_regex,
            include_globs: config.search_field_values.include_files.value.to_string(),
            exclude_globs: config.search_field_values.exclude_files.value.to_string(),
            match_whole_word: config.search_field_values.match_whole_word.value,
            match_case: config.search_field_values.match_case.value,
            include_hidden: config.app_run_config.include_hidden,
            directory: validate_dir_or_default(config.directory)?,
        })
    }
}

pub trait EventStream:
    Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin
{
}
impl<T: Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin> EventStream for T {}

pub type CrosstermEventStream = event::EventStream;

pub struct AppRunner<B: Backend, E: EventStream, S: SnapshotProvider<B>> {
    app: App,
    event_receiver: UnboundedReceiver<Event>,
    tui: Tui<B>,
    event_stream: E,
    snapshot_provider: S,
}

pub trait SnapshotProvider<B: Backend> {
    fn send_snapshot(&self, tui: &Tui<B>);
}

pub struct NoOpSnapshotProvider;

impl<B: Backend> SnapshotProvider<B> for NoOpSnapshotProvider {
    #[inline]
    fn send_snapshot(&self, _tui: &Tui<B>) {
        // No-op - optimized away in release builds
    }
}

pub struct TestSnapshotProvider {
    sender: UnboundedSender<String>,
}

// Used in integration tests
#[allow(dead_code)]
impl TestSnapshotProvider {
    pub fn new(sender: UnboundedSender<String>) -> Self {
        Self { sender }
    }
}

impl SnapshotProvider<TestBackend> for TestSnapshotProvider {
    fn send_snapshot(&self, tui: &Tui<TestBackend>) {
        let buffer = tui.terminal.backend().buffer();
        let contents = buffer
            .content
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if i % buffer.area.width as usize == 0 && i > 0 {
                    "\n"
                } else {
                    cell.symbol()
                }
            })
            .collect::<Vec<_>>()
            .join("");
        let _ = self.sender.send(contents);
    }
}

impl AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream, NoOpSnapshotProvider> {
    pub fn new_runner(config: AppConfig<'_>) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        let snapshot_provider = NoOpSnapshotProvider;
        Self::new(config, backend, event_stream, snapshot_provider)
    }
}

impl<E: EventStream> AppRunner<TestBackend, E, TestSnapshotProvider> {
    // Used in integration tests
    #[allow(dead_code)]
    pub fn new_test_with_snapshot(
        config: AppConfig<'_>,
        backend: TestBackend,
        event_stream: E,
        snapshot_sender: UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        let snapshot_provider = TestSnapshotProvider::new(snapshot_sender);
        Self::new(config, backend, event_stream, snapshot_provider)
    }
}

impl<B: Backend + 'static, E: EventStream, S: SnapshotProvider<B>> AppRunner<B, E, S> {
    pub fn new(
        config: AppConfig<'_>,
        backend: B,
        event_stream: E,
        snapshot_provider: S,
    ) -> anyhow::Result<Self> {
        setup_logging(config.log_level)?;

        let directory = validate_dir_or_default(config.directory)?;

        let (app, event_receiver) = App::new_with_receiver(
            directory,
            &config.search_field_values,
            &config.app_run_config,
        );

        let terminal = Terminal::new(backend)?;
        let tui = Tui::new(terminal);

        Ok(Self {
            app,
            event_receiver,
            tui,
            event_stream,
            snapshot_provider,
        })
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        self.tui.init()?;
        self.draw()?;

        Ok(())
    }

    pub fn draw(&mut self) -> anyhow::Result<()> {
        self.tui.draw(&mut self.app)?;
        self.snapshot_provider.send_snapshot(&self.tui);
        Ok(())
    }

    pub async fn run_event_loop(&mut self) -> anyhow::Result<Option<String>> {
        loop {
            let event_handling_result = tokio::select! {
                Some(Ok(event)) = self.event_stream.next() => {
                    match event {
                        CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            self.app.handle_key_event(&key)
                        },
                        CrosstermEvent::Resize(_, _) => EventHandlingResult::Rerender,
                        _ => EventHandlingResult::None,
                    }
                }
                Some(event) = self.event_receiver.recv() => {
                    match event {
                        Event::LaunchEditor((file_path, line)) => {
                            let mut res = EventHandlingResult::Rerender;
                            self.tui.show_cursor()?;
                            match self.open_editor(file_path, line) {
                                Ok(()) => {
                                    if self.app.config.editor_open.exit {
                                        res = EventHandlingResult::Exit(None);
                                    }
                                }
                                Err(e) => {
                                    self.app.add_error(
                                        AppError{
                                            name: "Failed to launch editor".to_string(),
                                            long: e.to_string(),
                                        },
                                    );
                                    error!("Failed to open editor: {e}");
                                }
                            }
                            self.tui.init()?;
                            res

                        }
                        Event::App(app_event) => {
                            self.app.handle_app_event(&app_event)
                        }
                    }
                }
                Some(event) = self.app.background_processing_recv() => {
                    self.app.handle_background_processing_event(event)
                }
                else => {
                    return Ok(None);
                }
            };

            match event_handling_result {
                EventHandlingResult::Rerender => self.draw()?,
                EventHandlingResult::Exit(results) => return Ok(results),
                EventHandlingResult::None => {}
            }
        }
    }

    pub fn cleanup(&mut self) -> anyhow::Result<()> {
        self.app.cancel_in_progress_tasks();
        self.tui.exit()
    }

    fn open_editor(&self, file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        match &self.app.config.editor_open.command {
            Some(command) => {
                Self::open_editor_from_command(command, &file_path, line)?;
            }
            None => {
                Self::open_default_editor(file_path, line)?;
            }
        }
        Ok(())
    }

    fn open_editor_from_command(
        editor_command: &str,
        file_path: &Path,
        line: usize,
    ) -> anyhow::Result<()> {
        let editor_command = editor_command
            .replace("%file", &file_path.to_string_lossy())
            .replace("%line", &line.to_string());

        let output = if cfg!(windows) {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(&editor_command);
            cmd.output()?
        } else {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(&editor_command);
            cmd.output()?
        };

        if output.status.success() {
            Ok(())
        } else {
            let status_code = output
                .status
                .code()
                .map_or("<not found>".to_owned(), |r| r.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "Failed to execute command\nStatus: {status_code}\nOutput: {stderr}",
            ))
        }
    }

    fn open_default_editor(file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        let editor = match env::var("EDITOR") {
            Ok(val) if !val.trim().is_empty() => val,
            _ => match env::var("VISUAL") {
                Ok(val) if !val.trim().is_empty() => val,
                _ => {
                    if cfg!(windows) {
                        "notepad".to_string()
                    } else {
                        "vi".to_string()
                    }
                }
            },
        };

        let parts: Vec<&str> = editor.split_whitespace().collect();
        let Some(program) = parts.first() else {
            return Err(anyhow::anyhow!("Found empty editor command"));
        };
        let mut cmd = Command::new(program);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }

        let editor_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
            .to_lowercase();
        match editor_name.as_str() {
            e if ["vi", "vim", "nvim", "kak", "nano"].contains(&e) => {
                cmd.arg(format!("+{line}")).arg(file_path);
            }
            e if ["hx", "helix", "subl", "sublime_text", "zed"].contains(&e) => {
                cmd.arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["code", "code-insiders", "codium", "vscodium"].contains(&e) => {
                cmd.arg("-g")
                    .arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["emacs", "emacsclient"].contains(&e) => {
                cmd.arg(format!("+{line}:0")).arg(file_path);
            }
            "notepad++" => {
                cmd.arg(file_path).arg(format!("-n{line}"));
            }
            _ => {
                cmd.arg(file_path);
            }
        }

        cmd.status()?;
        Ok(())
    }
}

pub async fn run_app_tui(app_config: AppConfig<'_>) -> anyhow::Result<()> {
    let mut runner = AppRunner::new_runner(app_config)?;
    runner.init()?;
    let results_to_print = runner.run_event_loop().await?;
    runner.cleanup()?;
    if let Some(results) = results_to_print {
        println!("{results}");
    }
    Ok(())
}

pub fn run_app_headless(app_config: AppConfig<'_>) -> anyhow::Result<()> {
    let mut error_handler = SimpleErrorHandler::new();
    let search_config = app_config.try_into()?;
    let result = validate_search_configuration(search_config, &mut error_handler)?;
    let searcher = match result {
        ValidationResult::Success(searcher) => searcher,
        ValidationResult::ValidationErrors => {
            bail!("{}", error_handler.errors_str().unwrap());
        }
    };

    let cancelled = Arc::new(AtomicBool::new(false));

    let (results_sender, results_receiver) = mpsc::channel::<Vec<SearchResult>>();

    let sender_clone = results_sender.clone();
    searcher.walk_files(&cancelled, move || {
        let sender = sender_clone.clone();
        Box::new(move |mut results| {
            if let Err(file_err) = scooter_core::replace_in_file(&mut results) {
                println!("Found error when performing replacement: {file_err}");
            }
            if sender.send(results).is_err() {
                // Channel closed, likely due to early termination
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    drop(results_sender);

    let all_results = results_receiver.into_iter().flatten();
    let stats = calculate_statistics(all_results);

    let results_output = format_replacement_results(stats.num_successes, None, &stats.errors);
    println!("{results_output}");

    Ok(())
}
