use std::any::TypeId;
use std::io;
use std::panic;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};

use scooter_core::app::App;

use crate::ui;

#[derive(Debug)]
pub struct Tui<B: Backend> {
    pub terminal: Terminal<B>,
}

impl<B: Backend + 'static> Tui<B>
where
    B::Error: Send + Sync,
{
    pub fn new(terminal: Terminal<B>) -> Self {
        Self { terminal }
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        if TypeId::of::<B>() == TypeId::of::<TestBackend>() {
            return Ok(());
        }

        terminal::enable_raw_mode()?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;

        let panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic| {
            Self::reset().expect("failed to reset the terminal");
            panic_hook(panic);
        }));

        self.terminal.hide_cursor()?;
        self.terminal.clear()?;

        Ok(())
    }

    pub fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.terminal.draw(|frame| ui::view::render(app, frame))?;
        Ok(())
    }

    fn reset() -> anyhow::Result<()> {
        terminal::disable_raw_mode()?;
        crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        Ok(())
    }

    pub fn show_cursor(&mut self) -> anyhow::Result<()> {
        self.terminal.show_cursor()?;
        Ok(())
    }

    pub fn exit(&mut self) -> anyhow::Result<()> {
        Self::reset()?;
        self.show_cursor()?;
        Ok(())
    }
}
