use crossterm::event::{self, Event as CrosstermEvent};
use futures::Stream;
use futures::StreamExt;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::utils::validate_directory;
use crate::{
    app::{App, AppEvent, EventHandlingResult},
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
    app_event_receiver: UnboundedReceiver<AppEvent>,
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

        let (app, app_event_receiver) =
            App::new_with_receiver(directory, config.hidden, config.advanced_regex);

        let terminal = Terminal::new(backend)?;
        let tui = Tui::new(terminal);

        Ok(Self {
            app,
            app_event_receiver,
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
            let EventHandlingResult { exit, rerender } = tokio::select! {
                Some(Ok(event)) = self.event_stream.next() => {
                    match event {
                        CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            self.app.handle_key_event(&key)?
                        },
                        CrosstermEvent::Resize(_, _) => EventHandlingResult {
                            exit: false,
                            rerender: true,
                        },
                        _ => EventHandlingResult {
                            exit: false,
                            rerender: false,
                        },
                    }
                }
                Some(event) = self.app_event_receiver.recv() => {
                    self.app.handle_app_event(event).await
                }
                Some(event) = self.app.background_processing_recv() => {
                    self.app.handle_background_processing_event(event)
                }
                else => {
                    break;
                }
            };

            if rerender {
                self.tui.draw(&mut self.app)?;
                self.send_snapshot();
            }

            if exit {
                break;
            }
        }

        Ok(())
    }

    pub fn cleanup(&mut self) -> anyhow::Result<()> {
        self.tui.exit()
    }
}

pub async fn run_app(config: AppConfig) -> anyhow::Result<()> {
    let mut runner = AppRunner::new_terminal(config)?;
    runner.init()?;
    runner.run_event_loop().await?;
    runner.cleanup()?;
    Ok(())
}
