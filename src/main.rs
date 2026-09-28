mod agent;
mod app;
mod models;
mod store;
mod tools;

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic;
use std::path::PathBuf;

use crate::app::App;
use crate::store::{load_settings, save_settings, validate_checkout_path, Paths, Settings, State};

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

/// Decide which checkout path the binary should run with this
/// invocation. Order of precedence:
///
/// 1. `agenthd --repo <absolute path>`. The override is validated up
///    front so the user is told *now* if the configured checkout is
///    unusable rather than getting a half-started TUI. The override
///    is persisted to `settings.json` so subsequent launches agree
///    with the user.
/// 2. The persisted `settings.json`. Loaded fresh on every launch so
///    the user can re-point the binary through the Settings screen
///    or by editing the file directly.
/// 3. None — first-run interactive flow.
///
/// When the persisted `settings.json` points at a checkout that has
/// moved or otherwise fails validation, the runtime does NOT silently
/// fall back to a cwd ancestor walk (no silent inference), nor does
/// it delete any targets. Instead it reports `StaleCheckout` so `run`
/// can open the Settings screen gated with the validation error
/// visible; `--repo` may repair the configuration before the TUI
/// starts, otherwise the user picks a new path inside the TUI.
fn resolve_checkout_path(paths: &Paths, args: &[String]) -> Result<ResolveOutcome> {
    if let Some(path) = parse_repo_override(args)? {
        validate_checkout_path(&path)
            .with_context(|| format!("--repo path `{}` is unusable", path.display()))?;
        let settings = Settings::new(path.to_string_lossy().into_owned());
        let existing = load_settings(&paths.settings_file).context("read settings")?;
        if existing.as_ref() != Some(&settings) {
            save_settings(&paths.settings_file, &settings).context("write settings")?;
        }
        return Ok(ResolveOutcome::Ready {
            path,
            override_applied: true,
        });
    }
    if let Some(settings) = load_settings(&paths.settings_file).context("read settings")? {
        let path = PathBuf::from(&settings.checkout_path);
        match validate_checkout_path(&path) {
            Ok(()) => {
                return Ok(ResolveOutcome::Ready {
                    path,
                    override_applied: false,
                })
            }
            Err(err) => {
                // The configured checkout is unusable. Surface the
                // failure in a gated Settings screen rather than
                // aborting the process or silently inferring a path
                // from the current working directory. The user sees
                // the validation error and can either type a new
                // path in the TUI or relaunch with `--repo` to
                // repair it.
                let banner = format!(
                    "configured checkout `{}` is unusable: {err}; \
                     type a new path or relaunch with `--repo <path>`",
                    path.display()
                );
                return Ok(ResolveOutcome::StaleCheckout { banner });
            }
        }
    }
    Ok(ResolveOutcome::FirstRun)
}

/// Outcome of `resolve_checkout_path`: the runtime may either know
/// the configured checkout (`Ready`), discover that the persisted
/// settings point at a checkout that is no longer valid
/// (`StaleCheckout`), or be running for the first time with no
/// settings at all (`FirstRun`).
enum ResolveOutcome {
    Ready {
        path: PathBuf,
        override_applied: bool,
    },
    StaleCheckout {
        banner: String,
    },
    FirstRun,
}

/// Parse the optional `agenthd --repo <absolute path>` flag.
/// Positional args are ignored.
fn parse_repo_override(args: &[String]) -> Result<Option<PathBuf>> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--repo" {
            let value = args
                .get(i + 1)
                .ok_or_else(|| anyhow::anyhow!("--repo requires an absolute path argument"))?;
            let candidate = PathBuf::from(value);
            if !candidate.is_absolute() {
                anyhow::bail!(
                    "--repo path `{}` must be an absolute path",
                    candidate.display()
                );
            }
            return Ok(Some(candidate));
        }
        i += 1;
    }
    Ok(None)
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
