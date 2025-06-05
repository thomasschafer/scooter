use crossterm::event::{self, Event as CrosstermEvent};
use fancy_regex::Regex as FancyRegex;
use futures::Stream;
use futures::StreamExt;
use ignore::overrides::Override;
use ignore::overrides::OverrideBuilder;
use ignore::WalkState;
use log::error;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use regex::Regex;
use scooter_core::search::FileSearcher;
use scooter_core::search::SearchType;
use std::env;
use std::env::current_dir;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::logging::DEFAULT_LOG_LEVEL;
use crate::utils;
use crate::{
    app::{App, AppError, AppRunConfig, Event, EventHandlingResult, ValidatedField},
    fields::SearchFieldValues,
    logging::setup_logging,
    tui::Tui,
    utils::validate_dir_or_default,
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig<'a> {
    pub directory: Option<&'a str>,
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
            &directory,
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

fn parse_search_text(
    search_text: &str,
    fixed_strings: bool,
    advanced_regex: bool,
) -> anyhow::Result<SearchType> {
    let result = if fixed_strings {
        SearchType::Fixed(search_text.to_string())
    } else if advanced_regex {
        SearchType::PatternAdvanced(FancyRegex::new(search_text)?)
    } else {
        SearchType::Pattern(Regex::new(search_text)?)
    };
    Ok(result)
}

fn parse_overrides(
    dir: &Path,
    include_globs: &str,
    exclude_globs: &str,
) -> anyhow::Result<ValidatedField<Override>> {
    let mut overrides = OverrideBuilder::new(dir);
    let mut success = true;

    let include_res = utils::add_overrides(&mut overrides, include_globs, "");
    if let Err(e) = include_res {
        // TODO(no-tui): log error
        success = false;
    }

    let exlude_res = utils::add_overrides(&mut overrides, exclude_globs, "!");
    if let Err(e) = exlude_res {
        // TODO(no-tui): log error
        success = false;
    }

    if success {
        let overrides = overrides.build()?;
        Ok(ValidatedField::Parsed(overrides))
    } else {
        Ok(ValidatedField::Error)
    }
}

pub async fn run_app_headless(app_config: AppConfig<'_>) -> anyhow::Result<()> {
    let search_pattern = match parse_search_text(
        &app_config.search_field_values.search.value,
        app_config.search_field_values.fixed_strings.value,
        app_config.app_run_config.advanced_regex,
    ) {
        Ok(p) => ValidatedField::Parsed(p),
        Err(e) => {
            if utils::is_regex_error(&e) {
                // TODO(no-tui): exit and log error
                ValidatedField::Error
            } else {
                return Err(e);
            }
        }
    };

    let directory = validate_dir_or_default(app_config.directory)?;

    let overrides = parse_overrides(
        &directory,
        app_config.search_field_values.include_files.value,
        app_config.search_field_values.exclude_files.value,
    )?;

    let searcher =
        if let (ValidatedField::Parsed(search_pattern), ValidatedField::Parsed(overrides)) =
            (search_pattern, overrides)
        {
            FileSearcher::new(
                search_pattern,
                app_config.search_field_values.replace.value.to_owned(),
                app_config.search_field_values.match_whole_word.value,
                app_config.search_field_values.match_case.value,
                overrides,
                directory,
                app_config.app_run_config.include_hidden,
            )
        } else {
            // TODO(no-tui): log error
            todo!()
        };

    let cancelled = Arc::new(AtomicBool::new(false));
    searcher.walk_files(&cancelled, || {
        Box::new(move |results| {
            // TODO(no-tui): Replace in file
            WalkState::Continue
        })
    });

    // TODO(no-tui): log out results

    todo!()
}
