use crossterm::event::{self, Event as CrosstermEvent};
use futures::Stream;
use futures::StreamExt;
use log::error;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::env;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::logging::DEFAULT_LOG_LEVEL;
use crate::{
    app::{App, AppError, AppRunConfig, Event, EventHandlingResult},
    fields::SearchFieldValues,
    logging::setup_logging,
    tui::Tui,
    utils::validate_directory,
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

pub trait EventStream:
    Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin
{
}
impl<T: Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin> EventStream for T {}

pub type CrosstermEventStream = event::EventStream;

pub struct AppRunner<B: Backend, E: EventStream, S: SnapshotProvider<B>> {
    app: App,
    event_receiver: UnboundedReceiver<Event>,
    tui: Option<Tui<B>>,
    event_stream: E,
    snapshot_provider: S,
}

pub trait SnapshotProvider<B: Backend> {
    fn send_snapshot(&self, tui: Option<&Tui<B>>);
}

pub struct NoOpSnapshotProvider;

impl<B: Backend> SnapshotProvider<B> for NoOpSnapshotProvider {
    #[inline]
    fn send_snapshot(&self, _tui: Option<&Tui<B>>) {
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
    fn send_snapshot(&self, tui: Option<&Tui<TestBackend>>) {
        if let Some(tui) = tui {
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
}

impl AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream, NoOpSnapshotProvider> {
    pub fn new_runner(config: AppConfig<'_>, tui: bool) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        let snapshot_provider = NoOpSnapshotProvider;
        Self::new(config, backend, event_stream, tui, snapshot_provider)
    }
}

impl<E: EventStream> AppRunner<TestBackend, E, TestSnapshotProvider> {
    // Used in integration tests
    #[allow(dead_code)]
    pub fn new_test_with_snapshot(
        config: AppConfig<'_>,
        backend: TestBackend,
        event_stream: E,
        use_tui: bool,
        snapshot_sender: UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        let snapshot_provider = TestSnapshotProvider::new(snapshot_sender);
        Self::new(config, backend, event_stream, use_tui, snapshot_provider)
    }
}

impl<B: Backend + 'static, E: EventStream, S: SnapshotProvider<B>> AppRunner<B, E, S> {
    pub fn new(
        config: AppConfig<'_>,
        backend: B,
        event_stream: E,
        use_tui: bool,
        snapshot_provider: S,
    ) -> anyhow::Result<Self> {
        setup_logging(config.log_level)?;

        let directory = match config.directory {
            None => None,
            Some(d) => Some(validate_directory(&d)?),
        };

        let (app, event_receiver) = App::new_with_receiver(
            directory,
            &config.search_field_values,
            &config.app_run_config,
        );

        let terminal = Terminal::new(backend)?;
        let tui = if use_tui {
            Some(Tui::new(terminal))
        } else {
            None
        };

        Ok(Self {
            app,
            event_receiver,
            tui,
            event_stream,
            snapshot_provider,
        })
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        if let Some(ref mut tui) = self.tui {
            tui.init()?;
        }
        self.draw()?;

        Ok(())
    }

    pub fn draw(&mut self) -> anyhow::Result<()> {
        if let Some(ref mut tui) = self.tui {
            tui.draw(&mut self.app)?;
        }

        self.snapshot_provider.send_snapshot(self.tui.as_ref());

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
                            if let Some(ref mut tui) = self.tui {
                                tui.show_cursor()?;
                            }
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
                            if let Some(ref mut tui) = self.tui {
                                tui.init()?;
                            }
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
        if let Some(ref mut tui) = self.tui {
            tui.exit()
        } else {
            Ok(())
        }
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

pub async fn run_app(app_config: AppConfig<'_>, tui: bool) -> anyhow::Result<()> {
    let mut runner = AppRunner::new_runner(app_config, tui)?;
    runner.init()?;
    let results_to_print = runner.run_event_loop().await?;
    runner.cleanup()?;
    if let Some(results) = results_to_print {
        println!("{results}");
    }
    Ok(())
}
