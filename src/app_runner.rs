use crossterm::event::{self, Event as CrosstermEvent};
use futures::Stream;
use futures::StreamExt;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::pin::Pin;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::utils::validate_directory;
use crate::{
    app::{App, AppEvent, EventHandlingResult},
    logging::setup_logging,
    tui::Tui,
};

pub trait EventStream:
    Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin
{
}

pub struct CrosstermEventStream(event::EventStream);

impl CrosstermEventStream {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(event::EventStream::new())
    }
}

impl Stream for CrosstermEventStream {
    type Item = Result<CrosstermEvent, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx)
    }
}

impl EventStream for CrosstermEventStream {}

pub struct AppConfig {
    pub directory: Option<String>,
    pub hidden: bool,
    pub advanced_regex: bool,
    pub log_level: LevelFilter,
}

pub struct AppRunner<B: Backend, E: EventStream> {
    pub app: App,
    app_event_receiver: UnboundedReceiver<AppEvent>,
    pub tui: Tui<B>,
    pub event_stream: E,

    buffer_snapshot_sender: Option<UnboundedSender<String>>,
}

impl AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream> {
    pub fn new_terminal(config: AppConfig) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        Self::new(config, backend, event_stream)
    }
}

impl<B: Backend + 'static, E: EventStream> AppRunner<B, E> {
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

        if self.buffer_snapshot_sender.is_some() {
            self.send_snapshot();
        }

        Ok(())
    }

    fn send_snapshot(&mut self) {
        let contents = self.buffer_contents();
        if let Some(sender) = &self.buffer_snapshot_sender {
            let _ = sender.send(contents);
        }
    }

    fn buffer_contents(&mut self) -> String {
        let buffer = self.tui.terminal.current_buffer_mut();
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
                Some(app_event) = self.app_event_receiver.recv() => {
                    self.app.handle_app_event(app_event).await
                }
                Some(event) = self.app.background_processing_recv() => {
                    self.app.handle_background_processing_event(event)
                }
                else => break,
            };

            if rerender {
                self.tui.draw(&mut self.app)?;

                if self.buffer_snapshot_sender.is_some() {
                    self.send_snapshot();
                }
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
