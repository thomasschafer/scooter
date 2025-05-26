use crossterm::event::{self, Event as CrosstermEvent};
use futures::Stream;
use futures::StreamExt;
use log::error;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
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
    pub include_hidden: bool,
    pub advanced_regex: bool,
    pub log_level: LevelFilter,
    pub search_field_values: SearchFieldValues<'a>,
    pub immediate_search: bool,
    pub immediate_replace: bool,
}

impl Default for AppConfig<'_> {
    fn default() -> Self {
        Self {
            directory: None,
            include_hidden: false,
            advanced_regex: false,
            log_level: LevelFilter::from_str(DEFAULT_LOG_LEVEL).unwrap(),
            search_field_values: SearchFieldValues::default(),
            immediate_search: false,
            immediate_replace: false,
        }
    }
}

pub trait EventStream:
    Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin
{
}
impl<T: Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin> EventStream for T {}

pub type CrosstermEventStream = event::EventStream;

pub struct AppRunner<B: Backend, E: EventStream> {
    app: App,
    event_receiver: UnboundedReceiver<Event>,
    tui: Tui<B>,
    event_stream: E,
    buffer_snapshot_sender: Option<UnboundedSender<String>>,
}

pub trait BufferProvider {
    fn get_buffer(&mut self) -> &Buffer;
}

impl BufferProvider for AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream> {
    fn get_buffer(&mut self) -> &Buffer {
        self.tui.terminal.current_buffer_mut()
    }
}

impl<E: EventStream> BufferProvider for AppRunner<TestBackend, E> {
    fn get_buffer(&mut self) -> &Buffer {
        self.tui.terminal.backend().buffer()
    }
}

impl AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream> {
    pub fn new_terminal(config: AppConfig<'_>) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        Self::new(config, backend, event_stream)
    }
}

impl<B: Backend + 'static, E: EventStream> AppRunner<B, E>
where
    Self: BufferProvider,
{
    pub fn new(config: AppConfig<'_>, backend: B, event_stream: E) -> anyhow::Result<Self> {
        setup_logging(config.log_level)?;

        let directory = match config.directory {
            None => None,
            Some(d) => Some(validate_directory(&d)?),
        };

        let (app, event_receiver) = App::new_with_receiver(
            directory,
            &config.search_field_values,
            &AppRunConfig {
                include_hidden: config.include_hidden,
                advanced_regex: config.advanced_regex,
                immediate_search: config.immediate_search,
                immediate_replace: config.immediate_replace,
            },
        );

        let terminal = Terminal::new(backend)?;
        let tui = Tui::new(terminal);

        Ok(Self {
            app,
            event_receiver,
            tui,
            event_stream,
            buffer_snapshot_sender: None,
        })
    }

    // Used only for testing
    #[allow(dead_code)]
    pub fn with_snapshot_channel(mut self, sender: UnboundedSender<String>) -> Self {
        self.buffer_snapshot_sender = Some(sender);
        self
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        self.tui.init()?;
        self.draw()?;

        Ok(())
    }

    pub fn draw(&mut self) -> anyhow::Result<()> {
        self.tui.draw(&mut self.app)?;
        self.send_snapshot();
        Ok(())
    }

    fn buffer_contents(&mut self) -> String {
        let buffer = self.get_buffer();
        buffer
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
            .join("")
    }

    fn send_snapshot(&mut self) {
        if self.buffer_snapshot_sender.is_none() {
            return;
        }

        let contents = self.buffer_contents();
        if let Some(sender) = &self.buffer_snapshot_sender {
            let _ = sender.send(contents);
        }
    }

    pub async fn run_event_loop(&mut self) -> anyhow::Result<()> {
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
                                        res = EventHandlingResult::Exit;
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
                    break;
                }
            };

            match event_handling_result {
                EventHandlingResult::Rerender => self.draw()?,
                EventHandlingResult::Exit => break,
                EventHandlingResult::None => {}
            }
        }

        Ok(())
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

pub async fn run_app(app_config: AppConfig<'_>) -> anyhow::Result<()> {
    let mut runner = AppRunner::new_terminal(app_config)?;
    runner.init()?;
    runner.run_event_loop().await?;
    runner.cleanup()?;
    Ok(())
}
