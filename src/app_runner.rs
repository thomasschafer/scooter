use crossterm::event::{self, Event as CrosstermEvent};
use futures::StreamExt;
use log::LevelFilter;
use ratatui::backend::Backend;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use tokio::sync::mpsc::UnboundedReceiver;

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

pub struct AppRunner<B: Backend> {
    pub app: App,
    app_event_receiver: UnboundedReceiver<AppEvent>,
    pub tui: Tui<B>,
}

impl AppRunner<CrosstermBackend<io::Stdout>> {
    pub fn new_terminal(config: AppConfig) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        Self::new(config, backend)
    }
}

impl<B: Backend + 'static> AppRunner<B> {
    pub fn new(config: AppConfig, backend: B) -> anyhow::Result<Self> {
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
        })
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        self.tui.init()?;
        self.tui.draw(&mut self.app)?;
        Ok(())
    }

    pub async fn run_event_loop(&mut self) -> anyhow::Result<()> {
        let mut reader = event::EventStream::new();

        loop {
            let EventHandlingResult { exit, rerender } = tokio::select! {
                Some(Ok(event)) = reader.next() => {
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
