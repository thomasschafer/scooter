use crossterm::{
    event::{self, Event as CrosstermEvent},
    style::Stylize as _,
};
use frep_core::search::SearchResultWithReplacement;
use futures::{Stream, StreamExt};
use log::{error, LevelFilter};
use ratatui::{
    backend::{Backend, CrosstermBackend, TestBackend},
    crossterm::event::KeyEventKind,
    Terminal,
};
use scooter_core::{
    app::{App, AppRunConfig, Event, EventHandlingResult, ExitState, InputSource},
    errors::AppError,
    fields::SearchFieldValues,
};
use std::{
    env, io,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    config::{load_config, Config},
    conversions,
    logging::DEFAULT_LOG_LEVEL,
    tui::Tui,
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig<'a> {
    pub directory: PathBuf,
    pub log_level: LevelFilter,
    pub search_field_values: SearchFieldValues<'a>,
    pub app_run_config: AppRunConfig,
    pub stdin_content: Option<String>,
}

impl Default for AppConfig<'_> {
    fn default() -> Self {
        Self {
            directory: env::current_dir().unwrap(),
            log_level: LevelFilter::from_str(DEFAULT_LOG_LEVEL).unwrap(),
            search_field_values: SearchFieldValues::default(),
            app_run_config: AppRunConfig::default(),
            stdin_content: None,
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
    config: Config,
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
                    "\n" // TODO: should this be `cell.symbol() + "\n"`?
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
    pub fn new_runner(config: &AppConfig<'_>) -> anyhow::Result<Self> {
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
        config: &AppConfig<'_>,
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
        app_config: &AppConfig<'_>,
        backend: B,
        event_stream: E,
        snapshot_provider: S,
    ) -> anyhow::Result<Self> {
        let config = load_config().expect("Failed to read config file");

        let input_source = if let Some(stdin_content) = &app_config.stdin_content {
            InputSource::Stdin(stdin_content.clone())
        } else {
            InputSource::Directory(app_config.directory.clone())
        };

        let (app, event_receiver) = App::new_with_receiver(
            input_source,
            &app_config.search_field_values,
            &app_config.app_run_config,
            config.search.disable_prepopulated_fields,
        );

        let terminal = Terminal::new(backend)?;
        let tui = Tui::new(terminal);

        Ok(Self {
            app,
            config,
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
        self.tui.draw(&mut self.app, &self.config)?;
        self.snapshot_provider.send_snapshot(&self.tui);
        Ok(())
    }

    pub async fn run_event_loop(&mut self) -> anyhow::Result<Option<ExitState>> {
        loop {
            let event_handling_result = tokio::select! {
                Some(Ok(event)) = self.event_stream.next() => {
                    match event {
                        CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            if let Some((code, modifiers)) = conversions::convert_key_event(&key) {
                                self.app.handle_key_event(code, modifiers)
                            } else {
                                EventHandlingResult::None
                            }
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
                                    if self.config.editor_open.exit {
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
                        Event::PerformReplacement => {
                            self.app.perform_replacement();
                            EventHandlingResult::Rerender
                        }
                        Event::ExitAndReplace(state) => {
                            return Ok(Some(ExitState{stats: None, stdout_state: Some(state)})); // TODO: implement print_results
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
                EventHandlingResult::Exit(results) => return Ok(results.map(|t| *t)),
                EventHandlingResult::None => {}
            }
        }
    }

    pub fn cleanup(&mut self) -> anyhow::Result<()> {
        self.app.cancel_in_progress_tasks();
        self.tui.exit()
    }

    fn open_editor(&self, file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        match &self.config.editor_open.command {
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

pub fn format_replacement_results(
    num_successes: usize,
    num_ignored: Option<usize>,
    errors: Option<&[SearchResultWithReplacement]>,
) -> String {
    let errors_display = if let Some(errors) = errors {
        #[allow(clippy::format_collect)]
        errors
            .iter()
            .map(|error| {
                let (path, error) = error.display_error();
                format!("\n{path}:\n  {}", error.red())
            })
            .collect::<String>()
    } else {
        String::new()
    };

    let maybe_ignored_str = match num_ignored {
        Some(n) => format!("\nIgnored (lines): {n}"),
        None => "".into(),
    };
    let maybe_errors_str = match errors {
        Some(errors) => format!(
            "\nErrors: {num_errors}{errors_display}",
            num_errors = errors.len()
        ),
        None => "".into(),
    };

    format!(
        "Successful replacements (lines): {num_successes}{maybe_ignored_str}{maybe_errors_str}\n"
    )
}

pub async fn run_app_tui(app_config: &AppConfig<'_>) -> anyhow::Result<Option<String>> {
    // TODO: handle stdin
    let mut runner = AppRunner::new_runner(app_config)?;
    runner.init()?;
    let maybe_replace_state = runner.run_event_loop().await?;
    runner.cleanup()?;
    let maybe_stats = maybe_replace_state
        .as_ref()
        .and_then(|replace_state| replace_state.stats.as_ref())
        .map(|stats| {
            format_replacement_results(
                stats.num_successes,
                Some(stats.num_ignored),
                Some(&stats.errors),
            )
        });
    if let Some(stdout_state) = maybe_replace_state.and_then(|state| state.stdout_state) {
        for line in stdout_state.stdin.lines() {
            if let Some(res) = frep_core::replace::replacement_if_match(
                line,
                &stdout_state.search_config.search,
                &stdout_state.search_config.replace,
            ) {
                println!("{res}");
            } else {
                println!("{line}");
            }
        }
    }

    Ok(maybe_stats)
}

#[cfg(test)]
mod tests {
    use frep_core::{line_reader::LineEnding, replace::ReplaceResult, search::SearchResult};

    use super::*;

    #[test]
    fn test_format_replacement_results_no_errors() {
        let result = format_replacement_results(5, Some(2), Some(&[]));
        assert_eq!(
            result,
            "Successful replacements (lines): 5\nIgnored (lines): 2\nErrors: 0\n"
        );
    }

    #[test]
    fn test_format_replacement_results_with_errors() {
        let error_result = SearchResultWithReplacement {
            search_result: SearchResult {
                path: PathBuf::from("file.txt"),
                line_number: 10,
                line: "line".to_string(),
                line_ending: LineEnding::Lf,
                included: true,
            },
            replacement: "replacement".to_string(),
            replace_result: Some(ReplaceResult::Error("Test error".to_string())),
        };

        let result = format_replacement_results(3, Some(1), Some(&[error_result]));
        assert!(result.contains("Successful replacements (lines): 3\n"));
        assert!(result.contains("Ignored (lines): 1"));
        assert!(result.contains("Errors: 1"));
        assert!(result.contains("file.txt:10"));
        assert!(result.contains("Test error"));
    }

    #[test]
    fn test_format_replacement_results_no_ignored_count() {
        let result = format_replacement_results(7, None, Some(&[]));
        assert_eq!(result, "Successful replacements (lines): 7\nErrors: 0\n");
        assert!(!result.contains("Ignored (lines):"));
    }
}
