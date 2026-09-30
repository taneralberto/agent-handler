//! CLI launcher resolution.
//!
//! Owns the boot-path pieces of `agenthd` that are UI-independent:
//!
//! - [`parse_launch`]: CLI-surface parser. Parses `argv` into a
//!   [`LaunchPlan`] (`Mode` + optional `--repo` override) and is
//!   the parser the boot path uses to reject the GUI mode before
//!   any side effect. Unknown positional args are ignored (pre-D2
//!   tolerance). `--repo` value is rejected when it collides with
//!   a mode keyword; the value must be an absolute path; the first
//!   `--repo` wins on duplicates.
//! - [`parse_repo_override`]: legacy override reader used by
//!   [`resolve_checkout_path`] for the override / persistence
//!   branch. It re-parses argv for the *resolution* step (where
//!   the path is validated against the checkout workflow and,
//!   when it differs from `settings.json`, persisted). This is the
//!   second parser that sees argv on the boot path; the split is
//!   intentional — `parse_launch` owns the CLI surface, while
//!   `parse_repo_override` is the reader the legacy persistence
//!   contract (save-if-changed, first-wins-on-duplicates) was
//!   pinned against. The two parsers are aligned for any argv
//!   that passes `parse_launch` (mode-keyword collision in
//!   particular is rejected before `resolve_checkout_path` ever
//!   runs, so `parse_repo_override` never sees `--repo tui`/
//!   `--repo gui` in normal flow).
//! - [`ResolveOutcome`] / [`resolve_checkout_path`]: classify the
//!   configured checkout. Reads through
//!   [`crate::workflows::read_checkout`] so the boot path and the
//!   in-app Settings screen agree on what "valid / stale / missing"
//!   means. `--repo` overrides the persisted value and (when it
//!   differs) persists the override to `settings.json`.
//!
//! The module deliberately does NOT import `crossterm`, `ratatui`, or
//! `std::panic`: those stay in `main.rs` so the binary's terminal
//! lifecycle is the only place that knows about a TUI. This is the
//! seam that lets a future GUI client reuse the same resolution
//! without dragging the TUI dependencies along.
//!
//! Order of precedence in `resolve_checkout_path`:
//!
//! 1. `agenthd --repo <absolute path>`. The override is validated
//!    up front so the user is told *now* if the configured checkout
//!    is unusable rather than getting a half-started TUI.
//! 2. The persisted `settings.json` (via
//!    [`crate::workflows::read_checkout`]).
//! 3. First-run (`Empty`).
//!
//! When the persisted `settings.json` points at a checkout that has
//! moved or otherwise fails validation, the runtime does NOT silently
//! fall back to a cwd ancestor walk (no silent inference), nor does
//! it delete any targets. Instead it reports
//! [`ResolveOutcome::StaleCheckout`] so `run` can open the Settings
//! screen gated with the validation error visible; `--repo` may
//! repair the configuration before the TUI starts, otherwise the user
//! picks a new path inside the TUI.

use crate::store::{load_settings, save_settings, validate_checkout_path, Paths, Settings};
use crate::workflows::{read_checkout, CheckoutStatus};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Decide which checkout path the binary should run with this
/// invocation. See the module docs for precedence and behavior.
pub fn resolve_checkout_path(paths: &Paths, args: &[String]) -> Result<ResolveOutcome> {
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
    // Persisted branch: classify `settings.json` through the shared
    // workflow so the boot path and the Settings screen agree on
    // what "valid / stale / missing" means. `read_checkout`
    // returns `Err` on I/O / parse failure; that surfaces as the
    // binary's startup error exactly like the prior inline
    // `load_settings(...).context("read settings")?` did.
    match read_checkout(&paths.settings_file).context("read settings")? {
        CheckoutStatus::Ready(path) => Ok(ResolveOutcome::Ready {
            path,
            override_applied: false,
        }),
        CheckoutStatus::Stale { raw: _, banner } => {
            // The configured checkout is unusable. Surface the
            // failure in a gated Settings screen rather than
            // aborting the process or silently inferring a path
            // from the current working directory. The user sees
            // the validation error and can either type a new
            // path in the TUI or relaunch with `--repo` to repair
            // it. The workflow stays CLI-agnostic; this suffix
            // is the boot path's CLI hint.
            let banner = format!("{banner}; type a new path or relaunch with `--repo <path>`");
            Ok(ResolveOutcome::StaleCheckout { banner })
        }
        CheckoutStatus::Empty => Ok(ResolveOutcome::FirstRun),
    }
}

/// Outcome of [`resolve_checkout_path`]: the runtime may either know
/// the configured checkout (`Ready`), discover that the persisted
/// settings point at a checkout that is no longer valid
/// (`StaleCheckout`), or be running for the first time with no
/// settings at all (`FirstRun`).
#[derive(Debug)]
pub enum ResolveOutcome {
    Ready {
        path: PathBuf,
        override_applied: bool,
    },
    StaleCheckout {
        banner: String,
    },
    FirstRun,
}

/// UI mode keyword selected by the user via `argv`. `Tui` is the
/// only implemented mode today; `Gui` exists so [`parse_launch`]
/// has a single parser for the CLI surface and is rejected by the
/// boot path before any side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Gui,
}

/// Output of [`parse_launch`]: the parsed mode and the optional
/// `--repo` override. Persistence is owned by
/// [`resolve_checkout_path`] and unchanged for `Tui`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub mode: Mode,
    pub repo: Option<PathBuf>,
}

/// Parse `argv` into a [`LaunchPlan`].
///
/// Recognized tokens:
///
/// - `tui` / `gui` — mode keyword. The first occurrence wins; a
///   later conflicting keyword is an error so a typo (e.g. `gii`)
///   cannot silently flip the mode. Unknown positional args are
///   ignored to preserve the pre-D2 tolerance.
/// - `--repo <value>` — overrides the configured checkout. The
///   next token is consumed as the value. The value is rejected
///   when it equals `tui` or `gui` (mode-keyword collision, checked
///   before the absolute-path check so the user sees the most
///   specific reason). The value must be an absolute path. The
///   **first** `--repo` wins on duplicates — matches the legacy
///   [`parse_repo_override`] / [`resolve_checkout_path`] contract,
///   so `parse_launch` is the single source of truth for argv.
///
/// `parse_launch` does NOT touch the filesystem and does NOT
/// persist anything; persistence is the caller's job, gated on
/// the parsed mode.
pub fn parse_launch(args: &[String]) -> Result<LaunchPlan> {
    let mut mode: Option<Mode> = None;
    let mut repo: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--repo requires an absolute path argument"))?;
                match value.as_str() {
                    "tui" | "gui" => {
                        anyhow::bail!(
                            "--repo value `{value}` is a mode keyword; \
                             pass the absolute path before or after the mode"
                        );
                    }
                    _ => {}
                }
                let candidate = PathBuf::from(value);
                if !candidate.is_absolute() {
                    anyhow::bail!(
                        "--repo path `{}` must be an absolute path",
                        candidate.display()
                    );
                }
                // First `--repo` wins: matches `parse_repo_override`
                // and keeps the legacy persistence contract intact.
                if repo.is_none() {
                    repo = Some(candidate);
                }
                i += 2;
            }
            "tui" => {
                set_mode(&mut mode, Mode::Tui, args[i].as_str())?;
                i += 1;
            }
            "gui" => {
                set_mode(&mut mode, Mode::Gui, args[i].as_str())?;
                i += 1;
            }
            _ => {
                // Unknown positional: ignored (pre-D2 tolerance).
                i += 1;
            }
        }
    }
    Ok(LaunchPlan {
        mode: mode.unwrap_or(Mode::Tui),
        repo,
    })
}

fn set_mode(slot: &mut Option<Mode>, next: Mode, token: &str) -> Result<()> {
    match *slot {
        None => {
            *slot = Some(next);
            Ok(())
        }
        Some(prev) if prev == next => Ok(()),
        Some(_) => Err(anyhow::anyhow!(
            "conflicting mode keyword `{token}` in argv"
        )),
    }
}

/// Parse the optional `agenthd --repo <absolute path>` flag.
/// Positional args are ignored.
pub fn parse_repo_override(args: &[String]) -> Result<Option<PathBuf>> {
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

#[cfg(test)]
mod tests {
    //! `resolve_checkout_path` tests pin the boot path's mapping
    //! from `CheckoutStatus` to `ResolveOutcome`, the exact stale
    //! banner suffix the CLI boot path appends, the override
    //! precedence, the propagation of malformed-settings errors,
    //! and the save-if-changed / invalid-override contracts the
    //! boot path relies on. Tests construct a `Paths` through
    //! `Paths::resolve` (no global env mutation) and write
    //! `settings.json` inside a `TempDir`.
    //!
    //! `parse_launch` tests pin the CLI parser contract that closes
    //! D2: mode default (Tui), explicit `tui` / `gui`, both
    //! `--repo path mode` and `mode --repo path` orderings,
    //! `--repo` missing value, `--repo` colliding with a mode
    //! keyword, conflicting mode keywords, unknown positional
    //! tolerance, and a non-absolute `--repo` value rejection.
    //! These are pure unit tests — `parse_launch` does not touch
    //! the filesystem.

    use super::*;
    use crate::store::{save_settings, Settings};
    use tempfile::TempDir;

    /// Build a `Paths` rooted in `dir` via `Paths::resolve` so the
    /// binary's exact path-resolution code is exercised. `HOME` is
    /// the tempdir and `XDG_CONFIG_HOME` is a sibling of the
    /// agenthd root, so the agenthd root and the OpenCode target
    /// tree land inside the tempdir without touching the real
    /// environment.
    fn paths_in(dir: &TempDir) -> Paths {
        let home = dir.path().to_str().unwrap().to_string();
        let xdg = dir.path().join("xdg").to_str().unwrap().to_string();
        Paths::resolve(Some(&xdg), Some(&home)).unwrap()
    }

    /// Land a valid `settings.json` pointing at `checkout`.
    fn write_valid_settings(paths: &Paths, checkout: &std::path::Path) {
        if let Some(parent) = paths.settings_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        save_settings(
            &paths.settings_file,
            &Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
    }

    /// Land a malformed `settings.json`.
    fn write_malformed_settings(paths: &Paths) {
        if let Some(parent) = paths.settings_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&paths.settings_file, b"{not-json").unwrap();
    }

    fn empty_args() -> Vec<String> {
        Vec::new()
    }

    // ---------- persisted branch ----------

    #[test]
    fn resolve_persisted_valid_returns_ready_without_override_flag() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        write_valid_settings(&paths, &checkout);

        match resolve_checkout_path(&paths, &empty_args()).unwrap() {
            ResolveOutcome::Ready {
                path,
                override_applied,
            } => {
                assert_eq!(path, checkout);
                assert!(!override_applied);
            }
            other => panic!("expected Ready, got: {other:?}"),
        }
    }

    #[test]
    fn resolve_persisted_stale_returns_banner_with_exact_prior_suffix() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let bogus = dir.path().join("does-not-exist");
        write_valid_settings(&paths, &bogus);

        match resolve_checkout_path(&paths, &empty_args()).unwrap() {
            ResolveOutcome::StaleCheckout { banner } => {
                assert!(
                    banner.contains("is unusable"),
                    "banner carries the workflow validation prefix, got: {banner}"
                );
                assert!(
                    banner.contains("does not exist"),
                    "banner carries the underlying validation error, got: {banner}"
                );
                // Exact prior suffix the CLI boot path appends to
                // the workflow banner. This is the bit that lets
                // the user know they can repair with --repo.
                assert!(
                    banner.ends_with("type a new path or relaunch with `--repo <path>`"),
                    "banner must end with the prior CLI hint, got: {banner}"
                );
            }
            other => panic!("expected StaleCheckout, got: {other:?}"),
        }
    }

    #[test]
    fn resolve_persisted_absent_returns_first_run() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // No settings file written.
        match resolve_checkout_path(&paths, &empty_args()).unwrap() {
            ResolveOutcome::FirstRun => {}
            other => panic!("expected FirstRun, got: {other:?}"),
        }
    }

    #[test]
    fn resolve_persisted_malformed_settings_propagates_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        write_malformed_settings(&paths);

        let err = resolve_checkout_path(&paths, &empty_args()).unwrap_err();
        // The boot path adds `"read settings"` context around the
        // workflow's `load_settings` error, which itself surfaces
        // the "malformed" diagnostic from `serde_json`. The CLI
        // prints `err` with `{}` (Display, no chain), so the
        // user-visible string is just `"read settings"` — same as
        // before the refactor. The full chain (which downstream
        // consumers and `Debug` formatting can reach) still
        // carries the malformed-settings diagnostic.
        let top = err.to_string();
        assert!(
            top.contains("read settings"),
            "boot-path context must be the user-visible top, got: {top}"
        );
        let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();
        let joined = chain.join(" | ");
        assert!(
            chain.iter().any(|e| e.contains("malformed")),
            "underlying malformed-settings diagnostic must survive in the chain, got: {joined}"
        );
    }

    // ---------- override precedence ----------

    #[test]
    fn resolve_repo_override_takes_precedence_over_persisted_settings() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Persisted path: invalid. The override must win and the
        // persisted invalid path must NOT produce a StaleCheckout
        // banner (the override also repairs settings.json).
        let bogus = dir.path().join("persisted-bogus");
        write_valid_settings(&paths, &bogus);

        let override_path = dir.path().join("override-checkout");
        std::fs::create_dir_all(override_path.join("agents")).unwrap();
        let args = vec![
            "--repo".to_string(),
            override_path.to_string_lossy().into_owned(),
        ];

        match resolve_checkout_path(&paths, &args).unwrap() {
            ResolveOutcome::Ready {
                path,
                override_applied,
            } => {
                assert_eq!(path, override_path);
                assert!(override_applied, "override must set override_applied=true");
            }
            other => panic!("expected Ready (override), got: {other:?}"),
        }

        // The override also persisted its path to settings.json;
        // confirm the save-if-changed branch fired by re-loading.
        let persisted = load_settings(&paths.settings_file).unwrap().unwrap();
        assert_eq!(
            persisted,
            Settings::new(override_path.to_string_lossy().into_owned())
        );
    }

    // ---------- override parsing edge cases ----------

    /// `--repo` with a relative path must fail validation before
    /// any read or write happens; the boot path must NOT silently
    /// promote it, and `settings.json` must be left as it was.
    #[test]
    fn resolve_repo_override_relative_path_errors_without_writing() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Seed a valid prior settings.json so we can assert it is
        // untouched after the failing override.
        let prior = dir.path().join("prior-checkout");
        std::fs::create_dir_all(prior.join("agents")).unwrap();
        write_valid_settings(&paths, &prior);

        let args = vec!["--repo".to_string(), "relative/path".to_string()];
        let err = resolve_checkout_path(&paths, &args).unwrap_err();
        let top = err.to_string();
        assert!(
            top.contains("--repo path") && top.contains("must be an absolute path"),
            "expected relative-path rejection, got: {top}"
        );

        // settings.json must not have been rewritten by the failing
        // override — the validator runs before the save branch.
        let on_disk = load_settings(&paths.settings_file).unwrap().unwrap();
        assert_eq!(
            on_disk,
            Settings::new(prior.to_string_lossy().into_owned()),
            "failing --repo override must not rewrite settings.json"
        );
    }

    /// `--repo` without a value is a user error: the parser surfaces
    /// the missing-argument condition so the user is told *now*
    /// rather than at a deeper layer.
    #[test]
    fn parse_repo_override_missing_value_is_an_error() {
        let args = vec!["--repo".to_string()];
        let err = parse_repo_override(&args).unwrap_err().to_string();
        assert!(
            err.contains("--repo requires an absolute path argument"),
            "expected missing-value error, got: {err}"
        );
    }

    /// Positional arguments are ignored: a lone positional arg must
    /// not be promoted to a checkout path.
    #[test]
    fn parse_repo_override_ignores_positional_arguments() {
        let args = vec!["some-positional".to_string(), "/another".to_string()];
        assert!(parse_repo_override(&args).unwrap().is_none());
    }

    /// `--repo` followed by a path that equals the already-persisted
    /// settings must NOT rewrite `settings.json`. The
    /// `existing.as_ref() != Some(&settings)` guard is the
    /// save-if-changed branch; rewriting would still atomically
    /// succeed but would replace the file's bytes (and bump its
    /// mtime), which can confuse out-of-band watchers / sync tools.
    /// Pin the contract deterministically: seed the file with
    /// compact bytes (whitespace differs from pretty-printed JSON),
    /// capture the raw bytes, run the boot path, and assert the
    /// bytes are unchanged. No sleep, no mtime — works the same on
    /// Linux and on Windows regardless of filesystem timestamp
    /// resolution.
    #[test]
    fn resolve_repo_override_does_not_rewrite_settings_when_unchanged() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("same-checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();

        // Seed settings.json directly so we control the exact
        // bytes. Use `serde_json::to_vec` (compact) — `save_settings`
        // uses `to_vec_pretty`, so pretty formatting differs from
        // the seeded compact form. That whitespace asymmetry is
        // what we rely on to catch a rewrite.
        if let Some(parent) = paths.settings_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let seeded =
            serde_json::to_vec(&Settings::new(checkout.to_string_lossy().into_owned())).unwrap();
        std::fs::write(&paths.settings_file, &seeded).unwrap();

        let before_bytes = std::fs::read(&paths.settings_file).unwrap();
        // Sanity: the seeded compact bytes must NOT match what
        // save_settings would write; otherwise the test would
        // silently pass on a rewrite too. pretty formatting differs
        // from the compact form (whitespace, indentation), so the
        // two byte sequences are unequal.
        let pretty_bytes =
            serde_json::to_vec_pretty(&Settings::new(checkout.to_string_lossy().into_owned()))
                .unwrap();
        assert_ne!(
            before_bytes, pretty_bytes,
            "seeded compact bytes must differ from pretty-printed bytes so a rewrite is detectable\n  seeded: {:?}\n  pretty: {:?}",
            String::from_utf8_lossy(&before_bytes),
            String::from_utf8_lossy(&pretty_bytes)
        );

        let args = vec![
            "--repo".to_string(),
            checkout.to_string_lossy().into_owned(),
        ];
        let outcome = resolve_checkout_path(&paths, &args).unwrap();
        match outcome {
            ResolveOutcome::Ready {
                path,
                override_applied,
            } => {
                assert_eq!(path, checkout);
                assert!(
                    override_applied,
                    "override path was supplied; override_applied must be true"
                );
            }
            other => panic!("expected Ready, got: {other:?}"),
        }

        let after_bytes = std::fs::read(&paths.settings_file).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "settings.json bytes must be unchanged when --repo equals the persisted path"
        );
        // And the parsed value is still the same path.
        let on_disk = load_settings(&paths.settings_file).unwrap().unwrap();
        assert_eq!(
            on_disk,
            Settings::new(checkout.to_string_lossy().into_owned())
        );
    }

    // ---------- parse_launch (D2 seam) ----------

    /// No argv: the CLI surface defaults to TUI with no override.
    /// Pre-D2 callers relied on this exact shape.
    #[test]
    fn parse_launch_no_args_defaults_to_tui_without_repo() {
        let plan = parse_launch(&empty_args()).unwrap();
        assert_eq!(
            plan,
            LaunchPlan {
                mode: Mode::Tui,
                repo: None,
            }
        );
    }

    /// Explicit `tui` keyword: same default as no args, but the
    /// parser still has to acknowledge the keyword.
    #[test]
    fn parse_launch_explicit_tui_keyword() {
        let plan = parse_launch(&["tui".to_string()]).unwrap();
        assert_eq!(
            plan,
            LaunchPlan {
                mode: Mode::Tui,
                repo: None,
            }
        );
    }

    /// Explicit `gui` keyword: the parser accepts it (caller
    /// rejects before side effects); the plan must carry the
    /// future-mode value.
    #[test]
    fn parse_launch_explicit_gui_keyword() {
        let plan = parse_launch(&["gui".to_string()]).unwrap();
        assert_eq!(
            plan,
            LaunchPlan {
                mode: Mode::Gui,
                repo: None,
            }
        );
    }

    /// `--repo path` first, then `tui`: both orderings must parse
    /// identically and the path must NOT be confused with the mode.
    #[test]
    fn parse_launch_repo_then_tui_preserves_path() {
        let dir = TempDir::new().unwrap();
        let checkout = dir.path().join("checkout");
        let args = vec![
            "--repo".to_string(),
            checkout.to_string_lossy().into_owned(),
            "tui".to_string(),
        ];
        let plan = parse_launch(&args).unwrap();
        assert_eq!(plan.mode, Mode::Tui);
        assert_eq!(plan.repo, Some(checkout));
    }

    /// `tui` first, then `--repo path`: mirror of the prior test;
    /// the mode keyword must not consume the path.
    #[test]
    fn parse_launch_tui_then_repo_preserves_path() {
        let dir = TempDir::new().unwrap();
        let checkout = dir.path().join("checkout");
        let args = vec![
            "tui".to_string(),
            "--repo".to_string(),
            checkout.to_string_lossy().into_owned(),
        ];
        let plan = parse_launch(&args).unwrap();
        assert_eq!(plan.mode, Mode::Tui);
        assert_eq!(plan.repo, Some(checkout));
    }

    /// `--repo gui`: the value slot accidentally contains a mode
    /// keyword. The parser must reject this so the value is never
    /// confused with the mode. Pre-D2 callers could not trip this
    /// because there was no mode keyword; post-D2 the guard pins
    /// the contract.
    #[test]
    fn parse_launch_repo_value_colliding_with_gui_keyword_is_an_error() {
        let args = vec!["--repo".to_string(), "gui".to_string()];
        let err = parse_launch(&args).unwrap_err().to_string();
        assert!(
            err.contains("mode keyword"),
            "mode-keyword collision error must surface, got: {err}"
        );
    }

    /// `--repo tui`: mirror of the prior test for the TUI keyword.
    #[test]
    fn parse_launch_repo_value_colliding_with_tui_keyword_is_an_error() {
        let args = vec!["--repo".to_string(), "tui".to_string()];
        let err = parse_launch(&args).unwrap_err().to_string();
        assert!(
            err.contains("mode keyword"),
            "mode-keyword collision error must surface, got: {err}"
        );
    }

    /// `--repo` at the end of argv without a value: same error
    /// pre-D2 callers relied on.
    #[test]
    fn parse_launch_repo_missing_value_is_an_error() {
        let args = vec!["--repo".to_string()];
        let err = parse_launch(&args).unwrap_err().to_string();
        assert!(
            err.contains("--repo requires an absolute path argument"),
            "missing-value error must match the pre-D2 contract, got: {err}"
        );
    }

    /// `--repo <relative>`: still rejected; pre-D2 contract holds.
    #[test]
    fn parse_launch_repo_relative_path_is_an_error() {
        let args = vec!["--repo".to_string(), "relative/path".to_string()];
        let err = parse_launch(&args).unwrap_err().to_string();
        assert!(
            err.contains("must be an absolute path"),
            "absolute-path error must surface, got: {err}"
        );
    }

    /// `tui gui`: the parser refuses to silently pick one. The
    /// contract pins a user-facing error.
    #[test]
    fn parse_launch_conflicting_mode_keywords_is_an_error() {
        let args = vec!["tui".to_string(), "gui".to_string()];
        let err = parse_launch(&args).unwrap_err().to_string();
        assert!(
            err.contains("conflicting mode keyword"),
            "conflicting-mode error must surface, got: {err}"
        );
    }

    /// Unknown positional args are ignored, matching pre-D2
    /// tolerance. A lone positional must not flip the mode.
    #[test]
    fn parse_launch_ignores_unknown_positional_args() {
        let args = vec![
            "some-positional".to_string(),
            "/another".to_string(),
            "tui".to_string(),
        ];
        let plan = parse_launch(&args).unwrap();
        assert_eq!(plan.mode, Mode::Tui);
        assert_eq!(plan.repo, None);
    }

    /// The `--repo` value must not be confused with the mode
    /// keyword across both orderings when both happen to be on the
    /// command line. This is the integration-level guard against
    /// the value/mode confusion the parser promises.
    #[test]
    fn parse_launch_repo_path_and_tui_in_either_order() {
        let dir = TempDir::new().unwrap();
        let checkout = dir.path().join("checkout");

        // Order A: mode first.
        let a = vec![
            "tui".to_string(),
            "--repo".to_string(),
            checkout.to_string_lossy().into_owned(),
        ];
        let plan_a = parse_launch(&a).unwrap();
        assert_eq!(
            plan_a,
            LaunchPlan {
                mode: Mode::Tui,
                repo: Some(checkout.clone()),
            }
        );

        // Order B: --repo first.
        let b = vec![
            "--repo".to_string(),
            checkout.to_string_lossy().into_owned(),
            "tui".to_string(),
        ];
        let plan_b = parse_launch(&b).unwrap();
        assert_eq!(
            plan_b,
            LaunchPlan {
                mode: Mode::Tui,
                repo: Some(checkout),
            }
        );
    }

    /// Duplicate `--repo`: the FIRST wins. This matches the legacy
    /// `parse_repo_override` contract used by `resolve_checkout_path`,
    /// so `parse_launch` is the single source of truth — the plan's
    /// `repo` is always the path the persistence layer will use.
    /// Validates on every occurrence (no shortcut on later ones) so
    /// a bad second value still surfaces its own error.
    #[test]
    fn parse_launch_duplicate_repo_first_wins_and_still_validates_later() {
        let dir = TempDir::new().unwrap();
        let first = dir.path().join("first");
        let second = dir.path().join("second");

        // Two absolute paths: first wins, second is ignored.
        let plan = parse_launch(&[
            "--repo".to_string(),
            first.to_string_lossy().into_owned(),
            "--repo".to_string(),
            second.to_string_lossy().into_owned(),
        ])
        .unwrap();
        assert_eq!(plan.repo, Some(first));

        // First absolute, second relative: first still wins; the
        // second's relative-path error still surfaces because we
        // validate on every occurrence.
        let err = parse_launch(&[
            "--repo".to_string(),
            dir.path().to_string_lossy().into_owned(),
            "--repo".to_string(),
            "relative/path".to_string(),
        ])
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("must be an absolute path"),
            "later duplicate still validates, got: {err}"
        );
    }
}
