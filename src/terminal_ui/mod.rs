mod actions;
mod app;
mod input;
mod model;
mod operations;
mod render;

use std::io::{self, Stdout};

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};

use crate::error::AppError;
use crate::logging::AuditLogger;

pub use app::TerminalApp;
pub use model::TerminalUiOptions;

use actions::run_ui_operation;
use render::render_app;

pub async fn run_terminal_ui(
    options: TerminalUiOptions,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    let mut guard = TerminalGuard::enter()
        .map_err(|err| AppError::Internal(format!("terminal setup failed: {err}")))?;
    let result = run_app(guard.terminal_mut(), TerminalApp::new(options), logger).await;
    guard
        .restore()
        .map_err(|err| AppError::Internal(format!("terminal cleanup failed: {err}")))?;
    result
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: TerminalApp,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    loop {
        terminal
            .draw(|frame| render_app(frame, &app))
            .map_err(|err| AppError::Internal(format!("terminal draw failed: {err}")))?;
        if app.should_quit {
            return Ok(());
        }
        if let Some(operation) = app.pending_operation.take() {
            run_ui_operation(terminal, &mut app, operation, logger).await;
            continue;
        }
        if let Event::Key(key) = event::read()
            .map_err(|err| AppError::Internal(format!("terminal input failed: {err}")))?
        {
            app.handle_key(key);
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut setup = TerminalSetupGuard {
            raw_enabled: true,
            alternate_screen: false,
            cursor_hidden: false,
        };
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        setup.alternate_screen = true;
        execute!(stdout, crossterm::cursor::Hide)?;
        setup.cursor_hidden = true;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        setup.disarm();
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let mut first_error = None;
        if let Err(err) = execute!(self.terminal.backend_mut(), crossterm::cursor::Show) {
            first_error = Some(err);
        }
        if let Err(err) = execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            && first_error.is_none()
        {
            first_error = Some(err);
        }
        if let Err(err) = disable_raw_mode()
            && first_error.is_none()
        {
            first_error = Some(err);
        }
        first_error.map_or(Ok(()), Err)
    }
}

struct TerminalSetupGuard {
    raw_enabled: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
}

impl TerminalSetupGuard {
    fn disarm(&mut self) {
        self.raw_enabled = false;
        self.alternate_screen = false;
        self.cursor_hidden = false;
    }
}

impl Drop for TerminalSetupGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.cursor_hidden {
            let _ = execute!(stdout, crossterm::cursor::Show);
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        if self.raw_enabled {
            let _ = disable_raw_mode();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn should_launch_interactive(command_present: bool, stdin_tty: bool, stdout_tty: bool) -> bool {
    !command_present && stdin_tty && stdout_tty
}

pub fn no_terminal_message() -> &'static str {
    "Interactive menu requires a terminal.\n\nUse:\n  ./to-digi --help\n  ./to-digi import ...\n"
}

#[cfg(test)]
mod tests;
