mod agent;
mod app;
mod launcher;
mod models;
mod store;
mod tools;
mod workflows;

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic;

use crate::app::App;
use crate::launcher::{resolve_checkout_path, ResolveOutcome};
use crate::store::{Paths, Settings, State};

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::from_env().context("resolve config paths")?;
    // `ensure_dirs` only creates the agenthd root (settings + state
    // parents) and the output target trees. It deliberately does NOT
    // create `canonical_dir`: before the user configures a checkout
    // there is no local canonical directory to back.
    paths.ensure_dirs().context("ensure config directories")?;

    // Resolve the configured checkout path. The CLI override and the
    // persisted `settings.json` both win over the first-run state,
    // and the result becomes the canonical directory for the rest
    // of the run.
    let (checkout_path, override_applied, stale_banner) =
        match resolve_checkout_path(&paths, &args)? {
            ResolveOutcome::Ready {
                path,
                override_applied,
            } => (Some(path), override_applied, None),
            ResolveOutcome::StaleCheckout { banner } => (None, false, Some(banner)),
            ResolveOutcome::FirstRun => (None, false, None),
        };

    // Three branches that need their own TUI boot: first-run (no
    // settings yet), stale-checkout recovery (the persisted path is
    // unusable), and the normal path (we have a checkout to load).
    if checkout_path.is_none() {
        // First run, or the configured checkout moved: open the
        // Settings screen gated so the user must pick a path. The
        // recovery banner is shown above the editor when one is
        // available so the failure is visible — the runtime never
        // silently falls back to a cwd ancestor walk and never
        // deletes any targets. `ensure_dirs` has already created
        // the agenthd root and output targets; `canonical_dir`
        // stays untouched until the user picks a checkout.
        let guard = TerminalGuard::new().context("initialize terminal")?;
        let panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let mut out = io::stdout();
            let _ = execute!(out, LeaveAlternateScreen);
            let _ = execute!(out, crossterm::cursor::Show);
            panic_hook(info);
        }));
        let result = {
            let backend = CrosstermBackend::new(io::stdout());
            let mut terminal = ratatui::Terminal::new(backend).context("create terminal")?;
            // Load any pre-existing ownership manifest before opening
            // the gated Settings screen: state.json (per-target
            // installed hashes) is independent of settings.json
            // and must survive across first-run / recovery setup
            // so a configured-but-uninitialized user does not
            // silently lose their prior sync runs. State::load
            // returns `default()` on not-found; only a malformed
            // manifest surfaces an error. Unknown legacy fields
            // (e.g. `plugin_hash` from older builds) are ignored
            // by serde, so the load is nonfatal.
            let state = State::load(&paths.state_file).context("load ownership state")?;
            let mut app = App::new(paths, state);
            let is_recovery = stale_banner.is_some();
            app.open_settings_with_error(true, stale_banner);
            if is_recovery {
                app.set_status(
                    "configured checkout is unusable; type a new path or relaunch with --repo",
                );
            } else {
                app.set_status("first run — pick a checkout to continue");
            }
            app.run(&mut terminal)
        };
        drop(guard);
        return result;
    }
    let checkout_path = checkout_path.expect("checked above");
    // Re-point canonical_dir at the configured checkout. A failure
    // here means the checkout passed the validation above but the
    // path resolution itself errored — extremely unlikely, but
    // surfaced explicitly rather than papering over.
    let settings = Settings::new(checkout_path.to_string_lossy().into_owned());
    let paths = paths.with_settings(&settings).context("apply settings")?;

    let state = State::load(&paths.state_file).context("load ownership state")?;

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
        let mut app = App::new(paths, state);
        if override_applied {
            app.set_status(format!("saved checkout path: {}", checkout_path.display()));
        }
        app.run(&mut terminal)
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
