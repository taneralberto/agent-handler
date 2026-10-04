//! Plan and apply the per-target sync of canonical bytes into the
//! OpenCode / Pi target directories. Holds `SyncStatus`, `SyncTarget`,
//! `SyncItem`, `ApplyOutcome`, the read-only `compute_plan`, the safe
//! `apply_safe`, and the UI-confirmed `force_install`.
//!
//! All on-disk mutation goes through `super::write_target` and the
//! `State` ownership manifest written via `super::write_state`. The
//! classifier and target-byte computation live here because they are
//! pure sync helpers with no use outside this module.
//!
//! Per-target isolation contract:
//! - `plan_for(paths, state, target)` plans a single harness. The
//!   `compute_plan` wrapper composes the two calls for callers that
//!   still want the combined view; the Install/Update UI calls
//!   `plan_for` directly so a session bound to one harness can never
//!   read or write the other.
//! - `apply_safe` only cleans up ownership entries for targets that
//!   appear in the `items` slice it received. An empty plan is a
//!   safe no-op: nothing is written and the manifest is left as-is.

use super::{
    hash_file, require_canonical_source, sha256_hex, write_state, write_target, Paths, State,
};
use crate::agent::Agent;
use crate::operation::{run_cancel_checked, CancelToken, Finish, OperationReport, Progress};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    NotInstalled,
    UpToDate,
    UpdateAvailable,
    Conflict,
    Remove,
    PreserveModified,
    Unowned,
}

impl SyncStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SyncStatus::NotInstalled => "not installed",
            SyncStatus::UpToDate => "up to date",
            SyncStatus::UpdateAvailable => "update available",
            SyncStatus::Conflict => "conflict",
            SyncStatus::Remove => "remove",
            SyncStatus::PreserveModified => "preserve modified",
            SyncStatus::Unowned => "unowned",
        }
    }

    pub fn is_safe_action(&self) -> bool {
        matches!(
            self,
            SyncStatus::NotInstalled
                | SyncStatus::UpToDate
                | SyncStatus::UpdateAvailable
                | SyncStatus::Remove
        )
    }

    #[allow(dead_code)]
    pub fn requires_confirm(&self) -> bool {
        matches!(self, SyncStatus::Conflict)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncTarget {
    OpenCode,
    Pi,
}

impl SyncTarget {
    pub fn label(self) -> &'static str {
        match self {
            SyncTarget::OpenCode => "OpenCode",
            SyncTarget::Pi => "Pi",
        }
    }

    fn dir(self, paths: &Paths) -> &Path {
        match self {
            SyncTarget::OpenCode => &paths.target_dir,
            SyncTarget::Pi => &paths.pi_target_dir,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SyncItem {
    pub target: SyncTarget,
    pub filename: String,
    pub status: SyncStatus,
    pub canonical_hash: Option<String>,
    pub target_hash: Option<String>,
    pub last_installed_hash: Option<String>,
    pub canonical_path: PathBuf,
    pub target_path: PathBuf,
}

impl SyncItem {
    pub fn reason(&self) -> String {
        match self.status {
            SyncStatus::NotInstalled => "agenthd does not yet manage this file".to_string(),
            SyncStatus::UpToDate => "already in sync".to_string(),
            SyncStatus::UpdateAvailable => {
                "source changed since last install; safe to overwrite".to_string()
            }
            SyncStatus::Conflict => {
                "target was modified externally or is unowned; review before overwriting"
                    .to_string()
            }
            SyncStatus::Remove => {
                "canonical removed but owned target remains; safe to delete".to_string()
            }
            SyncStatus::PreserveModified => {
                "canonical removed and target was modified externally; preserved".to_string()
            }
            SyncStatus::Unowned => {
                if self.canonical_hash.is_none() && self.target_hash.is_none() {
                    "stale manifest entry with no source or target; cleaned up".to_string()
                } else {
                    "target exists without agenthd ownership; preserved".to_string()
                }
            }
        }
    }
}

fn target_bytes(target: SyncTarget, canonical_path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(canonical_path) {
        Ok(bytes) if target == SyncTarget::OpenCode => Ok(Some(bytes)),
        Ok(_) => Ok(Some(Agent::read(canonical_path)?.render_pi().into_bytes())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("read {}: {}", canonical_path.display(), e)),
    }
}

/// Read the canonical source for `target`, returning the bytes
/// that the install/update path will write — OpenCode targets
/// use the raw Markdown; Pi targets use the rendered subagent
/// form. The hash covers exactly those bytes. Returns `None`
/// only when the source is genuinely absent (`NotFound`); any
/// other error (symlink, non-regular, parse failure) bubbles up
/// so the caller can surface a per-item outcome rather than
/// dropping out of the loop on a `.expect`.
fn verified_canonical_bytes(
    target: SyncTarget,
    canonical_path: &Path,
) -> std::result::Result<Option<(Vec<u8>, String)>, String> {
    match fs::symlink_metadata(canonical_path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err(format!(
                    "{} is not a regular file; refusing to use it",
                    canonical_path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("stat {}: {}", canonical_path.display(), e)),
    }
    match target_bytes(target, canonical_path) {
        Ok(Some(bytes)) => {
            let hash = sha256_hex(&bytes);
            Ok(Some((bytes, hash)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("read {}: {}", canonical_path.display(), e)),
    }
}

/// Live observation of a target path. `present` is true when the
/// target is a regular file; `absent` covers `NotFound`; a symlink
/// or any other non-regular type is reported as `not_regular` so
/// the caller can refuse to overwrite or delete.
#[derive(Debug)]
enum TargetState {
    Absent,
    NotRegular,
    Regular { hash: String },
}

fn verified_target_state(target_path: &Path) -> std::result::Result<TargetState, String> {
    match fs::symlink_metadata(target_path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                Ok(TargetState::NotRegular)
            } else {
                Ok(TargetState::Regular {
                    hash: hash_file(target_path)
                        .map_err(|e| format!("hash {}: {}", target_path.display(), e))?
                        .ok_or_else(|| {
                            format!(
                                "{} vanished after stat; refresh to re-plan",
                                target_path.display()
                            )
                        })?,
                })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TargetState::Absent),
        Err(e) => Err(format!("stat {}: {}", target_path.display(), e)),
    }
}

/// Plan the sync for a single harness. The canonical set is read
/// unconditionally — both harnesses consume the same canonical bytes — but
/// the target directory, the target-side file existence check, and the
/// per-target ownership map are all scoped to the requested `target`.
///
/// This is the shared core used by both `compute_plan` (which fans out to
/// both harnesses) and the Install/Update UI (which plans only the harness
/// the user picked). Per-target isolation: planning OpenCode never reads
/// Pi's directory, and vice versa.
pub fn plan_for(paths: &Paths, state: &State, target: SyncTarget) -> Result<Vec<SyncItem>> {
    // Validate the configured canonical source BEFORE iteration. The
    // missing-source fail-closed contract applies to planning too:
    // if the checkout has been removed, the planner must surface an
    // explicit error rather than producing an empty plan that
    // `apply_safe` would later honor (e.g., a precomputed `Remove`
    // entry that would silently delete an installed target even
    // though canonical never had it).
    require_canonical_source(paths)?;
    // Planning is read-only: refreshing the Install/Update screen must not
    // create directories. Mutations create their own parent directories.
    let canonical_names = read_md_filenames(&paths.canonical_dir)?;
    let target_dir = target.dir(paths);
    let target_names = read_md_filenames(target_dir)?;
    let mut all_names: BTreeSet<String> = BTreeSet::new();
    all_names.extend(canonical_names.iter().cloned());
    all_names.extend(target_names.iter().cloned());
    all_names.extend(state.installed(target).keys().cloned());

    let mut items = Vec::new();
    for filename in all_names {
        let canonical_path = paths.canonical_dir.join(&filename);
        let target_path = target_dir.join(&filename);
        let canonical_hash = target_bytes(target, &canonical_path)?
            .as_deref()
            .map(sha256_hex);
        let target_hash = match fs::symlink_metadata(&target_path) {
            Ok(meta) => {
                if !meta.file_type().is_file() {
                    bail!(
                        "target {} is not a regular file; refusing to plan around it",
                        target_path.display()
                    );
                }
                hash_file(&target_path)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(anyhow!("stat {}: {}", target_path.display(), e)),
        };
        let last_installed_hash = state.installed(target).get(&filename).cloned();
        let status = classify(
            canonical_hash.as_ref(),
            target_hash.as_ref(),
            last_installed_hash.as_ref(),
        );
        items.push(SyncItem {
            target,
            filename,
            status,
            canonical_hash,
            target_hash,
            last_installed_hash,
            canonical_path,
            target_path,
        });
    }
    Ok(items)
}

/// Compute the sync plan for every harness. Convenience wrapper that
/// preserves the original two-target API used by older call sites and
/// tests; new code that needs per-target isolation should call
/// `plan_for` directly.
///
/// `#[allow(dead_code)]` keeps the re-export alive for the store's
/// unit tests, which still call this wrapper to verify the combined
/// view, even though the production TUI now binds to one harness at a
/// time.
#[allow(dead_code)]
pub fn compute_plan(paths: &Paths, state: &State) -> Result<Vec<SyncItem>> {
    let mut items = Vec::new();
    for target in [SyncTarget::OpenCode, SyncTarget::Pi] {
        items.extend(plan_for(paths, state, target)?);
    }
    Ok(items)
}

fn read_md_filenames(dir: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    if !dir.exists() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.file_type()?;
        if meta.is_dir() {
            continue;
        }
        if !meta.is_file() {
            bail!("{} is not a regular file", entry.path().display());
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".md") {
            names.insert(name);
        }
    }
    Ok(names)
}

fn classify(
    canonical_hash: Option<&String>,
    target_hash: Option<&String>,
    last_installed_hash: Option<&String>,
) -> SyncStatus {
    match (canonical_hash, target_hash, last_installed_hash) {
        (Some(_), None, _) => SyncStatus::NotInstalled,
        (Some(c), Some(t), _) if c == t => SyncStatus::UpToDate,
        (Some(c), Some(t), Some(l)) if c != t && t == l => SyncStatus::UpdateAvailable,
        (Some(_), Some(_), _) => SyncStatus::Conflict,
        (None, None, _) => SyncStatus::Unowned,
        (None, Some(t), Some(l)) if t == l => SyncStatus::Remove,
        (None, Some(_), Some(_)) => SyncStatus::PreserveModified,
        (None, Some(_), None) => SyncStatus::Unowned,
    }
}

/// Apply all safe actions from the plan. Returns the updated state
/// and a per-file summary.
///
/// Per-target isolation: the manifest-cleanup pass only touches
/// `state.installed_mut(target)` for targets that appear in
/// `items`, so a session bound to one harness can never prune
/// the other harness's ownership map. An empty `items` slice is
/// a safe no-op.
///
/// Per-item revalidation: the `SyncItem` snapshot was taken at
/// plan time. Every item is routed through a single
/// `revalidate_item` helper that re-reads the canonical source
/// (with the per-target byte transform) and the live target
/// hash. Each action branch then checks the snapshot against
/// the live observation before any mutation. A stale snapshot
/// produces an `error` outcome, retains ownership, and the loop
/// continues — successes are not blocked by sibling failures,
/// and the runtime never overwrites, deletes, or adopts a
/// target on the strength of a stale plan.
///
/// Residual TOCTOU race: the revalidation reads the source hash
/// then `write_target` atomically renames a temp file into
/// place. Between those two calls the canonical bytes could in
/// principle change (a concurrent editor save, an out-of-band
/// git pull). We do not claim an atomic compare-and-swap; the
/// worst case is a brief window where the target is replaced by
/// bytes that were current at revalidation time but stale by
/// the time the rename completed. The `compute_plan` ->
/// `apply_safe` round trip is the caller's natural retry
/// boundary.
pub fn apply_safe(
    paths: &Paths,
    mut state: State,
    items: Vec<SyncItem>,
) -> Result<(State, Vec<ApplyOutcome>)> {
    // The configured canonical source must still be a real
    // directory the planner actually inspected. A precomputed
    // `Remove` row from a vanished checkout would otherwise
    // delete an installed target.
    require_canonical_source(paths)?;
    let original_state = state.clone();
    let mut outcomes = Vec::new();
    let mut targets_touched: HashSet<SyncTarget> = HashSet::new();
    for item in items {
        targets_touched.insert(item.target);
        apply_one_row(&item, &mut state, &mut outcomes);
    }
    // Manifest cleanup is scoped to the targets the caller
    // actually planned (per-target isolation) and to rows
    // whose canonical AND target are both genuinely absent —
    // a stale-snapshot row that errored/skip-errored above
    // keeps its entry, because the next plan will reclassify
    // it on its own.
    for target in &targets_touched {
        let target_dir = target.dir(paths);
        state.installed_mut(*target).retain(|name, _| {
            target_dir.join(name).exists() || paths.canonical_dir.join(name).exists()
        });
    }
    if state != original_state {
        write_state(&paths.state_file, &state)?;
    }
    Ok((state, outcomes))
}

/// Apply all safe actions from the plan with cooperative
/// cancellation and progress reporting.
///
/// Cancel checkpoints are at the row boundary only: before each
/// row's `revalidate_item` (so a pre-cancel pass writes nothing
/// for the row), and after each row's `apply_one_row` (so a
/// successful row is persisted before reporting the safe
/// checkpoint). Cancellation is **not** observed inside an in-flight
/// row — `apply_one_row` mutates `state` and writes files, and
/// tearing it down mid-row would violate the row's atomic
/// guarantees. A late cancel that arrives after the last row
/// already committed is honored as `Completed` (the work is
/// already done; we never report a no-op run as cancelled).
///
/// On cancellation the global target-cleanup pass (the `retain`
/// over `state.installed_mut(target)` in [`apply_safe`]) is
/// **omitted** — a concurrent cancel between rows must not sweep
/// pending rows whose canonical/target might still resolve on a
/// re-plan. Only the per-row state mutations done so far are
/// persisted, and the report returns `Finish::Cancelled` with the
/// full partial state and outcomes so the caller can present
/// them. On a full run the cleanup + single-write state pass
/// matches [`apply_safe`] exactly so the observable behavior is
/// preserved.
///
/// Persistence: the row path persists exactly once per row (when
/// a row actually changed `state`). The final legacy-style
/// `write_state` is replaced by a per-row write that fires when
/// `state != original_state` immediately after the row commits,
/// so a cancelled run still has every completed row on disk.
/// Manifest persistence failure is surfaced as `Finish::Failed`
/// with the partial state and the explicit "manifest may have
/// changed" message; no rollback and no further rows are run.
pub fn apply_safe_controlled(
    paths: &Paths,
    mut state: State,
    items: Vec<SyncItem>,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<(State, Vec<ApplyOutcome>)> {
    let total = items.len();
    if let Err(detail) = require_canonical_source(paths) {
        return OperationReport {
            finish: Finish::Failed,
            partial: (state, Vec::new()),
            error: Some(detail.to_string()),
        };
    }
    let mut outcomes = Vec::new();
    let mut last_persisted = state.clone();
    // Track the targets the caller planned so the
    // Completed path can run the per-target `retain`
    // cleanup pass the legacy `apply_safe` performs.
    // Cancellation skips this cleanup — a stale snapshot
    // row that errored keeps its entry, and the next plan
    // will reclassify it on its own.
    let mut targets_touched: HashSet<SyncTarget> = HashSet::new();
    for (idx, item) in items.into_iter().enumerate() {
        targets_touched.insert(item.target);
        // Pre-row checkpoint: honor cancel, emit progress,
        // only then enter `apply_one_row`. A pre-cancel
        // pass writes nothing for the row.
        if run_cancel_checked(
            token,
            sink,
            Progress {
                stage: "agents: row",
                item: Some(item.filename.clone()),
                processed: idx,
                total: Some(total),
            },
        ) {
            return OperationReport {
                finish: Finish::Cancelled,
                partial: (state, outcomes),
                error: Some("cancelled before row".to_string()),
            };
        }
        apply_one_row(&item, &mut state, &mut outcomes);
        // Post-row checkpoint: persist if state actually
        // changed. The Completed cleanup below adds the
        // legacy per-target retain pass.
        if state != last_persisted {
            if let Err(e) = write_state(&paths.state_file, &state) {
                return OperationReport {
                    finish: Finish::Failed,
                    partial: (state, outcomes),
                    error: Some(format!(
                        "{} manifest write failed after row {}; manifest may have changed and was not saved: {}",
                        item.target_path.display(),
                        item.filename,
                        e
                    )),
                };
            }
            last_persisted = state.clone();
        }
    }
    // Final progress tick so UIs that key off
    // `processed == total` can settle before the Completed
    // report.
    sink(Progress {
        stage: "agents: row",
        item: None,
        processed: total,
        total: Some(total),
    });
    // Completed-only cleanup pass: drop owned manifest
    // entries whose canonical AND target files are both
    // absent on disk — matches the legacy `apply_safe`
    // exactly for the rows the caller actually processed.
    // Skipped on cancellation.
    for target in &targets_touched {
        let target_dir = target.dir(paths);
        state.installed_mut(*target).retain(|name, _| {
            target_dir.join(name).exists() || paths.canonical_dir.join(name).exists()
        });
    }
    if state != last_persisted {
        if let Err(e) = write_state(&paths.state_file, &state) {
            return OperationReport {
                finish: Finish::Failed,
                partial: (state, outcomes),
                error: Some(format!(
                    "{} final manifest write failed; manifest may have changed and was not saved: {}",
                    paths.state_file.display(),
                    e
                )),
            };
        }
    }
    OperationReport {
        finish: Finish::Completed,
        partial: (state, outcomes),
        error: None,
    }
}

/// Apply one `SyncItem` to `state`, appending the resulting
/// `ApplyOutcome` to `outcomes`. Shared by [`apply_safe`] (legacy)
/// and [`apply_safe_controlled`] (cooperative cancel) so the row
/// logic lives in one place; the wrappers only differ in how
/// they wrap the loop and persist the result.
fn apply_one_row(item: &SyncItem, state: &mut State, outcomes: &mut Vec<ApplyOutcome>) {
    // The State's owned hash for this filename is the
    // ground truth for who (if anyone) currently owns the
    // target. The plan's `last_installed_hash` is a snapshot
    // and may have drifted; every safe action compares them.
    let manifest_hash = state.installed(item.target).get(&item.filename).cloned();
    let snapshot = match revalidate_item(item.target, item) {
        Ok(s) => s,
        Err(detail) => {
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: "error".to_string(),
                detail,
                ok: false,
            });
            return;
        }
    };
    // Conflict / Unowned rows never mutate state — they
    // either need user review (Conflict / Unowned-with-file)
    // or are stash markers the next plan will resolve
    // (Unowned). PreserveModified is handled below because
    // its only safe action is releasing ownership when the
    // canonical is genuinely absent.
    if !item.status.is_safe_action() {
        if matches!(item.status, SyncStatus::PreserveModified) {
            // `PreserveModified` releases ownership of a
            // target the user has modified externally while
            // canonical was absent. The only preconditions
            // are that the canonical is still absent on
            // disk (so the plan's `last_installed_hash`
            // refers to genuine ownership the user
            // invalidated by editing) AND the manifest has
            // not drifted (so we are not silently dropping
            // ownership the user just re-acquired through a
            // concurrent adopt). The target file is never
            // touched; the existing modification is
            // preserved regardless of its current hash.
            if matches!(snapshot.canonical_state, CanonicalState::Absent)
                && item.last_installed_hash.as_ref() == manifest_hash.as_ref()
            {
                state.installed_mut(item.target).remove(&item.filename);
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "released".to_string(),
                    detail: "removed stale ownership entry".to_string(),
                    ok: true,
                });
            } else {
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "skipped".to_string(),
                    detail: format!(
                        "{} changed since plan; refresh to re-plan",
                        item.canonical_path.display()
                    ),
                    ok: true,
                });
            }
        } else {
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: "skipped".to_string(),
                detail: format!("status: {}", item.status.label()),
                ok: true,
            });
        }
        return;
    }
    // Manifest drift: the State's owned hash for this row
    // and the plan's `last_installed_hash` must agree. If
    // they disagree, the action's ownership precondition no
    // longer holds and we refuse to proceed. This is the
    // per-item snapshot-vs-State check the spec calls out:
    // mismatch → retain ownership and surface a
    // refresh/retry error.
    if item.last_installed_hash.as_ref() != manifest_hash.as_ref() {
        outcomes.push(ApplyOutcome {
            filename: item.filename.clone(),
            action: "error".to_string(),
            detail: format!(
                "{} manifest drifted between plan and apply; refresh to re-plan",
                item.target_path.display()
            ),
            ok: false,
        });
        return;
    }
    match item.status {
        SyncStatus::NotInstalled => {
            // `NotInstalled` requires (a) verified canonical
            // bytes whose hash matches the snapshot and
            // (b) the target absent on disk. If anything
            // changed since plan we never overwrite.
            let Some((bytes, hash)) = snapshot.canonical_bytes else {
                let detail = match snapshot.canonical_state {
                    CanonicalState::PresentNotRegular => format!(
                        "{} is not a regular file; refresh and retry",
                        item.canonical_path.display()
                    ),
                    CanonicalState::Absent | CanonicalState::PresentRegular => format!(
                        "{} disappeared after plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                };
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail,
                    ok: false,
                });
                return;
            };
            if Some(&hash) != item.canonical_hash.as_ref() {
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} changed since plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                    ok: false,
                });
                return;
            }
            match &snapshot.target_state {
                TargetState::Absent => {}
                TargetState::NotRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} is not a regular file; refusing to overwrite",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
                TargetState::Regular { .. } => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} appeared after plan; refresh to re-plan",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
            }
            write_or_record(state, item, &bytes, &hash, "installed", outcomes);
        }
        SyncStatus::UpdateAvailable => {
            // `UpdateAvailable` requires verified source
            // bytes whose hash matches the snapshot, AND
            // the live target must equal the snapshot's
            // `target_hash` (== `last_installed_hash`) so
            // we know we are not overwriting an externally
            // edited file.
            let Some((bytes, hash)) = snapshot.canonical_bytes else {
                let detail = match snapshot.canonical_state {
                    CanonicalState::PresentNotRegular => format!(
                        "{} is not a regular file; refresh and retry",
                        item.canonical_path.display()
                    ),
                    CanonicalState::Absent | CanonicalState::PresentRegular => format!(
                        "{} disappeared after plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                };
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail,
                    ok: false,
                });
                return;
            };
            if Some(&hash) != item.canonical_hash.as_ref() {
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} changed since plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                    ok: false,
                });
                return;
            }
            match &snapshot.target_state {
                TargetState::Regular { hash: current } => {
                    if Some(current) != item.target_hash.as_ref() {
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!(
                                "{} changed since plan; refresh to re-plan",
                                item.target_path.display()
                            ),
                            ok: false,
                        });
                        return;
                    }
                    if Some(current) != item.last_installed_hash.as_ref() {
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!(
                                "{} was modified externally; refusing to overwrite",
                                item.target_path.display()
                            ),
                            ok: false,
                        });
                        return;
                    }
                }
                TargetState::NotRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} is not a regular file; refusing to overwrite",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
                TargetState::Absent => {
                    // Target vanished between plan and apply;
                    // the snapshot said `UpdateAvailable`
                    // but the live filesystem is
                    // `NotInstalled`. Refuse: a stale
                    // `UpdateAvailable` row must never
                    // silently become an install, because
                    // the user's last-installed hash is
                    // gone with the target and any owner
                    // check now overwrites something the
                    // user did not consent to install.
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} vanished after plan; refresh to re-plan",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
            }
            write_or_record(state, item, &bytes, &hash, "updated", outcomes);
        }
        SyncStatus::UpToDate => {
            // `UpToDate` adopts ownership on disk evidence:
            // the source still matches the snapshot and the
            // target is still the regular file whose hash
            // matches the plan's `target_hash`.
            let Some((_bytes, hash)) = snapshot.canonical_bytes else {
                let detail = match snapshot.canonical_state {
                    CanonicalState::PresentNotRegular => format!(
                        "{} is not a regular file; refresh and retry",
                        item.canonical_path.display()
                    ),
                    CanonicalState::Absent | CanonicalState::PresentRegular => format!(
                        "{} disappeared after plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                };
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail,
                    ok: false,
                });
                return;
            };
            if Some(&hash) != item.canonical_hash.as_ref() {
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} changed since plan; refresh and retry",
                        item.canonical_path.display()
                    ),
                    ok: false,
                });
                return;
            }
            match &snapshot.target_state {
                TargetState::Regular { hash: current } => {
                    if Some(current) != item.target_hash.as_ref() {
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!(
                                "{} changed since plan; refresh to re-plan",
                                item.target_path.display()
                            ),
                            ok: false,
                        });
                        return;
                    }
                }
                TargetState::NotRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} is not a regular file; refresh to re-plan",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
                TargetState::Absent => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} vanished after plan; refresh to re-plan",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
            }
            // Manifest already agrees with the snapshot
            // (drift check above). Re-insert the verified
            // source hash so a tampered manifest snapshot
            // gets corrected to the live canonical.
            state
                .installed_mut(item.target)
                .insert(item.filename.clone(), hash);
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: "kept".to_string(),
                detail: "already in sync".to_string(),
                ok: true,
            });
        }
        SyncStatus::Remove => {
            // `Remove` requires (a) verified canonical
            // absence (or non-regular source we refuse to
            // trust), AND (b) a live target whose hash
            // matches the snapshot's `target_hash` AND the
            // manifest's owned hash. A stale Remove must
            // never overwrite or delete a target that has
            // since been externally modified or vanished.
            match snapshot.canonical_state {
                CanonicalState::Absent => {}
                CanonicalState::PresentNotRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} is not a regular file; refresh to re-plan",
                            item.canonical_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
                CanonicalState::PresentRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} reappeared after plan; refresh to re-plan",
                            item.canonical_path.display()
                        ),
                        ok: false,
                    });
                    return;
                }
            }
            match &snapshot.target_state {
                TargetState::Absent => {
                    state.installed_mut(item.target).remove(&item.filename);
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "removed".to_string(),
                        detail: format!("already absent: {}", item.target_path.display()),
                        ok: true,
                    });
                }
                TargetState::NotRegular => {
                    outcomes.push(ApplyOutcome {
                        filename: item.filename.clone(),
                        action: "error".to_string(),
                        detail: format!(
                            "{} is not a regular file; refresh to re-plan",
                            item.target_path.display()
                        ),
                        ok: false,
                    });
                }
                TargetState::Regular { hash: current } => {
                    if Some(current) != item.target_hash.as_ref() {
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!(
                                "{} changed since plan; refresh to re-plan",
                                item.target_path.display()
                            ),
                            ok: false,
                        });
                        return;
                    }
                    if Some(current) != item.last_installed_hash.as_ref() {
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!(
                                "{} was modified externally; refusing to remove",
                                item.target_path.display()
                            ),
                            ok: false,
                        });
                        return;
                    }
                    match fs::remove_file(&item.target_path) {
                        Ok(()) => {
                            state.installed_mut(item.target).remove(&item.filename);
                            outcomes.push(ApplyOutcome {
                                filename: item.filename.clone(),
                                action: "removed".to_string(),
                                detail: format!("removed {}", item.target_path.display()),
                                ok: true,
                            });
                        }
                        Err(e) => outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: "error".to_string(),
                            detail: format!("remove {}: {}", item.target_path.display(), e),
                            ok: false,
                        }),
                    }
                }
            }
        }
        _ => unreachable!(),
    }
}

/// Per-item live snapshot. Holds exactly the verified observations
/// `apply_safe` needs for every action branch, so each branch can
/// check its preconditions against one source of truth instead of
/// re-reading the filesystem through duplicated `match` chains.
///
/// `canonical_state` covers the source side for `Remove` (which
/// requires genuine absence) and for `PreserveModified`. The
/// install/update branches use `canonical_bytes`, which is `None`
/// iff the source is absent.
struct ItemSnapshot {
    canonical_state: CanonicalState,
    canonical_bytes: Option<(Vec<u8>, String)>,
    target_state: TargetState,
}

enum CanonicalState {
    Absent,
    PresentRegular,
    PresentNotRegular,
}

fn revalidate_item(
    target: SyncTarget,
    item: &SyncItem,
) -> std::result::Result<ItemSnapshot, String> {
    let canonical_state = match fs::symlink_metadata(&item.canonical_path) {
        Ok(meta) => {
            if meta.file_type().is_file() {
                CanonicalState::PresentRegular
            } else {
                CanonicalState::PresentNotRegular
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CanonicalState::Absent,
        Err(e) => return Err(format!("stat {}: {}", item.canonical_path.display(), e)),
    };
    let canonical_bytes = match canonical_state {
        CanonicalState::PresentRegular => verified_canonical_bytes(target, &item.canonical_path)?,
        _ => None,
    };
    let target_state = verified_target_state(&item.target_path)?;
    Ok(ItemSnapshot {
        canonical_state,
        canonical_bytes,
        target_state,
    })
}

/// Shared writer for `NotInstalled` and `UpdateAvailable`. Both
/// branches need identical failure handling around `write_target`,
/// so the write itself lives in one place.
fn write_or_record(
    state: &mut State,
    item: &SyncItem,
    bytes: &[u8],
    hash: &str,
    action: &str,
    outcomes: &mut Vec<ApplyOutcome>,
) {
    match write_target(&item.target_path, bytes) {
        Ok(()) => {
            state
                .installed_mut(item.target)
                .insert(item.filename.clone(), hash.to_string());
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: action.to_string(),
                detail: format!("wrote {}", item.target_path.display()),
                ok: true,
            });
        }
        Err(e) => outcomes.push(ApplyOutcome {
            filename: item.filename.clone(),
            action: "error".to_string(),
            detail: format!("write {}: {}", item.target_path.display(), e),
            ok: false,
        }),
    }
}

/// Force-overwrite a single conflict after UI confirmation.
///
/// Re-hashes the target immediately before writing. If the target now equals
/// the canonical bytes (someone else already synced it) the call is a no-op
/// that records the new ownership. If the target file vanished or is no
/// longer a regular file, the call is refused.
pub fn force_install(
    paths: &Paths,
    mut state: State,
    target: SyncTarget,
    filename: &str,
) -> Result<(State, ApplyOutcome)> {
    // Validate the configured canonical source BEFORE any file IO.
    // force_install reads from canonical and writes into the target,
    // and the call site is user-confirmed — so a missing source must
    // surface as an error rather than letting the read on
    // canonical_path return NotFound and the call claim a phantom
    // success.
    require_canonical_source(paths)?;
    let canonical_path = paths.canonical_dir.join(filename);
    let target_path = target.dir(paths).join(filename);
    let meta = fs::symlink_metadata(&canonical_path)
        .with_context(|| format!("stat {}", canonical_path.display()))?;
    if !meta.file_type().is_file() {
        bail!(
            "refusing to install {}: not a regular file",
            canonical_path.display()
        );
    }
    let canonical_bytes = target_bytes(target, &canonical_path)?.ok_or_else(|| {
        anyhow!(
            "canonical source {} disappeared after stat",
            canonical_path.display()
        )
    })?;
    let canonical_hash = sha256_hex(&canonical_bytes);
    match fs::symlink_metadata(&target_path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                bail!(
                    "refusing to overwrite {}: not a regular file",
                    target_path.display()
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow!("stat {}: {}", target_path.display(), e)),
    }
    let target_hash = hash_file(&target_path)?;
    if target_hash.as_deref() == Some(canonical_hash.as_str()) {
        // Already in sync; just adopt ownership.
        state
            .installed_mut(target)
            .insert(filename.to_string(), canonical_hash);
        write_state(&paths.state_file, &state)?;
        return Ok((
            state,
            ApplyOutcome {
                filename: filename.to_string(),
                action: "kept".to_string(),
                detail: "already in sync".to_string(),
                ok: true,
            },
        ));
    }
    write_target(&target_path, &canonical_bytes)?;
    state
        .installed_mut(target)
        .insert(filename.to_string(), canonical_hash);
    write_state(&paths.state_file, &state)?;
    Ok((
        state,
        ApplyOutcome {
            filename: filename.to_string(),
            action: "force installed".to_string(),
            detail: format!("wrote {}", target_path.display()),
            ok: true,
        },
    ))
}

#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub filename: String,
    pub action: String,
    pub detail: String,
    pub ok: bool,
}
