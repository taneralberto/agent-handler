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

use super::{hash_file, sha256_hex, write_state, write_target, Paths, State};
use crate::agent::Agent;
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

/// Apply all safe actions from the plan. Returns the updated state and a
/// per-file summary.
///
/// Per-target isolation: the manifest-cleanup loop only touches
/// `state.installed_mut(target)` for targets that appear in `items`.
/// Callers that plan a single harness (the Install/Update UI when bound
/// to OpenCode or Pi) get a guaranteed no-op on the other target's
/// ownership map, so a stale Pi entry cannot be silently pruned by a
/// Ui session that never touched Pi. An empty `items` slice is a safe
/// no-op: nothing is processed and no cleanup runs.
pub fn apply_safe(
    paths: &Paths,
    mut state: State,
    items: Vec<SyncItem>,
) -> Result<(State, Vec<ApplyOutcome>)> {
    let original_state = state.clone();
    let mut outcomes = Vec::new();
    let mut targets_touched: HashSet<SyncTarget> = HashSet::new();
    for item in items {
        // Record the target before mutating state so the cleanup pass at
        // the end knows exactly which harnesses' ownership maps may need
        // pruning. Without this, the cleanup would walk every target
        // unconditionally and could prune an entry on a target the
        // caller never asked us to plan.
        targets_touched.insert(item.target);
        // Release ownership for modified orphans even when the target is
        // preserved — the canonical was removed and the user owns the file.
        if matches!(item.status, SyncStatus::PreserveModified) {
            state.installed_mut(item.target).remove(&item.filename);
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: "released".to_string(),
                detail: "removed stale ownership entry".to_string(),
                ok: true,
            });
            continue;
        }
        if !item.status.is_safe_action() {
            outcomes.push(ApplyOutcome {
                filename: item.filename.clone(),
                action: "skipped".to_string(),
                detail: format!("status: {}", item.status.label()),
                ok: true,
            });
            continue;
        }
        match item.status {
            SyncStatus::NotInstalled | SyncStatus::UpdateAvailable => {
                let bytes = target_bytes(item.target, &item.canonical_path)?
                    .expect("canonical source is present for install");
                let hash = sha256_hex(&bytes);
                match write_target(&item.target_path, &bytes) {
                    Ok(()) => {
                        state
                            .installed_mut(item.target)
                            .insert(item.filename.clone(), hash);
                        outcomes.push(ApplyOutcome {
                            filename: item.filename.clone(),
                            action: match item.status {
                                SyncStatus::NotInstalled => "installed".to_string(),
                                _ => "updated".to_string(),
                            },
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
            SyncStatus::UpToDate => {
                if let Some(hash) = &item.canonical_hash {
                    state
                        .installed_mut(item.target)
                        .insert(item.filename.clone(), hash.clone());
                }
                outcomes.push(ApplyOutcome {
                    filename: item.filename.clone(),
                    action: "kept".to_string(),
                    detail: "already in sync".to_string(),
                    ok: true,
                });
            }
            SyncStatus::Remove => match fs::remove_file(&item.target_path) {
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
            },
            _ => unreachable!(),
        }
    }
    // Manifest cleanup is scoped to the targets the caller actually
    // planned. A session bound to one harness never sees its manifest
    // pruned by cleanup logic that walked the other target.
    for target in targets_touched {
        let target_dir = target.dir(paths);
        state.installed_mut(target).retain(|name, _| {
            target_dir.join(name).exists() || paths.canonical_dir.join(name).exists()
        });
    }
    if state != original_state {
        write_state(&paths.state_file, &state)?;
    }
    Ok((state, outcomes))
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
    let canonical_bytes =
        target_bytes(target, &canonical_path)?.expect("canonical source exists after stat");
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
