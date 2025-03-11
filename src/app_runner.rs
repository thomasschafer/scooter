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
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::utils::validate_directory;
use crate::{
    app::{App, Event, EventHandlingResult},
    logging::setup_logging,
    tui::Tui,
};

pub struct AppConfig {
    pub directory: Option<String>,
    pub hidden: bool,
    pub advanced_regex: bool,
    pub log_level: LevelFilter,
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
    pub fn new_terminal(config: AppConfig) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        Self::new(config, backend, event_stream)
    }
}

impl<B: Backend + 'static, E: EventStream> AppRunner<B, E>
where
    Self: BufferProvider,
{
    pub fn new(config: AppConfig, backend: B, event_stream: E) -> anyhow::Result<Self> {
        setup_logging(config.log_level)?;

        let directory = match config.directory {
            None => None,
            Some(d) => Some(validate_directory(&d)?),
        };

        let (app, event_receiver) =
            App::new_with_receiver(directory, config.hidden, config.advanced_regex);

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
                            self.app.handle_key_event(&key)?
                        },
                        CrosstermEvent::Resize(_, _) => EventHandlingResult::Rerender,
                        _ => EventHandlingResult::None,
                    }
                }
                Some(event) = self.event_receiver.recv() => {
                    match event {
                        Event::LaunchEditor((file_path, line)) => {
                                if let Err(e) = self.open_editor(file_path, line) {
                                    // TODO(editor): show error in popup
                                    error!("Failed to open editor: {e}");
                                };
                                self.tui.init().expect("Failed to initialise TUI");
                            EventHandlingResult::Rerender
                        }
                        Event::App(app_event) => {
                            self.app.handle_app_event(app_event).await
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
        self.tui.exit()
    }

    fn open_editor(&self, file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        let editor = env::var("EDITOR").unwrap_or_else(|_| {
            env::var("VISUAL").unwrap_or_else(|_| {
                if cfg!(windows) {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            })
        });
        let parts: Vec<&str> = editor.split_whitespace().collect();
        let program = parts[0];
        let editor_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
            .to_lowercase();

        let mut cmd = Command::new(program);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }

        // TODO(editor): let users override editor
        // TODO(editor): TEST THESE
        match editor_name.as_str() {
            e if ["vi", "vim", "nvim", "nano"].contains(&e) => {
                cmd.arg(format!("+{}", line)).arg(file_path);
            }
            e if ["hx", "helix", "subl", "sublime_text", "zed"].contains(&e) => {
                cmd.arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["code", "code-insiders", "codium", "vscodium"].contains(&e) => {
                cmd.arg("-g")
                    .arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["emacs", "emacsclient"].contains(&e) => {
                cmd.arg(format!("+{}:0", line)).arg(file_path);
            }
            e if ["kak", "micro"].contains(&e) => {
                cmd.arg(file_path).arg(format!("+{}", line));
            }
            "notepad++" => {
                cmd.arg(file_path).arg(format!("-n{}", line));
            }
            _ => {
                cmd.arg(file_path);
            }
        }

        cmd.status()?;
        Ok(())
    }
}

pub async fn run_app(config: AppConfig) -> anyhow::Result<()> {
    let mut runner = AppRunner::new_terminal(config)?;
    runner.init()?;
    runner.run_event_loop().await?;
    runner.cleanup()?;
    Ok(())
}
