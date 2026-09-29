//! Front-end-agnostic workflows for the agenthd runtime.
//!
//! Each entry point here orchestrates the data-store primitives in
//! `crate::store` and `crate::agent` and returns typed outcomes
//! (`CheckoutStatus`, `ApplyError`, `Vec<Agent>`). Callers
//! decide how to render or transition around them.
//!
//! Three flows:
//!
//! 1. [`read_checkout`] classifies the persisted `settings.json`:
//!    a missing file is `Empty`, a valid path is `Ready`, a moved
//!    or otherwise invalid path is `Stale` with the raw invalid
//!    text so the editor can prefill it. I/O / parse failures
//!    propagate as `Err` so callers can decide whether to surface
//!    them or fall back to first-run.
//!
//! 2. [`apply_checkout`] validates a user-typed path, persists it
//!    through the store, re-validates the post-write state, and
//!    only then mutates `Paths::canonical_dir`. The ordering is
//!    pinned (trim → empty → absolute → validate → save →
//!    revalidate → mutate) so the visible failure modes stay
//!    identical to the prior TUI it replaces. `ensure_dirs` is
//!    deliberately NOT called: the runtime already created the
//!    agenthd root and target trees at startup, and a configured
//!    checkout's `agents/` directory must already exist (the
//!    validator requires it). Calling `ensure_dirs` here would
//!    also risk silently masking a real I/O error.
//!
//! 3. [`list_canonical_agents`] loads the configured checkout's
//!    `agents/` directory and returns the agents sorted by name.
//!    The configured source is validated by `load_canonical` up
//!    front, so a missing or moved checkout is an explicit error
//!    rather than an empty list. A valid empty directory returns
//!    an empty vec; the caller decides what to render around it.

use crate::agent::Agent;
use crate::store::{
    canonical_dir_from, load_canonical, load_settings, save_settings, validate_checkout_path,
    Paths, Settings,
};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// What the persisted `settings.json` contains, classified for
/// the Settings screen. A missing settings file is `Empty`; a
/// valid persisted path is `Ready`; a path that fails
/// `validate_checkout_path` is `Stale` and carries the raw
/// invalid text so the editor can prefill it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutStatus {
    /// The persisted path is valid. The wrapped `PathBuf` is the
    /// absolute checkout path; callers append `agents/` to reach
    /// the canonical directory when they need it.
    Ready(PathBuf),
    /// The persisted path exists on disk but fails validation
    /// (the checkout has moved, is now a symlink, etc.). The raw
    /// text is preserved so the editor can prefill it; the banner
    /// is the validation error formatted for display.
    Stale { raw: PathBuf, banner: String },
    /// No settings file exists yet.
    Empty,
}

/// Read and classify the persisted checkout. I/O and parse
/// failures propagate as `Err` so the caller can decide whether
/// to surface them or fall back to first-run.
pub fn read_checkout(settings_file: &Path) -> Result<CheckoutStatus> {
    let settings = match load_settings(settings_file)? {
        Some(s) => s,
        None => return Ok(CheckoutStatus::Empty),
    };
    let raw = PathBuf::from(&settings.checkout_path);
    match validate_checkout_path(&raw) {
        Ok(()) => Ok(CheckoutStatus::Ready(raw)),
        Err(err) => {
            // Validation-error banner; the CLI boot path appends
            // its own `--repo` hint on top of this string.
            let banner = format!("configured checkout `{}` is unusable: {err}", raw.display());
            Ok(CheckoutStatus::Stale { raw, banner })
        }
    }
}

/// Apply-time failure modes. Each variant carries the exact
/// text the TUI currently surfaces so the visible UI is
/// preserved bit-for-bit. The `.message()` accessor returns the
/// display string the screen renders into its error field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The buffer was empty after trimming. Distinct from
    /// `NotAbsolute` so the screen can render the empty-specific
    /// hint instead of the "not absolute" prefix.
    Empty,
    /// The trimmed path was non-empty but not absolute.
    NotAbsolute(PathBuf),
    /// `validate_checkout_path` refused the path (missing,
    /// symlink, missing `agents/`, etc.). The string is the
    /// store error verbatim — the TUI renders it as-is.
    Invalid(String),
    /// `save_settings` failed (I/O error, malformed JSON before
    /// write, etc.). The string is already prefixed with
    /// `"write settings: "` for parity with the TUI's existing
    /// screen-local message.
    Save(String),
    /// The post-write revalidation failed. The settings file is
    /// on disk but the configured checkout no longer passes
    /// validation. The string is already prefixed with
    /// `"re-validate: "` for parity with the TUI's existing
    /// screen-local message.
    Revalidate(String),
}

impl ApplyError {
    /// Visible error message. Matches the exact text the TUI
    /// surfaces in its `path_input.error` field today.
    pub fn message(&self) -> String {
        match self {
            ApplyError::Empty => "checkout path is empty; type the absolute path".to_string(),
            ApplyError::NotAbsolute(path) => {
                format!("`{}` is not an absolute path", path.display())
            }
            // `Invalid` carries the store error verbatim; `Save`
            // and `Revalidate` already carry their `write settings: `
            // / `re-validate: ` prefixes.
            ApplyError::Invalid(s) | ApplyError::Save(s) | ApplyError::Revalidate(s) => s.clone(),
        }
    }
}

/// Validate, persist, re-validate, and (only on full success)
/// mutate `paths.canonical_dir`. The trimmed path is returned
/// alongside the mutated paths so the caller can use it for
/// success-status messages without re-parsing the input.
///
/// On any failure `paths` is left untouched — the caller can
/// keep using its existing canonical directory even when the
/// user submits a bogus path.
///
/// `ensure_dirs` is intentionally NOT called: that helper
/// creates the output target trees and the agenthd root, but it
/// must not be conflated with the apply path. The runtime
/// already created those at startup; the configured checkout's
/// `agents/` directory is what the apply validates, not what it
/// makes.
pub fn apply_checkout(paths: &mut Paths, input: &str) -> Result<PathBuf, ApplyError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ApplyError::Empty);
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(ApplyError::NotAbsolute(path));
    }
    if let Err(err) = validate_checkout_path(&path) {
        return Err(ApplyError::Invalid(err.to_string()));
    }
    let settings = Settings::new(path.to_string_lossy().into_owned());
    if let Err(err) = save_settings(&paths.settings_file, &settings) {
        return Err(ApplyError::Save(format!("write settings: {err}")));
    }
    let new_canonical = match canonical_dir_from(&paths.agenthd_root, &settings) {
        Ok(p) => p,
        Err(err) => {
            return Err(ApplyError::Revalidate(format!("re-validate: {err}")));
        }
    };
    paths.canonical_dir = new_canonical;
    Ok(path)
}

/// Sorted canonical agents. The configured-checkout `agents/`
/// directory is validated by `load_canonical` up front, so a
/// missing or moved checkout is an explicit error rather than
/// an empty vec. A valid empty directory (a freshly-cloned
/// checkout with no `.md` files) returns an empty vec; the
/// caller decides what UI to render around it.
pub fn list_canonical_agents(paths: &Paths) -> Result<Vec<Agent>> {
    let map = load_canonical(paths)?;
    let mut agents: Vec<Agent> = map.into_values().map(|(agent, _)| agent).collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

#[cfg(test)]
mod tests {
    //! Direct workflow tests. These pin the workflow contract
    //! independently of the TUI surface.

    use super::*;
    use crate::agent::{starter_agent, STARTERS};
    use tempfile::TempDir;

    /// Build a `Paths` whose agenthd root lives inside the tempdir
    /// and whose settings file lives at the conventional
    /// `<root>/settings.json`. `canonical_dir` defaults to
    /// `<root>/agents`; tests that exercise a configured
    /// checkout re-point it via `apply_checkout` /
    /// `Paths::with_settings`.
    fn setup_paths(dir: &TempDir) -> Paths {
        Paths {
            agenthd_root: dir.path().join(".agenthd"),
            canonical_dir: dir.path().join(".agenthd").join("agents"),
            state_file: dir.path().join(".agenthd").join("state.json"),
            target_dir: dir.path().join(".config").join("opencode").join("agents"),
            pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
            settings_file: dir.path().join(".agenthd").join("settings.json"),
        }
    }

    /// Write `settings.json` for `paths` with `checkout_path`
    /// pointing at `checkout`. The parent directory is created
    /// on demand so tests don't have to call `Paths::ensure_dirs`
    /// just to land a settings file.
    fn write_settings(paths: &Paths, checkout: &Path) {
        if let Some(parent) = paths.settings_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();
    }

    // ---------- read_checkout ----------

    #[test]
    fn read_checkout_valid_persisted_path_returns_ready() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        write_settings(&paths, &checkout);

        match read_checkout(&paths.settings_file).unwrap() {
            CheckoutStatus::Ready(p) => assert_eq!(p, checkout),
            other => panic!("expected Ready, got: {other:?}"),
        }
    }

    #[test]
    fn read_checkout_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        assert!(matches!(
            read_checkout(&paths.settings_file).unwrap(),
            CheckoutStatus::Empty
        ));
    }

    #[test]
    fn read_checkout_invalid_persisted_path_returns_stale_with_raw_text() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let bogus = dir.path().join("does-not-exist");
        write_settings(&paths, &bogus);

        match read_checkout(&paths.settings_file).unwrap() {
            CheckoutStatus::Stale { raw, banner } => {
                assert_eq!(raw, bogus, "raw preserved for the editor buffer");
                assert!(
                    banner.contains("does not exist"),
                    "banner carries the validation error, got: {banner}"
                );
            }
            other => panic!("expected Stale, got: {other:?}"),
        }
    }

    #[test]
    fn read_checkout_malformed_settings_is_an_error() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        if let Some(parent) = paths.settings_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&paths.settings_file, b"{not-json").unwrap();

        let err = read_checkout(&paths.settings_file).unwrap_err().to_string();
        assert!(
            err.contains("malformed"),
            "expected malformed-settings error, got: {err}"
        );
    }

    // ---------- apply_checkout ----------

    #[test]
    fn apply_checkout_empty_input_is_rejected() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let before = paths.clone();
        let err = apply_checkout(&mut paths, "   ").unwrap_err();
        assert_eq!(err, ApplyError::Empty);
        assert_eq!(
            err.message(),
            "checkout path is empty; type the absolute path"
        );
        assert_eq!(paths, before, "empty input must not mutate paths");
    }

    #[test]
    fn apply_checkout_relative_input_is_rejected_with_exact_prefix() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let before = paths.clone();
        let input = "relative/path";
        let err = apply_checkout(&mut paths, input).unwrap_err();
        match &err {
            ApplyError::NotAbsolute(p) => {
                assert_eq!(p, &PathBuf::from(input));
            }
            other => panic!("expected NotAbsolute, got: {other:?}"),
        }
        assert_eq!(
            err.message(),
            "`relative/path` is not an absolute path",
            "exact TUI error prefix must be preserved"
        );
        assert_eq!(paths, before, "relative input must not mutate paths");
    }

    #[test]
    fn apply_checkout_missing_path_is_rejected_without_persisting_or_mutating() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let before = paths.clone();
        let missing = dir.path().join("does-not-exist");
        let err = apply_checkout(&mut paths, &missing.to_string_lossy()).unwrap_err();
        match &err {
            ApplyError::Invalid(msg) => {
                assert!(
                    msg.contains("does not exist"),
                    "validation error must surface, got: {msg}"
                );
            }
            other => panic!("expected Invalid, got: {other:?}"),
        }
        assert_eq!(paths, before, "invalid input must not mutate paths");
        assert!(
            !paths.settings_file.exists(),
            "invalid input must not write settings.json"
        );
    }

    #[test]
    fn apply_checkout_valid_path_persists_repoints_canonical_dir_and_does_not_create_dirs() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        // No `ensure_dirs` call: the test pins that the apply
        // path does not create agenthd-root directories beyond
        // what `save_settings` (via `write_target`) needs to
        // land the settings file.
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();

        let resolved = apply_checkout(&mut paths, &checkout.to_string_lossy()).unwrap();
        assert_eq!(resolved, checkout);

        // settings.json was written and contains the path.
        let on_disk = load_settings(&paths.settings_file).unwrap().unwrap();
        assert_eq!(
            on_disk,
            Settings::new(checkout.to_string_lossy().into_owned())
        );

        // canonical_dir was re-pointed at <checkout>/agents.
        assert_eq!(paths.canonical_dir, checkout.join("agents"));

        // The historical <agenthd_root>/agents default must NOT
        // exist: that directory was the old local-mode source
        // and `apply_checkout` must never create it. The
        // agenthd root itself may exist (save_settings' parent
        // mkdir lands it), but the runtime does not populate
        // it as a canonical source.
        assert!(
            !dir.path().join(".agenthd").join("agents").exists(),
            "apply must not create <agenthd_root>/agents"
        );
        // The target trees (OpenCode agents, Pi agents, skills)
        // were not created either; `ensure_dirs` is the only
        // thing that creates them and the apply does not call
        // it.
        assert!(
            !paths.target_dir.exists(),
            "apply must not create the OpenCode target dir"
        );
        assert!(
            !paths.pi_target_dir.exists(),
            "apply must not create the Pi target dir"
        );
        assert!(
            !paths.skills_dir.exists(),
            "apply must not create the skills dir"
        );
    }

    #[test]
    fn apply_checkout_preserves_paths_on_failure() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        // Seed a valid prior settings file so the failing apply
        // can be checked against it.
        let prior = dir.path().join("prior-checkout");
        std::fs::create_dir_all(prior.join("agents")).unwrap();
        write_settings(&paths, &prior);
        // Repoint canonical_dir at the prior checkout (as if the
        // runtime had boot-strapped successfully).
        paths.canonical_dir = prior.join("agents");
        let before = paths.clone();

        // Submit a bogus input: the apply must fail without
        // mutating paths and without overwriting the prior
        // settings.json.
        let bogus = dir.path().join("bogus");
        let err = apply_checkout(&mut paths, &bogus.to_string_lossy()).unwrap_err();
        assert!(matches!(err, ApplyError::Invalid(_)));
        assert_eq!(paths, before, "failed apply must not mutate paths");
        let on_disk = load_settings(&paths.settings_file).unwrap().unwrap();
        assert_eq!(on_disk, Settings::new(prior.to_string_lossy().into_owned()));
    }

    // ---------- list_canonical_agents ----------

    #[test]
    fn list_canonical_agents_sorts_by_name_regardless_of_on_disk_order() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let agents_dir = dir.path().join("checkout").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        // Write three starter files in reverse-alphabetical order.
        // `Agent::read` does not require frontmatter validity
        // for the list operation to succeed: every starter is a
        // valid canonical document.
        for starter in STARTERS.iter().rev() {
            std::fs::write(
                agents_dir.join(format!("{}.md", starter.name)),
                starter_agent(starter).render(),
            )
            .unwrap();
        }
        let paths = paths
            .with_settings(&Settings::new(
                dir.path().join("checkout").to_string_lossy().into_owned(),
            ))
            .unwrap();

        let agents = list_canonical_agents(&paths).unwrap();
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        let mut expected: Vec<&str> = STARTERS.iter().map(|s| s.name).collect();
        expected.sort();
        assert_eq!(names, expected, "agents must be sorted by name");
    }

    #[test]
    fn list_canonical_agents_empty_checkout_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let checkout = dir.path().join("empty-checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        let paths = paths
            .with_settings(&Settings::new(checkout.to_string_lossy().into_owned()))
            .unwrap();
        assert!(list_canonical_agents(&paths).unwrap().is_empty());
    }

    #[test]
    fn list_canonical_agents_fails_closed_when_checkout_missing() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        // A `Paths` whose canonical_dir parent is missing must
        // surface an error from the listing: `load_canonical`
        // validates the source up front so the listing cannot
        // silently return an empty vec when the configured
        // checkout has been removed out-of-band.
        paths.canonical_dir = dir.path().join("nope").join("agents");
        let err = list_canonical_agents(&paths).unwrap_err().to_string();
        assert!(
            err.contains("does not exist"),
            "expected missing-source error from listing, got: {err}"
        );
    }
}
