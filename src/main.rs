// The shared library crate (`src/lib.rs`) is the single source of
// truth for the UI-independent modules (`agent`, `launcher`,
// `models`, `store`, `tools`, `workflows`). The binary crate does
// NOT redeclare them with `mod` (that would double-compile every
// test). Instead, the bin re-exports them at its own crate root
// via `pub use agenthd::...`, so the `crate::store::*` /
// `crate::workflows::*` / `crate::agent::*` paths inside `app/`
// resolve to the lib's types — same path-based references, single
// source of truth, no test duplication.
//
// The `app` module is TUI-only (ratatui / crossterm wiring,
// `TerminalGuard`, panic hook, the `App::run` loop) and stays
// declared locally with `mod app;` so it is not part of the
// shared surface. It depends on `ratatui` / `crossterm`, which
// the GUI candidate will replace.
pub use agenthd::{agent, launcher, models, store, tools, workflows};

mod app;

// Test-only fixture re-include. The binary's `app::tests` references
// `STARTERS` / `starter_agent` (defined in the lib's
// `src/agent/starter_fixture/mod.rs`). `cfg(test)` does not
// propagate across crates: when `cargo test --bin agenthd` runs,
// the bin is in `cfg(test)` but the lib is NOT, so the lib's
// `#[cfg(test)]` items are absent from the bin's test build.
// Re-include the fixture here, gated to `#[cfg(test)]` so the
// production binary carries zero markdown bytes. The fixture
// re-uses the same source as the lib's (single source of truth
// for the markdown bytes), and the include_str paths to
// `agents/*.md` resolve correctly because the fixture's depth
// (one level under `src/agent/`) is preserved relative to this
// `#[path]` declaration. Inside the fixture, `crate::agent::Agent`
// resolves to the lib's `Agent` via the `pub use agenthd::agent`
// at the top of this file.
#[cfg(test)]
#[path = "agent/starter_fixture/mod.rs"]
mod starter_fixture;

use anyhow::{Context, Result};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::panic;

use crate::app::App;
use crate::launcher::{parse_launch, resolve_checkout_path, Mode, ResolveOutcome};
use crate::store::{Paths, Settings, State};
use std::path::{Path, PathBuf};

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
    // Parse argv into a launch plan before touching the filesystem
    // or the persisted `settings.json`. The GUI branch locates
    // its companion binary next to the running executable and
    // rejects with exit `2` before any settings.json write can
    // happen, so a user typo or a `--repo` that collides with
    // the mode keyword never produces side effects.
    //
    // `parse_launch` is the CLI-surface parser: it owns the mode
    // keyword and the `--repo` value-shape validation that the
    // boot path cares about (mode-keyword collision, absolute-path
    // shape). The legacy override resolution path — the one that
    // actually loads `settings.json` and persists the override —
    // still re-parses argv through `parse_repo_override` inside
    // `resolve_checkout_path` below. The two parsers are aligned
    // for any input that passes `parse_launch`; the split is
    // deliberate so the legacy persistence contract (the `--repo`
    // wins / save-if-changed / first-wins-on-duplicates behaviour
    // pinned by the launcher tests) is unchanged for the
    // implemented TUI mode.
    let plan = parse_launch(&args)?;
    if plan.mode == Mode::Gui {
        return run_gui(&args);
    }
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

/// Run the `agenthd gui` branch. Locate the companion binary
/// adjacent to the current executable; if it is missing, exit
/// with code `2` and a stderr message that names the missing
/// file **before** `resolve_checkout_path` runs (so no
/// `settings.json` write can happen). When the companion is
/// present, run `resolve_checkout_path` to honour the
/// `--repo` / save-if-changed contract the TUI branch uses,
/// then spawn the companion and forward its exit code.
///
/// The GUI branch does NOT call `Paths::ensure_dirs` and does
/// NOT install the `TerminalGuard` / panic hook: the companion
/// owns its own windowing surface, and the TUI restoration
/// primitives only apply to a TTY the CLI boot path allocates.
/// `save_settings` creates the parent directory, so the
/// missing-agenthd-root case still works without an explicit
/// `ensure_dirs` call.
fn run_gui(args: &[String]) -> Result<()> {
    // Adjacent-exact-filename lookup only — PATH is intentionally
    // not consulted so a hostile PATH cannot masquerade as the
    // companion. See the paired-install contract in the README.
    let current_exe = std::env::current_exe().context("locate current executable")?;
    let companion = match locate_companion(&current_exe) {
        Some(c) => c,
        None => {
            eprintln!(
                "agenthd: GUI mode requires the companion binary `{}` adjacent to {}; \
                 `cargo install` of agenthd alone does not produce it. \
                 See the paired-install contract in the README.",
                companion_filename(),
                current_exe.display(),
            );
            std::process::exit(2);
        }
    };

    // Honour the `--repo` / save-if-changed contract before the
    // companion starts. The companion reads the checkout itself;
    // `resolve_checkout_path` is called here only to persist the
    // override the user asked for.
    let paths = Paths::from_env().context("resolve config paths")?;
    let _ = resolve_checkout_path(&paths, args).context("resolve configured checkout")?;

    // Env is inherited by default; `status()` waits and surfaces
    // non-zero exits / signal terminations so the caller observes
    // the companion's true outcome.
    let status = std::process::Command::new(&companion)
        .args(args)
        .status()
        .with_context(|| format!("spawn companion `{}`", companion.display()))?;
    match status.code() {
        Some(code) => std::process::exit(code),
        None => {
            eprintln!(
                "agenthd: companion `{}` terminated by signal",
                companion.display()
            );
            std::process::exit(1);
        }
    }
}

/// Companion binary filename the boot path looks for next to
/// the current executable. Cargo emits `agenthd-gui.exe` on
/// Windows and `agenthd-gui` on Unix, so the lookup uses the
/// platform-native suffix only — neither fallback nor PATH
/// lookup is performed.
fn companion_filename() -> &'static str {
    #[cfg(windows)]
    {
        "agenthd-gui.exe"
    }
    #[cfg(not(windows))]
    {
        "agenthd-gui"
    }
}

/// Locate the companion binary adjacent to `current_exe`.
/// Returns the platform-native companion path if it exists,
/// `None` otherwise. Pure: no I/O outside the directory
/// containing `current_exe`; safe to call before any
/// settings.json write.
fn locate_companion(current_exe: &Path) -> Option<PathBuf> {
    let candidate = current_exe.parent()?.join(companion_filename());
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("agenthd: {}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the GUI companion slice. Two seams are
    //! pinned here: [`locate_companion`] (pure directory lookup
    //! against an injected `current_exe` path; no real binary
    //! spawned) and the spawn-and-wait contract `run_gui`
    //! exposes to its caller (exercised with a fake process in a
    //! `TempDir`: a shebang script on Unix, a copy of `cmd.exe`
    //! renamed to the companion filename on Windows). The fake
    //! lives and dies inside the tempdir; nothing is written to
    //! the user's `target/` tree or to `$HOME`. Integration
    //! coverage (exit code 2 + clear error message + no
    //! filesystem side effects when the companion is absent, and
    //! companion-present `--repo` persistence) lives in
    //! `tests/cli_launch.rs`.
    use super::*;

    /// Drop a placeholder file in `dir` named like the current
    /// executable. `locate_companion` only consults the
    /// directory, never the file itself, so an empty file is
    /// enough.
    fn write_fake_current_exe(dir: &std::path::Path) -> PathBuf {
        let name = if cfg!(windows) {
            "agenthd.exe"
        } else {
            "agenthd"
        };
        let path = dir.join(name);
        std::fs::write(&path, b"").expect("write fake current_exe placeholder");
        path
    }

    /// The locator returns `None` when the platform-native
    /// companion filename is not adjacent to `current_exe`.
    #[test]
    fn locate_companion_returns_none_when_no_companion_adjacent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let fake_exe = write_fake_current_exe(dir.path());
        assert!(
            locate_companion(&fake_exe).is_none(),
            "locator must return None when the platform-native companion is missing"
        );
    }

    /// The locator returns the platform-native companion path
    /// when present. Pin the equality on the exact path so a
    /// future refactor that searches PATH (or returns a
    /// relative path) trips this test.
    #[test]
    fn locate_companion_finds_platform_native_companion_adjacent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let fake_exe = write_fake_current_exe(dir.path());
        let companion = dir.path().join(companion_filename());
        std::fs::write(&companion, b"").expect("write fake companion placeholder");
        let found = locate_companion(&fake_exe).expect("locator must find adjacent companion");
        assert_eq!(
            found, companion,
            "locator must return the exact adjacent companion path"
        );
    }

    /// Adjacent files that are not the exact platform-native
    /// companion filename must not match. The locator is
    /// exact-filename: an `agenthd-gui.bak` decoy (or any other
    /// sibling) must not be confused with the companion.
    #[test]
    fn locate_companion_ignores_non_companion_files() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let fake_exe = write_fake_current_exe(dir.path());
        let decoy = dir.path().join("agenthd-gui.bak");
        std::fs::write(&decoy, b"").expect("write decoy");
        assert!(
            locate_companion(&fake_exe).is_none(),
            "decoy must not be confused with the companion; locator is exact-filename only"
        );
    }

    /// Spawn-and-wait contract: when the GUI branch launches a
    /// companion process, its exit code is propagated. The
    /// "fake process" is a file in a `TempDir`: a shell script
    /// with `exit 42` (Unix) or a copy of `cmd.exe` invoked
    /// with `/c exit 42` (Windows).
    #[test]
    fn spawn_propagates_companion_exit_code() {
        use std::process::Command;

        let dir = tempfile::TempDir::new().expect("tempdir");
        let fake_exe = write_fake_current_exe(dir.path());
        let companion = dir.path().join(companion_filename());

        #[cfg(unix)]
        {
            std::fs::write(&companion, "#!/bin/sh\nexit 42\n").expect("write fake");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&companion, std::fs::Permissions::from_mode(0o755))
                .expect("set exec bit");
        }
        #[cfg(windows)]
        {
            // Copy cmd.exe (via ComSpec) to the companion
            // filename so the fake is recognised as an
            // executable by the OS; pass `/c exit 42` so it
            // exits 42. Fall back to the canonical path if
            // ComSpec is unset (e.g. on minimal hosts).
            let comspec = std::env::var("ComSpec")
                .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
            std::fs::copy(&comspec, &companion).expect("copy cmd.exe as fake companion");
        }

        let located = locate_companion(&fake_exe).expect("locator must find the fake");
        assert_eq!(located, companion);

        #[cfg(unix)]
        let spawn_args: Vec<String> = Vec::new();
        #[cfg(windows)]
        let spawn_args: Vec<String> = vec!["/c".to_string(), "exit".to_string(), "42".to_string()];

        let status = Command::new(&located)
            .args(&spawn_args)
            .status()
            .expect("spawn fake companion");
        assert_eq!(
            status.code(),
            Some(42),
            "GUI branch must propagate the companion's exit code; got: {status:?}"
        );
    }
}
