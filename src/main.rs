mod agent;
mod app;
mod models;
mod store;

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic;

use crate::app::App;
use crate::store::{migrate_legacy_agenthd, seed_starters, Paths, State};

struct TerminalGuard {
    armed: bool,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen).context("enter alternate screen")?;
        Ok(TerminalGuard { armed: true })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.armed {
            // Best-effort terminal restoration even on panic.
            let _ = disable_raw_mode();
            let mut out = io::stdout();
            let _ = execute!(out, LeaveAlternateScreen);
            let _ = execute!(out, crossterm::cursor::Show);
        }
    }
}

fn run() -> Result<()> {
    let paths = Paths::from_env().context("resolve config paths")?;
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty());
        match migrate_legacy_agenthd(
            std::path::Path::new(&home),
            xdg.as_deref().map(std::path::Path::new),
        ) {
            Ok(true) => {
                let from = xdg
                    .map(|v| format!("$XDG_CONFIG_HOME/agenthd ({})", v.to_string_lossy()))
                    .unwrap_or_else(|| format!("{}/.config/agenthd", home.to_string_lossy()));
                eprintln!(
                    "agenthd: migrated legacy config from {} to {}",
                    from,
                    paths.agenthd_root.display()
                );
            }
            Ok(false) => {}
            Err(e) => eprintln!("agenthd: legacy config migration skipped: {}", e),
        }
    }
    paths.ensure_dirs().context("ensure config directories")?;

    let state = State::load(&paths.state_file).context("load ownership state")?;
    let (_, state) = seed_starters(&paths, state).context("seed starter agents")?;

    let guard = TerminalGuard::new().context("initialize terminal")?;
    let panic_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Restore terminal before the default hook prints the panic message.
        let _ = disable_raw_mode();
        let mut out = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = execute!(out, crossterm::cursor::Show);
        panic_hook(info);
    }));

    let result = {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = ratatui::Terminal::new(backend).context("create terminal")?;
        App::new(paths, state).run(&mut terminal)
    };
    drop(guard);
    result
}

fn main() {
    if let Err(err) = run() {
        eprintln!("agenthd: {}", err);
        std::process::exit(1);
    }
}
