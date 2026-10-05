//! Front-end-agnostic workflows for the agenthd runtime.
//!
//! Each entry point here orchestrates the data-store primitives in
//! `crate::store` and `crate::agent` and returns typed outcomes
//! (`CheckoutStatus`, `ApplyError`, `Vec<Agent>`, the Skills
//! orchestration's `Option<(State, Vec<SkillOutcome>)>`). Callers
//! decide how to render or transition around them.
//!
//! Flows:
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
//!
//! 4. [`plan_then_apply_skills`] is the Skills-install
//!    orchestration the TUI's `apply_skills_install` handler used
//!    to inline: take a freshly-loaded `State`, replan from disk,
//!    and only call `apply_skills` if the plan is non-empty. Empty
//!    plan is `Ok(None)` and writes nothing; non-empty plan returns
//!    the post-apply `(State, Vec<SkillOutcome>)`. No force-
//!    overwrite is performed on conflicts — the store layer's
//!    `Conflict` row surfaces a `skipped` outcome with `ok = true`.
//!
//! 5. [`plan_then_apply_agents_safe`] is the per-target agent
//!    safe-install orchestration the TUI's `apply_safe_install`
//!    handler used to inline: take a freshly-loaded `State` and
//!    the bound `SyncTarget`, fail-closed via `load_canonical`
//!    before any target-side read, replan via `plan_for` from
//!    disk, and only call `apply_safe` if the plan is non-empty.
//!    Empty plan is `Ok(None)` and writes nothing; non-empty plan
//!    returns the post-apply `(State, Vec<ApplyOutcome>)`. The
//!    `force_install` path stays on the TUI side — this workflow
//!    is the safe-install mirror of [`plan_then_apply_skills`]
//!    and never force-overwrites. Signature mirrors
//!    [`plan_then_apply_skills`]: a concrete `SyncTarget`, no UI
//!    preconditions in the contract.
//!
//! 6. [`save_agent`] is the rename-then-save helper the TUI's
//!    `EditorOp::Save` arm used to inline. When the agent name
//!    changed (`original_name != material.name`), the source
//!    hash is re-checked against `prior_hash` BEFORE the rename
//!    and the destination name is rejected if the canonical
//!    source has drifted; only then does `rename_canonical`
//!    run. After the rename (or when no rename is needed),
//!    `save_canonical` writes the rendered material with the
//!    **original** `prior_hash` so an external edit that
//!    slipped in between open and save is rejected. The
//!    composite path is **not** atomic across the rename + save
//!    boundary — a successful rename followed by a failed save
//!    leaves the source renamed with the pre-edit bytes; the
//!    TUI surfaces the error so the user can re-open and
//!    retry. Errors from `rename_canonical` are prefixed with
//!    `"rename: "`; errors from `save_canonical` are prefixed
//!    with `"save: "`, matching the inline TUI text exactly.

use crate::agent::Agent;
use crate::operation::{CancelToken, Finish, OperationReport, Progress};
use crate::store::{
    apply_safe, apply_safe_controlled, apply_skills, apply_skills_controlled, canonical_dir_from,
    hash_file, load_canonical, load_settings, plan_for, plan_skills, rename_canonical,
    save_canonical, save_settings, validate_checkout_path, ApplyOutcome, Paths, Settings,
    SkillOutcome, State, SyncTarget,
};
use anyhow::{anyhow, bail, Result};
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

/// Skills-install orchestration, UI-agnostic.
///
/// The caller has already reloaded `State` from disk (matching the
/// reload the Skills screen performs at the top of its install
/// handler); the workflow takes that fresh `State`, replans from
/// disk, and only invokes `apply_skills` when the plan is
/// non-empty. An empty plan is a no-op: `apply_skills` is not
/// called, no skill files are written, and `state.json` is not
/// rewritten.
///
/// Returns:
///
/// - `Ok(None)` — the replan produced no actionable rows. The
///   caller's `State` is the freshly-loaded value and is
///   authoritative; no disk write happens on this path.
/// - `Ok(Some((state, outcomes)))` — `apply_skills` ran; outcomes
///   describe what happened per skill, and the returned `state`
///   is the post-apply manifest (which `apply_skills` persists
///   only if it actually changed). Conflict rows in `outcomes`
///   carry `ok = true` because the refusal to overwrite is the
///   contract — the installer never force-overwrites a target
///   with different bytes.
/// - `Err(_)` — the replan or the apply failed. The caller's
///   in-memory `State` (the freshly-loaded value) remains valid;
///   this function does not return partial state on error.
///
/// The function does not render, mutate UI status, or call into
/// the TUI. The Skills screen keeps its state/status/key handling
/// and only delegates the orchestration here.
pub fn plan_then_apply_skills(
    paths: &Paths,
    state: State,
) -> Result<Option<(State, Vec<SkillOutcome>)>> {
    // Replan from disk before deciding whether to apply. The
    // store's `plan` walks the source tree fresh, so the caller's
    // `state` only seeds the manifest lookups; the per-row
    // decisions come from what is actually on disk now. This is
    // the "no invocar apply si plan vacío" guard: an empty plan
    // short-circuits before any filesystem write.
    let plan = plan_skills(paths, &state)?;
    if plan.is_empty() {
        return Ok(None);
    }
    let (new_state, outcomes) = apply_skills(paths, state, plan)?;
    Ok(Some((new_state, outcomes)))
}

/// Per-target safe-install orchestration, UI-agnostic.
///
/// The caller has already reloaded `State` from disk (matching the
/// reload the Install/Update screen performs at the top of its
/// safe-install handler) and resolved the bound `SyncTarget`
/// from its UI state. The workflow fail-closed-loads the
/// canonical source, replans from disk via `plan_for`, and only
/// invokes `apply_safe` when the plan is non-empty. An empty
/// plan is a no-op: `apply_safe` is not called, no agent files
/// are written, and `state.json` is not rewritten. The
/// `force_install` path stays on the TUI side — this workflow
/// is the safe-install mirror of [`plan_then_apply_skills`] and
/// never force-overwrites a conflict target.
///
/// Signature mirrors [`plan_then_apply_skills`]: a concrete
/// `SyncTarget` (not `Option<SyncTarget>`) so the workflow
/// stays free of UI preconditions like "no harness is bound".
/// The TUI owns that guard; on the no-target path the TUI
/// runs `load_canonical` once on its own to preserve the
/// original inline ordering (`load_canonical` runs before the
/// target guard surfaces `pick a harness first`), then maps
/// the success into `"pick a harness first"`. On the normal
/// path (target bound) the workflow is the single owner of
/// `load_canonical` — no duplication.
///
/// Returns:
///
/// - `Ok(None)` — the replan produced no actionable rows. The
///   caller's `State` is the freshly-loaded value and is
///   authoritative; no disk write happens on this path.
/// - `Ok(Some((state, outcomes)))` — `apply_safe` ran; outcomes
///   describe what happened per agent file, and the returned
///   `state` is the post-apply manifest (which `apply_safe`
///   persists only if it actually changed). Conflict rows in
///   `outcomes` carry `ok = true` because the refusal to
///   overwrite is the contract — the installer never force-
///   overwrites a target with different bytes.
/// - `Err(_)` — the source load, the replan, or the apply
///   failed. The caller's in-memory `State` (the freshly-loaded
///   value) remains valid; this function does not return partial
///   state on error.
///
/// The function does not render, mutate UI status, or call into
/// the TUI. The Install/Update screen keeps its state/status/
/// key handling and only delegates the orchestration here. The
/// caller is responsible for the post-apply refresh that
/// rebuilds the visible item list.
pub fn plan_then_apply_agents_safe(
    paths: &Paths,
    state: State,
    target: SyncTarget,
) -> Result<Option<(State, Vec<ApplyOutcome>)>> {
    // Fail closed on the canonical source BEFORE any target-side
    // read. The configured checkout has to be a real directory
    // with parseable agent files; without that guard the
    // planner could otherwise produce a `Remove` row that
    // `apply_safe` would later honor (silently deleting an
    // installed target) even though the source has vanished.
    load_canonical(paths)?;
    // Replan from disk before deciding whether to apply. The
    // store's `plan_for` walks both the source and target fresh,
    // so the caller's `state` only seeds the manifest lookups;
    // the per-row decisions come from what is actually on disk
    // now. This is the "no invocar apply si plan vacío" guard:
    // an empty plan short-circuits before any filesystem write.
    let plan = plan_for(paths, &state, target)?;
    if plan.is_empty() {
        return Ok(None);
    }
    let (new_state, outcomes) = apply_safe(paths, state, plan)?;
    Ok(Some((new_state, outcomes)))
}

/// Per-target safe-install orchestration with cooperative
/// cancellation and progress reporting.
///
/// Three checkpoints, in order:
///
/// 1. **Initial checkpoint** — fires before any source/target
///    read. A cancel here is honored before any disk IO and
///    returns `Finish::Cancelled` with the unchanged freshly-loaded
///    `State`. This is the GUI's chance to abort a queued run
///    without ever touching the source tree.
/// 2. **Existing canonical validation + replan** — the same
///    `load_canonical` + `plan_for` chain
///    [`plan_then_apply_agents_safe`] uses; this is a
///    **non-cancellable** unit by contract (planning/reading the
///    tree is cheap and atomic enough that introducing a cancel
///    seam here would just race with the tree walk itself). A
///    failure here returns `Finish::Failed` with the partial
///    state.
/// 3. **Apply controlled** — [`apply_safe_controlled`] handles
///    the per-row cancel checks. An empty plan short-circuits
///    between checkpoints 2 and 3 with `Finish::Completed` and no
///    disk write.
///
/// Returns an [`OperationReport`] whose `partial` is the fresh
/// `(State, Vec<ApplyOutcome>)` pair. On `Finish::Failed` or
/// `Finish::Cancelled` the caller is expected to keep the partial
/// state and refresh its UI; the workflow does not roll back or
/// surface legacy-style `Result::Err` for cooperative cancellation.
pub fn plan_then_apply_agents_safe_controlled(
    paths: &Paths,
    state: State,
    target: SyncTarget,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<(State, Vec<ApplyOutcome>)> {
    // Initial checkpoint.
    if token.is_requested() {
        return OperationReport {
            finish: Finish::Cancelled,
            partial: (state, Vec::new()),
            error: Some("cancelled before plan".to_string()),
        };
    }
    sink(Progress {
        stage: "agents: plan",
        item: None,
        processed: 0,
        total: None,
    });
    // Fail closed on the canonical source BEFORE any target-side
    // read. The configured checkout has to be a real directory
    // with parseable agent files; without that guard the planner
    // could otherwise produce a `Remove` row that `apply_safe`
    // would later honor (silently deleting an installed target)
    // even though the source has vanished.
    if let Err(e) = load_canonical(paths) {
        return OperationReport {
            finish: Finish::Failed,
            partial: (state, Vec::new()),
            error: Some(e.to_string()),
        };
    }
    // Replan from disk before deciding whether to apply.
    let plan = match plan_for(paths, &state, target) {
        Ok(p) => p,
        Err(e) => {
            return OperationReport {
                finish: Finish::Failed,
                partial: (state, Vec::new()),
                error: Some(e.to_string()),
            };
        }
    };
    if plan.is_empty() {
        sink(Progress {
            stage: "agents: plan",
            item: None,
            processed: 0,
            total: Some(0),
        });
        return OperationReport {
            finish: Finish::Completed,
            partial: (state, Vec::new()),
            error: None,
        };
    }
    apply_safe_controlled(paths, state, plan, token, sink)
}

/// Skills-install orchestration with cooperative cancellation and
/// progress reporting. Mirrors [`plan_then_apply_agents_safe_controlled`]:
/// initial checkpoint → canonical validation + replan →
/// apply_controlled. Planning/reading the skills tree is a
/// non-cancellable unit by contract; the row cancel seam lives in
/// [`apply_skills_controlled`].
pub fn plan_then_apply_skills_controlled(
    paths: &Paths,
    state: State,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<(State, Vec<SkillOutcome>)> {
    if token.is_requested() {
        return OperationReport {
            finish: Finish::Cancelled,
            partial: (state, Vec::new()),
            error: Some("cancelled before plan".to_string()),
        };
    }
    sink(Progress {
        stage: "skills: plan",
        item: None,
        processed: 0,
        total: None,
    });
    let plan = match plan_skills(paths, &state) {
        Ok(p) => p,
        Err(e) => {
            return OperationReport {
                finish: Finish::Failed,
                partial: (state, Vec::new()),
                error: Some(e.to_string()),
            };
        }
    };
    if plan.is_empty() {
        sink(Progress {
            stage: "skills: plan",
            item: None,
            processed: 0,
            total: Some(0),
        });
        return OperationReport {
            finish: Finish::Completed,
            partial: (state, Vec::new()),
            error: None,
        };
    }
    apply_skills_controlled(paths, state, plan, token, sink)
}

/// Save an agent, performing a rename first if the name changed.
///
/// `prior_hash` is the SHA-256 captured when the editor opened
/// the canonical file (or `None` for a new agent). It is passed
/// straight through to `save_canonical` so an external edit
/// made between open and save is rejected. Recomputing it here
/// would defeat the check.
///
/// This workflow is a verbatim lift of the previous inline
/// `save_agent` helper in `src/app/editor.rs`. Body and order
/// are preserved bit-for-bit:
///
/// 1. Compute `target_name` from the material; decide whether a
///    rename is needed by comparing against `original_name`.
/// 2. If renaming: hash the **source** path (the original
///    filename), bail if the hash drifted from `prior_hash`,
///    then `rename_canonical(old, target_name)`.
/// 3. `save_canonical(material, prior_hash)` — with the
///    original `prior_hash`, so a post-rename external edit
///    also surfaces as the standard "changed on disk" error.
/// 4. Return the final `target_name` so the caller can use it
///    for status messages without re-reading the material.
///
/// Composite non-atomic behavior (preserved, not improved): a
/// successful rename followed by a failed `save_canonical`
/// leaves the source renamed with the pre-edit bytes on disk
/// (the rendered `material` was never landed). The workflow
/// surfaces the `save_canonical` error verbatim so the caller
/// can re-open and retry; no compensating rename-back is
/// attempted. The store layer's atomicity guarantees apply
/// per `write_target` only.
///
/// Errors from `rename_canonical` are prefixed with `"rename: "`;
/// errors from `save_canonical` are prefixed with `"save: "`,
/// matching the inline TUI text exactly so the user-visible
/// error field is unchanged.
pub fn save_agent(
    paths: &Paths,
    original_name: Option<String>,
    prior_hash: Option<String>,
    material: Agent,
) -> Result<String> {
    let target_name = material.name.clone();
    let needs_rename = original_name
        .as_ref()
        .map(|o| o != &target_name)
        .unwrap_or(false);
    if needs_rename {
        let old = original_name.clone().unwrap();
        let source_path = paths.canonical_dir.join(format!("{}.md", old));
        let current_hash = hash_file(&source_path)?;
        if current_hash.as_deref() != prior_hash.as_deref() {
            bail!(
                "`{}` changed on disk since this edit started; reload to pick up the latest version",
                source_path.display()
            );
        }
        rename_canonical(paths, &old, &target_name).map_err(|e| anyhow!("rename: {}", e))?;
    }
    save_canonical(paths, &material, prior_hash.as_deref()).map_err(|e| anyhow!("save: {}", e))?;
    Ok(target_name)
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

    // ---------- plan_then_apply_skills ----------

    /// The empty-plan branch is a no-op: `apply_skills` is not
    /// invoked, no skill files appear under `paths.skills_dir`,
    /// and `state.json` is not rewritten. The workflow returns
    /// `Ok(None)` and the caller's `State` is unchanged.
    #[test]
    fn plan_then_apply_skills_empty_plan_is_no_op_and_does_not_write() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        paths.canonical_dir = dir.path().join("checkout").join("agents");
        // Configured checkout with `skills/` but no actual skill
        // subdirectories — every plan row is UpToDate/Absent.
        let checkout = paths.canonical_dir.parent().unwrap();
        fs::create_dir_all(checkout.join("agents")).unwrap();
        fs::create_dir_all(checkout.join("skills")).unwrap();
        // Seed a non-empty state.json so a rewrite would land
        // something on disk; we then assert it stays byte-stable.
        let state = State::default();
        let before_bytes = {
            if let Some(parent) = paths.state_file.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            // Seed directly via `serde_json` so the test does not
            // need to call `store::write_state` (which is
            // `pub(in crate::store)`).
            let bytes = serde_json::to_vec_pretty(&state).unwrap();
            fs::write(&paths.state_file, &bytes).unwrap();
            fs::read(&paths.state_file).unwrap()
        };
        assert!(
            !paths.skills_dir.join("anything").exists(),
            "skills_dir must be empty before the no-op call"
        );

        let result = plan_then_apply_skills(&paths, state).unwrap();
        assert!(result.is_none(), "empty plan must return Ok(None)");

        // No skill file landed under the configured skills dir. The
        // skills dir may not exist at all in this test (we never
        // called `Paths::ensure_dirs`); that is also a valid
        // "no writes" outcome. If it does exist, it must be empty.
        match fs::read_dir(&paths.skills_dir) {
            Ok(rd) => assert_eq!(
                rd.count(),
                0,
                "skills_dir must remain empty after the no-op call"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("unexpected skills_dir error: {e}"),
        }

        // state.json was NOT rewritten — bytes identical to seed.
        let after_bytes = fs::read(&paths.state_file).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "state.json must not be rewritten on the no-op path"
        );
    }

    /// `plan_then_apply_skills` must replan from disk, not trust
    /// the caller's `State` alone. Seed an old `State` whose
    /// manifest records a no-op outcome, then mutate the source
    /// tree on disk after the seed; the workflow replans and
    /// installs because disk says so, even though the old
    /// `State` did not. This pins the "no invocar apply si plan
    /// vacío" + "no confiar en plan stale" contract.
    #[test]
    fn plan_then_apply_skills_replans_from_disk_even_when_caller_state_is_stale() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        fs::create_dir_all(checkout.join("skills")).unwrap();
        paths.canonical_dir = checkout.join("agents");

        // Seed: caller hands us a `State` with no `installed_skills`
        // entries — the old `State` says "nothing owned", so a naive
        // caller could conclude there's no work. But the disk tells
        // a different story after we add a skill directory below.
        let stale_state = State::default();
        assert!(
            stale_state.installed_skills.is_empty(),
            "stale seed: empty manifest"
        );

        // Land a brand-new skill on disk AFTER the seed. The
        // workflow must replan from disk and install it.
        let skill = checkout.join("skills").join("foo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nbody\n",
        )
        .unwrap();

        let result =
            plan_then_apply_skills(&paths, stale_state).expect("replan + apply must succeed");
        let (new_state, outcomes) =
            result.expect("non-empty plan must produce Some((state, outcomes))");

        // At least one outcome for `foo`, marked installed / adopted.
        assert_eq!(outcomes.len(), 1, "expected one outcome");
        assert_eq!(outcomes[0].name, "foo");
        assert!(
            outcomes[0].ok,
            "install outcome must be ok: {}",
            outcomes[0].detail
        );

        // The manifest now records ownership of `foo` even though
        // the caller's stale State did not. This is the
        // "replan from disk" half of the contract.
        assert!(
            new_state.installed_skills.contains_key("foo"),
            "post-apply state must record `foo`"
        );

        // The skill landed on the destination side.
        assert!(
            paths.skills_dir.join("foo").join("SKILL.md").is_file(),
            "skill must have been published under skills_dir"
        );
    }

    /// Conflict rows: a target that already exists with different
    /// bytes produces a `Conflict` plan item and the apply pass
    /// leaves the target untouched (`apply_skills` only ever
    /// writes through `rename_no_replace` for owned targets, and
    /// unowned-with-different-bytes targets are refused). The
    /// outcome carries `ok = true` because the refusal is the
    /// contract — `plan_then_apply_skills` must NOT force-overwrite.
    #[test]
    fn plan_then_apply_skills_conflict_does_not_overwrite_target() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        fs::create_dir_all(checkout.join("skills")).unwrap();
        paths.canonical_dir = checkout.join("agents");

        // Source: a clean `foo` skill directory.
        let skill = checkout.join("skills").join("foo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nsource body\n",
        )
        .unwrap();

        // Destination: an UNOWNED `foo` directory with different
        // bytes — the planner must classify this as `Conflict`,
        // and the apply must refuse to touch it.
        let target = paths.skills_dir.join("foo");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nforeign body\n",
        )
        .unwrap();
        let foreign_bytes_before = fs::read(target.join("SKILL.md")).unwrap();

        let result =
            plan_then_apply_skills(&paths, State::default()).expect("replan + apply must succeed");
        let (new_state, outcomes) =
            result.expect("non-empty plan must produce Some((state, outcomes))");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].name, "foo");
        assert!(
            outcomes[0].ok,
            "conflict outcome must be ok=true (refusal is the contract): {}",
            outcomes[0].detail
        );
        assert!(
            outcomes[0].detail.contains("refusing")
                || outcomes[0].detail.contains("different bytes"),
            "conflict detail must mention refusal / different bytes, got: {}",
            outcomes[0].detail
        );

        // Target bytes unchanged on disk.
        let foreign_bytes_after = fs::read(target.join("SKILL.md")).unwrap();
        assert_eq!(
            foreign_bytes_before, foreign_bytes_after,
            "conflict target must NOT be force-overwritten"
        );

        // Manifest does NOT gain ownership of the conflicting
        // target — it remains an unowned foreign install.
        assert!(
            !new_state.installed_skills.contains_key("foo"),
            "manifest must not record ownership of a conflicting target"
        );
    }

    // ---------- plan_then_apply_agents_safe ----------

    /// Helper that writes an agent file into the canonical agents
    /// directory so the per-target safe-install planner has a real
    /// `NotInstalled` row to act on. Mirrors how the Skills test
    /// helpers seed a fresh source tree. The agent's `name` is
    /// derived from the filename (the canonical schema's contract,
    /// not from the frontmatter), so we keep the frontmatter
    /// minimal and parseable.
    fn write_canonical_agent(canonical_dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(canonical_dir).unwrap();
        std::fs::write(
            canonical_dir.join(format!("{name}.md")),
            format!("---\ndescription: test\nmode: subagent\n---\n{body}\n"),
        )
        .unwrap();
    }

    /// Empty-plan branch is a no-op: `apply_safe` is not invoked,
    /// no agent files appear under `paths.target_dir`, and
    /// `state.json` is not rewritten. The workflow returns
    /// `Ok(None)` and the caller's `State` is unchanged.
    #[test]
    fn plan_then_apply_agents_safe_empty_plan_is_no_op_and_does_not_write() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        // Configured checkout with `agents/` but no actual agent
        // files — every plan row is `Unowned`/`Absent` and there
        // is nothing the safe path would act on.
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        paths.canonical_dir = checkout.join("agents");

        // Seed a non-empty `state.json` so a rewrite would land
        // something on disk; we then assert it stays byte-stable.
        let state = State::default();
        let before_bytes = {
            if let Some(parent) = paths.state_file.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let bytes = serde_json::to_vec_pretty(&state).unwrap();
            fs::write(&paths.state_file, &bytes).unwrap();
            fs::read(&paths.state_file).unwrap()
        };

        let result = plan_then_apply_agents_safe(&paths, state, SyncTarget::OpenCode).unwrap();
        assert!(result.is_none(), "empty plan must return Ok(None)");

        // No agent file landed under the OpenCode target dir. The
        // target dir may not exist at all in this test (we never
        // called `Paths::ensure_dirs`); that is also a valid
        // "no writes" outcome. If it does exist, it must be empty.
        match fs::read_dir(&paths.target_dir) {
            Ok(rd) => assert_eq!(
                rd.count(),
                0,
                "target_dir must remain empty after the no-op call"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("unexpected target_dir error: {e}"),
        }

        // state.json was NOT rewritten — bytes identical to seed.
        let after_bytes = fs::read(&paths.state_file).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "state.json must not be rewritten on the no-op path"
        );
    }

    /// `plan_then_apply_agents_safe` must replan from disk, not
    /// trust the caller's `State` alone. Seed an empty `State`,
    /// then mutate the source tree on disk after the seed; the
    /// workflow replans and installs because disk says so, even
    /// though the old `State` did not. This pins the
    /// "no confiar en plan stale" half of the contract.
    #[test]
    fn plan_then_apply_agents_safe_replans_from_disk_even_when_caller_state_is_stale() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        paths.canonical_dir = checkout.join("agents");

        // Seed: caller hands us a `State` with no ownership
        // entries — the old `State` says "nothing owned", so a
        // naive caller could conclude there's no work. But the
        // disk tells a different story after we add an agent
        // file below.
        let stale_state = State::default();
        assert!(
            stale_state.installed.is_empty(),
            "stale seed: empty opencode manifest"
        );

        // Land a brand-new agent on disk AFTER the seed. The
        // workflow must replan from disk and install it.
        write_canonical_agent(&paths.canonical_dir, "foo", "source body");

        let result = plan_then_apply_agents_safe(&paths, stale_state, SyncTarget::OpenCode)
            .expect("replan + apply must succeed");
        let (new_state, outcomes) =
            result.expect("non-empty plan must produce Some((state, outcomes))");

        assert_eq!(outcomes.len(), 1, "expected one outcome");
        assert_eq!(outcomes[0].filename, "foo.md");
        assert!(
            outcomes[0].ok,
            "install outcome must be ok: {}",
            outcomes[0].detail
        );

        // The manifest now records ownership of `foo.md` even though
        // the caller's stale State did not. This is the
        // "replan from disk" half of the contract. The store
        // keys the ownership map by the full filename (with
        // `.md` extension), not the bare agent name.
        assert!(
            new_state.installed.contains_key("foo.md"),
            "post-apply state must record `foo.md` for OpenCode"
        );

        // The agent landed on the destination side.
        assert!(
            paths.target_dir.join("foo.md").is_file(),
            "agent must have been published under target_dir"
        );
    }

    /// Per-target scope: a plan computed for `SyncTarget::OpenCode`
    /// must NOT touch the Pi target directory or the Pi
    /// ownership map, even when the Pi target is empty. The
    /// mirror property holds in reverse. This pins the
    /// per-harness isolation the Install/Update session relies
    /// on.
    #[test]
    fn plan_then_apply_agents_safe_plan_is_scoped_to_the_bound_target_only() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        paths.canonical_dir = checkout.join("agents");

        // Source: one agent under canonical.
        write_canonical_agent(&paths.canonical_dir, "foo", "source body");

        // Both target dirs are missing — the planner must still
        // produce a `NotInstalled` row for the bound target only.
        let result = plan_then_apply_agents_safe(&paths, State::default(), SyncTarget::OpenCode)
            .expect("replan + apply must succeed");
        let (new_state, outcomes) =
            result.expect("non-empty plan must produce Some((state, outcomes))");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].filename, "foo.md");

        // OpenCode target received the agent.
        assert!(
            paths.target_dir.join("foo.md").is_file(),
            "bound (OpenCode) target must receive the agent"
        );
        // Pi target directory must NOT have been touched.
        assert!(
            !paths.pi_target_dir.exists() || !paths.pi_target_dir.join("foo.md").exists(),
            "other (Pi) target dir must not be touched"
        );
        // OpenCode manifest gains ownership, Pi manifest stays empty.
        assert!(
            new_state.installed.contains_key("foo.md"),
            "bound target manifest must record ownership"
        );
        assert!(
            !new_state.pi_installed.contains_key("foo.md"),
            "other target manifest must not be touched"
        );
    }

    /// Missing source fail-closed: when the configured canonical
    /// source has been removed out-of-band, the workflow must
    /// surface an explicit error from `load_canonical` BEFORE
    /// reaching `plan_for` — a missing checkout must never
    /// produce a phantom plan that `apply_safe` could otherwise
    /// honor (e.g., a precomputed `Remove` row).
    #[test]
    fn plan_then_apply_agents_safe_fails_closed_when_canonical_source_missing() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        // A `Paths` whose canonical_dir parent is missing must
        // surface an error before the planner runs.
        paths.canonical_dir = dir.path().join("nope").join("agents");
        let err = plan_then_apply_agents_safe(&paths, State::default(), SyncTarget::OpenCode)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not exist"),
            "expected missing-source error from safe-install workflow, got: {err}"
        );
    }

    // ---------- save_agent ----------

    /// Build a fresh agent with valid body content for the
    /// save-agent tests. Centralizes the description + prompt
    /// seeding so each test only has to vary the rename /
    /// prior-hash inputs.
    fn material(name: &str) -> Agent {
        let mut agent = Agent::new_default(name.to_string()).unwrap();
        agent.description = "test description".to_string();
        agent.prompt = "test prompt body".to_string();
        agent
    }

    /// New agent (no `original_name`, no `prior_hash`): the
    /// workflow must land the rendered material at
    /// `<canonical_dir>/<name>.md` and return `target_name`.
    /// No rename, no hash check; this is the "alta" path.
    #[test]
    fn save_agent_creates_new_canonical_when_no_original_name() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let target = material("helper");
        let returned = save_agent(&paths, None, None, target).expect("new save must succeed");
        assert_eq!(returned, "helper");
        let path = paths.canonical_dir.join("helper.md");
        assert!(path.is_file(), "new agent file must be created");
        let bytes = std::fs::read_to_string(&path).unwrap();
        assert!(
            bytes.contains("test prompt body"),
            "rendered body must be on disk"
        );
    }

    /// Same name as the original (no rename), with a
    /// `prior_hash` matching disk: the workflow must overwrite
    /// the existing file with the new rendered material and
    /// not touch any other file. This pins the "update"
    /// branch of the workflow.
    #[test]
    fn save_agent_updates_existing_canonical_when_name_unchanged() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        // Seed an existing agent on disk so the "update" path
        // is meaningful (overwriting an existing file rather
        // than creating a fresh one).
        let existing = material("helper");
        let path = paths.canonical_dir.join("helper.md");
        std::fs::write(&path, existing.render()).unwrap();
        let prior_hash = crate::store::hash_file(&path).unwrap();

        // Update the prompt and save with the matching prior
        // hash.
        let mut updated = material("helper");
        updated.prompt = "updated prompt body".to_string();
        let returned = save_agent(
            &paths,
            Some("helper".to_string()),
            prior_hash,
            updated.clone(),
        )
        .expect("update must succeed");
        assert_eq!(returned, "helper");
        let bytes = std::fs::read_to_string(&path).unwrap();
        assert!(
            bytes.contains("updated prompt body"),
            "updated body must land on disk"
        );
        // No rename: source path == destination path; no
        // sibling file.
        assert!(
            !paths.canonical_dir.join("helper.md").exists()
                || paths.canonical_dir.join("helper.md") == path,
            "no rename means no extra files"
        );
    }

    /// Rename path: `original_name != target_name` with a
    /// `prior_hash` matching disk. The workflow must rename
    /// the source to the new name, write the rendered
    /// material at the new path, and remove the old file.
    /// The return value is the new name.
    #[test]
    fn save_agent_renames_when_name_changes_and_prior_hash_matches() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let existing = material("helper");
        let old_path = paths.canonical_dir.join("helper.md");
        std::fs::write(&old_path, existing.render()).unwrap();
        let prior_hash = crate::store::hash_file(&old_path).unwrap();

        // Save as `assistant` (different name).
        let mut renamed = material("assistant");
        renamed.prompt = "renamed body".to_string();
        let returned = save_agent(
            &paths,
            Some("helper".to_string()),
            prior_hash,
            renamed.clone(),
        )
        .expect("rename + save must succeed");
        assert_eq!(returned, "assistant");

        // Old path gone, new path present with the new body.
        assert!(
            !old_path.exists(),
            "old canonical file must be removed after rename"
        );
        let new_path = paths.canonical_dir.join("assistant.md");
        assert!(new_path.is_file(), "new canonical file must exist");
        let bytes = std::fs::read_to_string(&new_path).unwrap();
        assert!(
            bytes.contains("renamed body"),
            "renamed body must land on disk"
        );
    }

    /// Stale source without moving: the source file was
    /// edited externally between open and save. The workflow
    /// must surface the inline-style error
    /// "`X` changed on disk since this edit started..." from
    /// the rename branch. Crucially, the file must NOT be
    /// renamed (the hash check happens before `rename_canonical`
    /// is called). For a non-rename save, `save_canonical`
    /// performs its own hash check and surfaces the same
    /// error shape; this test exercises the rename branch.
    #[test]
    fn save_agent_rename_branch_rejects_when_source_hash_drifts() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        // Seed the source.
        let existing = material("helper");
        let old_path = paths.canonical_dir.join("helper.md");
        std::fs::write(&old_path, existing.render()).unwrap();
        // Capture the hash as it stood when the editor opened
        // the file.
        let prior_hash = crate::store::hash_file(&old_path).unwrap();

        // External edit between open and save: rewrite the
        // source with different bytes.
        let mut external = material("helper");
        external.prompt = "external body slipped in".to_string();
        std::fs::write(&old_path, external.render()).unwrap();

        // Attempt a rename — the workflow must bail before
        // moving the file.
        let renamed = material("assistant");
        let err = save_agent(&paths, Some("helper".to_string()), prior_hash, renamed)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("changed on disk"),
            "expected stale-source error, got: {err}"
        );
        // Source file must NOT have been moved.
        assert!(
            old_path.exists(),
            "stale-source bail must not rename the file"
        );
        assert!(
            !paths.canonical_dir.join("assistant.md").exists(),
            "destination must not be created when source drifts"
        );
    }

    /// Destination occupied: renaming into a name whose
    /// canonical file already exists must surface the
    /// `"rename: "` prefixed error from `rename_canonical`
    /// (verbatim: "cannot rename to `X`: file already exists").
    /// The source file must remain in place.
    #[test]
    fn save_agent_rename_branch_rejects_when_destination_occupied() {
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        // Seed source: `helper.md`.
        let source = material("helper");
        let source_path = paths.canonical_dir.join("helper.md");
        std::fs::write(&source_path, source.render()).unwrap();
        let prior_hash = crate::store::hash_file(&source_path).unwrap();
        // Seed destination: `assistant.md` already on disk.
        let occupant = material("assistant");
        let dest_path = paths.canonical_dir.join("assistant.md");
        std::fs::write(&dest_path, occupant.render()).unwrap();
        let dest_bytes_before = std::fs::read(&dest_path).unwrap();

        // Attempt a rename into the occupied destination.
        let renamed = material("assistant");
        let err = save_agent(&paths, Some("helper".to_string()), prior_hash, renamed)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("rename: "),
            "expected `rename: ` prefix, got: {err}"
        );
        assert!(
            err.contains("file already exists"),
            "expected destination-occupied error from rename_canonical, got: {err}"
        );
        // Source must still be in place (the rename never
        // landed). Destination bytes unchanged (rename did
        // not touch it).
        assert!(
            source_path.exists(),
            "source must not be moved on a failed rename"
        );
        let dest_bytes_after = std::fs::read(&dest_path).unwrap();
        assert_eq!(
            dest_bytes_before, dest_bytes_after,
            "destination bytes must not be touched on a failed rename"
        );
    }

    // ---------- save_agent helpers ----------

    /// `setup_paths_with_checkout` lives in the app-level test
    /// module; we replicate a minimal version here so the
    /// `save_agent` tests can land canonical files against a
    /// real `<checkout>/agents/` directory (the same shape the
    /// inline editor used). The two test modules are
    /// independent — `workflows` does not pull from `app`'s
    /// private tests.
    fn setup_paths_with_checkout(dir: &TempDir) -> (Paths, std::path::PathBuf) {
        let paths = Paths {
            agenthd_root: dir.path().join(".agenthd"),
            canonical_dir: dir.path().join(".agenthd").join("agents"),
            state_file: dir.path().join(".agenthd").join("state.json"),
            target_dir: dir.path().join(".config").join("opencode").join("agents"),
            pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
            settings_file: dir.path().join(".agenthd").join("settings.json"),
        };
        let checkout = dir.path().join("checkout");
        let agents = checkout.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let paths = Paths {
            canonical_dir: agents.clone(),
            ..paths
        };
        (paths, checkout)
    }

    // ---------- D3 cooperative-cancellation tests (workflows) ----------
    //
    // Each test exercises one slice of the contract:
    // - initial checkpoint cancels BEFORE any IO,
    // - canonical validation/planning is non-cancellable
    //   (the contract says "Planning/read-tree unit
    //   NON-interruptible explicitly allowed"),
    // - empty plan returns Completed with no disk writes,
    // - controlled apply delegates cancel handling to the
    //   store layer.

    use crate::operation::{CancelToken, Finish, Progress};

    fn capture_progress() -> (
        std::rc::Rc<std::cell::RefCell<Vec<Progress>>>,
        impl FnMut(Progress),
    ) {
        let captured: std::rc::Rc<std::cell::RefCell<Vec<Progress>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink_captured = captured.clone();
        let sink = move |p: Progress| sink_captured.borrow_mut().push(p);
        (captured, sink)
    }

    /// Initial checkpoint: a cancel requested before the
    /// workflow reads the source tree is honored as
    /// `Finish::Cancelled` with the unchanged freshly-loaded
    /// state and an empty outcome list.
    #[test]
    fn plan_then_apply_agents_safe_controlled_pre_cancel_is_pure() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        let state = State::default();
        // Plant a sentinel in the opencode target dir; the
        // cancel-before-plan must not touch it.
        fs::create_dir_all(&paths.target_dir).unwrap();
        let sentinel = paths.target_dir.join("sentinel.md");
        fs::write(&sentinel, b"sentinel").unwrap();
        let token = CancelToken::default();
        token.request();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_agents_safe_controlled(
            &paths,
            state,
            SyncTarget::OpenCode,
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Cancelled);
        let (state_after, outcomes) = report.partial;
        assert!(outcomes.is_empty());
        assert!(state_after.installed.is_empty());
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"sentinel",
            "sentinel target file must be untouched after pre-cancel"
        );
    }

    /// Empty plan returns `Finish::Completed` with no disk
    /// writes and no state mutation.
    #[test]
    fn plan_then_apply_agents_safe_controlled_empty_plan_completes() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        // Build a checkout with an `agents/` directory but no
        // starter agents — plan is empty.
        let mut paths = setup_paths(&dir);
        paths.canonical_dir = dir.path().join("checkout").join("agents");
        let checkout = paths.canonical_dir.parent().unwrap();
        fs::create_dir_all(checkout.join("agents")).unwrap();
        let state = State::default();
        let token = CancelToken::default();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_agents_safe_controlled(
            &paths,
            state,
            SyncTarget::OpenCode,
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        let (state_after, outcomes) = report.partial;
        assert!(outcomes.is_empty());
        assert!(state_after.installed.is_empty());
    }

    /// Full run delegates to `apply_safe_controlled`. With no
    /// cancel, the workflow reports Completed and the
    /// canonical bytes are now published to the opencode
    /// target dir.
    #[test]
    fn plan_then_apply_agents_safe_controlled_full_run_publishes() {
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(checkout.join("agents")).unwrap();
        paths.canonical_dir = checkout.join("agents");
        write_canonical_agent(&paths.canonical_dir, "foo", "source body");
        let state = State::default();
        let token = CancelToken::default();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_agents_safe_controlled(
            &paths,
            state,
            SyncTarget::OpenCode,
            &token,
            &mut sink,
        );
        assert_eq!(report.finish, Finish::Completed);
        let (_state_after, outcomes) = report.partial;
        assert!(!outcomes.is_empty());
        // At least one outcome must have landed its target.
        let any_written = outcomes
            .iter()
            .any(|o| !o.action.is_empty() && paths.target_dir.join(&o.filename).is_file());
        assert!(
            any_written,
            "at least one outcome must correspond to a written target file"
        );
    }

    /// Same trio for the skills workflow: pre-cancel, empty
    /// plan, full run.
    #[test]
    fn plan_then_apply_skills_controlled_pre_cancel_is_pure() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let (paths, _checkout) = setup_paths_with_checkout(&dir);
        // Plant a sentinel skill under skills_dir; cancel-
        // before-plan must not touch it.
        fs::create_dir_all(&paths.skills_dir).unwrap();
        let sentinel = paths.skills_dir.join("sentinel");
        fs::create_dir_all(&sentinel).unwrap();
        fs::write(sentinel.join("SKILL.md"), b"sentinel").unwrap();
        let state = State::default();
        let token = CancelToken::default();
        token.request();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_skills_controlled(&paths, state, &token, &mut sink);
        assert_eq!(report.finish, Finish::Cancelled);
        let (state_after, outcomes) = report.partial;
        assert!(outcomes.is_empty());
        assert!(state_after.installed_skills.is_empty());
        assert!(sentinel.join("SKILL.md").is_file());
    }

    #[test]
    fn plan_then_apply_skills_controlled_empty_plan_completes() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        paths.canonical_dir = dir.path().join("checkout").join("agents");
        let checkout = paths.canonical_dir.parent().unwrap();
        fs::create_dir_all(checkout.join("agents")).unwrap();
        fs::create_dir_all(checkout.join("skills")).unwrap();
        let state = State::default();
        let token = CancelToken::default();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_skills_controlled(&paths, state, &token, &mut sink);
        assert_eq!(report.finish, Finish::Completed);
        let (state_after, outcomes) = report.partial;
        assert!(outcomes.is_empty());
        assert!(state_after.installed_skills.is_empty());
    }

    #[test]
    fn plan_then_apply_skills_controlled_full_run_publishes() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let mut paths = setup_paths(&dir);
        paths.canonical_dir = dir.path().join("checkout").join("agents");
        let checkout = paths.canonical_dir.parent().unwrap();
        fs::create_dir_all(checkout.join("agents")).unwrap();
        let skills_src = checkout.join("skills");
        fs::create_dir_all(&skills_src).unwrap();
        // Drop a single skill into the source tree.
        let skill = skills_src.join("alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            b"---\nname: alpha\ndescription: x\n---\nbody\n",
        )
        .unwrap();
        let state = State::default();
        let token = CancelToken::default();
        let (_captured, mut sink) = capture_progress();
        let report = plan_then_apply_skills_controlled(&paths, state, &token, &mut sink);
        assert_eq!(report.finish, Finish::Completed);
        let (_state_after, outcomes) = report.partial;
        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0].ok,
            "skill install must succeed: {:?}",
            outcomes[0].detail
        );
        assert!(paths.skills_dir.join("alpha").join("SKILL.md").is_file());
    }
}
