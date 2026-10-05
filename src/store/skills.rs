//! Checked-out `skills/` directory sync into the OpenCode global skills dir.
//!
//! agenthd installs the configured checkout's `skills/<name>/` subdirectories
//! into `Paths.skills_dir/<name>/` (the OpenCode global skills root).
//! `pi-psql` and any other third-party catalog tool remains an explicit
//! third-party installer and is **never** scanned, recursed, adopted, or
//! removed by this module.
//!
//! Source model
//! ------------
//!
//! - Source root: `<configured checkout>/skills/`. Each immediate child that
//!   contains a `SKILL.md` file is one skill.
//! - The root is validated up front: it must be a real, regular directory
//!   (no symlinks). A missing or invalid root fails closed with an explicit
//!   error. The configured checkout itself is already validated by
//!   `super::require_canonical_source`; this module additionally insists the
//!   `skills/` subdirectory is real and regular before any read.
//! - Source entries are walked recursively. Symlinks anywhere in the tree
//!   are refused with a per-entry error: the install path copies bytes, not
//!   indirections. Every entry must be either a regular file or a regular
//!   directory.
//!
//! Identity
//! --------
//!
//! - Each skill's `SKILL.md` frontmatter must parse, must contain a
//!   top-level `name:` scalar, and the value must equal the skill directory
//!   name. A mismatch is an explicit error surfaced in the per-item
//!   `outcome.detail`; the install does not silently rename.
//!
//! Ownership
//! ---------
//!
//! - `State.installed_skills` maps `<skill-dir-name>` to a record with
//!   `tree_hash` (deterministic whole-tree SHA-256, see `compute_tree_hash`)
//!   and `skill_name` (the verified identity string from `SKILL.md`).
//! - The field is `#[serde(default)]` so older `state.json` files (which
//!   lack the key) load cleanly into the default empty map. This preserves
//!   the existing per-machine state compatibility contract.
//! - Ownership is intentionally per-target: there is only one OpenCode
//!   global skills directory; Pi does not consume the same skill format.
//!
//! Plan
//! ----
//!
//! `plan` is read-only: it observes the source tree, the destination, and
//! the ownership manifest and produces a per-skill `SkillPlanItem` with
//! `action: SkillAction`. The plan is reproducible: deterministic walk
//! order (sorted names), deterministic relative path encoding (`/`
//! separator), deterministic whole-tree hash.
//!
//! Scratch root
//! ------------
//!
//! All staging (`Install`/`Update`) and backup (`Update`/`Remove`)
//! directories are created under a sibling of `Paths.skills_dir`
//! inside the opencode config root, e.g.
//! `<xdg|home>/.config/opencode/.agenthd-staging.<pid>.<n>` and
//! `<xdg|home>/.config/opencode/.agenthd-backup.<pid>.<n>`. The
//! prefix `.agenthd-` scopes the temp directories to this tool so
//! other tools sharing the opencode config root can identify and
//! ignore them. OpenCode's skills scanner walks the entire
//! `skills_dir` subtree looking for `SKILL.md` files at any depth;
//! keeping scratch directories OUTSIDE that subtree ensures a
//! half-staged tree or a retained backup cannot be discovered as a
//! misformed or stale skill. `scratch_root` fails closed when
//! `target.parent().parent()` cannot be derived (e.g. a target
//! directly under the filesystem root).
//!
//! Apply
//! -----
//!
//! `apply` walks the plan item-by-item and dispatches on `action`:
//!
//! - `Install`: target must be absent OR byte-identical to the planned
//!   source tree (in which case we adopt ownership without rewriting). Any
//!   other state at the target (regular file, regular directory with a
//!   different tree hash, symlink) is a conflict and the entry is left
//!   alone.
//! - `Update`: target's whole-tree hash must equal the manifest's recorded
//!   `tree_hash`. On a match the source tree is staged into a scratch
//!   directory SIBLING TO the skills dir (under the opencode config
//!   root), a backup of the existing target is taken in the same scratch
//!   root, the target is removed, and the staged tree is renamed into
//!   place via the OS no-replace primitive. The two renames are
//!   individually atomic (renameat2/MoveFileW); the swap as a whole is
//!   NOT atomic — the brief window between them has the target absent.
//!   Staging/backup directories deliberately live OUTSIDE
//!   `Paths.skills_dir` so an interrupted stage cannot expose a
//!   misformed `SKILL.md` tree to the OpenCode skills scanner. On any
//!   failure path the backup is restored so the user's installed skill
//!   directory survives an interrupted update.
//! - `UpToDate`: target hash equals the source hash and the manifest
//!   already records it. The manifest entry's `tree_hash` is re-inserted
//!   (refresh) — no filesystem write.
//! - `Remove`: source is absent and the manifest records ownership. The
//!   target's whole-tree hash must still equal the recorded `tree_hash`
//!   (no external edits). On match the target is removed and the manifest
//!   entry dropped. On mismatch the entry is preserved as
//!   `PreserveModified` (drop ownership only, do not touch the user's
//!   modifications).
//! - `Ghost`: source absent, manifest records ownership, but target also
//!   absent — clean up the manifest entry, no filesystem write.
//! - `Conflict`: explicit non-action; never auto-overwrite. A third-party
//!   skill (e.g. `pi-psql` installed via the Tools installer) that the
//!   configured checkout's `skills/` does not contain is invisible to
//!   this module and remains untouched.
//!
//! Per-item revalidation: at apply time, before any mutation, the live
//! filesystem state of the source and target is re-checked against the
//! plan snapshot. A drifted snapshot produces an `error` outcome and the
//! loop continues; sibling successes are not blocked.
//!
//! TOCTOU acknowledgement: between revalidation and the publish step the
//! target tree could change on a competing process. The publish uses the
//! OS no-replace primitive (scratch-root temp dir + `renameat2` /
//! `MoveFileW`) so each individual rename is atomic on each platform.
//! The `Update` swap as a whole is two atomic renames, not an atomic
//! compare-and-swap: there is a brief window between step 2 (target
//! into backup) and step 3 (staging into target) during which the
//! target path is absent. Recovery paths always either restore the
//! previous tree or report a retained backup. The bytes that land in
//! place are exactly the bytes the publish step staged. The `plan` ->
//! `apply` round trip is the natural retry boundary for the user.
//!
//! pi-psql protection
//! ------------------
//!
//! `pi-psql` (and any other Tools-catalog-installed skill) is explicitly
//! excluded: this module never scans, recurses, adopts, or removes
//! `Paths.skills_dir/pi-psql` unless that directory is also present in
//! the configured checkout's `skills/` source AND already recorded in the
//! ownership manifest. The destination-side scan only ever names
//! directories that are either (a) in the configured checkout source or
//! (b) in the ownership manifest. A `pi-psql` directory that is NOT in
//! source AND NOT in the manifest is invisible to this module — it is
//! left alone on disk exactly as the Tools installer placed it.
//!
//! Public surface
//! --------------
//!
//! - `SkillPlanItem`, `SkillAction`, `SkillOutcome`, `OwnedSkill`
//! - `plan(paths, state) -> Vec<SkillPlanItem>` (read-only)
//! - `apply(paths, state, items) -> (State, Vec<SkillOutcome>)`
//!
//! Internal helpers stay `pub(super)` or private so the only caller
//! surface is `plan` + `apply` plus the type re-exports.

use super::{sha256_hex, write_state, Paths, State};
use crate::operation::{run_cancel_checked, CancelToken, Finish, OperationReport, Progress};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

// ---- Public types ------------------------------------------------------------

/// One row of the Skills sync plan. The plan is read-only and
/// deterministic; `apply` consumes a `Vec<SkillPlanItem>` and dispatches
/// on `action`.
#[derive(Debug, Clone)]
pub struct SkillPlanItem {
    pub name: String,
    pub action: SkillAction,
    /// Verified identity parsed from the source `SKILL.md` `name:`
    /// scalar, when the source was readable. `None` for source-absent
    /// items where the SKILL.md could not be read.
    pub skill_name: Option<String>,
    /// Whole-tree SHA-256 of the source tree under
    /// `<source>/<name>/` (regular files only, deterministic order).
    /// `None` when the source is absent.
    pub source_tree_hash: Option<String>,
    /// Whole-tree SHA-256 of the destination tree under
    /// `<skills_dir>/<name>/` (regular files only, deterministic order).
    /// `None` when the destination is absent.
    pub target_tree_hash: Option<String>,
    /// The recorded `tree_hash` in the ownership manifest, if any.
    pub owned_tree_hash: Option<String>,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
}

/// Classification of a skill for the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAction {
    Install,
    /// Destination already matches source byte-for-byte, manifest has no
    /// entry. Adopt ownership without rewriting.
    Adopt,
    /// Source differs from manifest's recorded hash; destination still
    /// matches the manifest. Refresh in place.
    Update,
    /// Source, destination, and manifest agree; no filesystem write.
    UpToDate,
    /// Source absent, manifest records ownership, destination still
    /// matches. Safe to delete.
    Remove,
    /// Source absent, manifest records ownership, destination differs
    /// from manifest (external edit). Drop ownership only.
    PreserveModified,
    /// Source absent, manifest records ownership, destination also
    /// absent. Clean up the manifest entry; no filesystem action.
    Ghost,
    /// Any other state — destination is unowned with different bytes,
    /// destination is a symlink or nonregular entry, etc. The installer
    /// does NOT auto-overwrite conflicts.
    Conflict,
}

impl SkillAction {
    pub fn label(&self) -> &'static str {
        match self {
            SkillAction::Install => "install",
            SkillAction::Adopt => "adopt",
            SkillAction::Update => "update",
            SkillAction::UpToDate => "up to date",
            SkillAction::Remove => "remove",
            SkillAction::PreserveModified => "preserve modified",
            SkillAction::Ghost => "ghost",
            SkillAction::Conflict => "conflict",
        }
    }
}

/// Per-skill ownership record. `tree_hash` is the deterministic
/// whole-tree SHA-256 of the source bytes that were installed; `skill_name`
/// is the verified identity from `SKILL.md`. Both are required so the
/// installer can detect identity drift (e.g. a skill whose `SKILL.md` was
/// edited under ownership to advertise a different `name:`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedSkill {
    pub tree_hash: String,
    pub skill_name: String,
}

/// Per-skill summary from `apply`. `ok = true` includes intentional
/// non-actions (a `Conflict` produces `skipped` with `ok = true` because
/// the refusal is the contract). `ok = false` is reserved for actual
/// filesystem or revalidation failures.
#[derive(Debug, Clone)]
pub struct SkillOutcome {
    pub name: String,
    pub action: String,
    pub detail: String,
    pub ok: bool,
}

// ---- Name validation --------------------------------------------------------

/// Validate that `name` is exactly one normal directory component:
/// no path separators, no `..`, no NULs. The manifest, source
/// directory listing, and per-skill plan items all flow through
/// this check before being joined into any filesystem path so a
/// crafted manifest entry cannot escape `paths.skills_dir` or the
/// configured checkout's `skills/` root.
fn validate_skill_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("empty skill name".to_string());
    }
    if name.contains('\0') {
        return Err(format!("skill name {name:?} contains NUL"));
    }
    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(format!(
            "skill name `{name}` must be exactly one directory component"
        )),
    }
}

/// Derive `<checkout>/skills/<name>` for the configured checkout.
/// Used by both `plan` and `apply` so the derived path is the
/// single source of truth for what a skill's source location
/// *should* be.
fn derived_source_path(paths: &Paths, name: &str) -> std::result::Result<PathBuf, String> {
    validate_skill_name(name)?;
    let checkout_root = paths.canonical_dir.parent().ok_or_else(|| {
        format!(
            "canonical source `{}` has no parent directory",
            paths.canonical_dir.display()
        )
    })?;
    Ok(checkout_root.join("skills").join(name))
}

/// Derive `<skills_dir>/<name>`.
fn derived_target_path(paths: &Paths, name: &str) -> std::result::Result<PathBuf, String> {
    validate_skill_name(name)?;
    Ok(paths.skills_dir.join(name))
}

// ---- Internal helpers --------------------------------------------------------

struct FileEntry {
    rel: String,
    bytes: Vec<u8>,
}

/// Validate the configured checkout's `skills/` directory is real,
/// regular, and not a symlink. The configured checkout's
/// `agents/` is the source of truth for agents; the `skills/`
/// directory is a sibling under the configured checkout root
/// (i.e. `<checkout>/skills/`, NOT `<checkout>/agents/skills/`).
pub(super) fn require_skills_source(paths: &Paths) -> Result<PathBuf> {
    super::require_canonical_source(paths)?;
    // The canonical agents dir is `<checkout>/agents`. Its parent is
    // the configured checkout root, which carries the `skills/`
    // sibling. We refuse if the parent can't be derived.
    let checkout_root = paths.canonical_dir.parent().ok_or_else(|| {
        anyhow!(
            "canonical source `{}` has no parent directory",
            paths.canonical_dir.display()
        )
    })?;
    let skills_root = checkout_root.join("skills");
    match fs::symlink_metadata(&skills_root) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                bail!(
                    "skills source `{}` is a symlink; refusing to follow links",
                    skills_root.display()
                );
            }
            if !meta.is_dir() {
                bail!(
                    "skills source `{}` is not a directory",
                    skills_root.display()
                );
            }
            Ok(skills_root)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "skills source `{}` does not exist; the configured checkout must contain a `skills/` directory",
            skills_root.display()
        ),
        Err(e) => Err(anyhow!("stat {}: {}", skills_root.display(), e)),
    }
}

/// Recursively read a tree of regular files. Symlinks and other
/// non-regular entries are refused.
fn read_tree(root: &Path) -> Result<Vec<FileEntry>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry
                .file_type()
                .with_context(|| format!("file_type {}", path.display()))?;
            if meta.is_symlink() {
                bail!(
                    "{} contains a symlink; refusing to scan symlinked entries",
                    path.display()
                );
            }
            let rel = path
                .strip_prefix(root)
                .map_err(|e| anyhow!("strip_prefix {}: {}", path.display(), e))?
                .components()
                .map(|c| match c {
                    Component::Normal(s) => Ok(s.to_string_lossy().into_owned()),
                    other => Err(anyhow!(
                        "non-normal path component {:?} in {}",
                        other,
                        path.display()
                    )),
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            if meta.is_dir() {
                walk(root, &path, out)?;
            } else if meta.is_file() {
                let mut bytes = Vec::new();
                fs::File::open(&path)
                    .with_context(|| format!("open {}", path.display()))?
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("read {}", path.display()))?;
                out.push(FileEntry { rel, bytes });
            } else {
                bail!("{} is not a regular file or directory", path.display());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// Whole-tree SHA-256: hash the concatenation of relative-path + bytes
/// pairs in deterministic order. Including the relative path makes the
/// value sensitive to file *location* (a rename produces a different
/// hash, matching the reality that the tree contents changed).
fn compute_tree_hash(entries: &[FileEntry]) -> String {
    let mut hasher = sha2::Sha256::new();
    for entry in entries {
        hasher.update(entry.rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(&entry.bytes);
    }
    sha256_hex(&hasher.finalize())
}

/// Read and validate `SKILL.md` in `dir`. Returns the parsed `name:`
/// scalar.
fn read_skill_name(dir: &Path) -> Result<String> {
    let skill_md = dir.join("SKILL.md");
    let bytes = match fs::read(&skill_md) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("missing SKILL.md at {}", skill_md.display())
        }
        Err(e) => return Err(anyhow!("read {}: {}", skill_md.display(), e)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        anyhow!(
            "SKILL.md at {} is not valid UTF-8: {}",
            skill_md.display(),
            e
        )
    })?;
    let name = crate::tools::parse_skill_name(text).ok_or_else(|| {
        anyhow!(
            "SKILL.md at {} has no parseable `name:` in frontmatter",
            skill_md.display()
        )
    })?;
    Ok(name)
}

struct SourceSkill {
    source_path: PathBuf,
    tree_hash: String,
    skill_name: String,
}

/// Read all source skills. Returns an empty map when the source has no
/// skill subdirectories with a valid `SKILL.md`. Refuses entries whose
/// `SKILL.md` `name:` does not match the directory name.
fn read_source_skills(paths: &Paths) -> Result<BTreeMap<String, SourceSkill>> {
    let root = require_skills_source(paths)?;
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read_dir {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.file_type()?;
        if meta.is_symlink() {
            bail!(
                "{} is a symlink; refusing to scan symlinked skills",
                path.display()
            );
        }
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        validate_skill_name(&name).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        let entries = read_tree(&path)?;
        let tree_hash = compute_tree_hash(&entries);
        let skill_name = read_skill_name(&path)?;
        if skill_name != name {
            bail!(
                "{} directory name `{}` does not match SKILL.md `name: {}`",
                path.display(),
                name,
                skill_name
            );
        }
        out.insert(
            name.clone(),
            SourceSkill {
                source_path: path,
                tree_hash,
                skill_name,
            },
        );
    }
    Ok(out)
}

enum DestinationState {
    Absent,
    NotRegular { detail: String },
    Present { tree_hash: String },
}

fn inspect_destination(target: &Path) -> DestinationState {
    match fs::symlink_metadata(target) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                DestinationState::NotRegular {
                    detail: format!("{} is a symlink", target.display()),
                }
            } else if meta.is_dir() {
                match read_tree(target) {
                    Ok(entries) => DestinationState::Present {
                        tree_hash: compute_tree_hash(&entries),
                    },
                    Err(e) => DestinationState::NotRegular {
                        detail: e.to_string(),
                    },
                }
            } else {
                DestinationState::NotRegular {
                    detail: format!("{} is not a directory", target.display()),
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DestinationState::Absent,
        Err(e) => DestinationState::NotRegular {
            detail: format!("stat {}: {}", target.display(), e),
        },
    }
}

/// Build the read-only sync plan.
pub fn plan(paths: &Paths, state: &State) -> Result<Vec<SkillPlanItem>> {
    let source_skills = read_source_skills(paths)?;
    let mut names: BTreeSet<String> = BTreeSet::new();
    for n in source_skills.keys() {
        validate_skill_name(n)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("refusing source skill name `{n}` from checkout"))?;
        names.insert(n.clone());
    }
    for n in state.installed_skills.keys() {
        validate_skill_name(n)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("refusing manifest skill name `{n}`"))?;
        names.insert(n.clone());
    }

    let mut items = Vec::new();
    for name in names {
        let source = source_skills.get(&name);
        let owned = state.installed_skills.get(&name).cloned();
        let target_path = derived_target_path(paths, &name)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("refusing manifest target name `{name}`"))?;
        let destination = inspect_destination(&target_path);
        items.push(classify(
            paths,
            &name,
            source,
            owned.as_ref(),
            destination,
            target_path,
        ));
    }
    Ok(items)
}

fn classify(
    paths: &Paths,
    name: &str,
    source: Option<&SourceSkill>,
    owned: Option<&OwnedSkill>,
    destination: DestinationState,
    target_path: PathBuf,
) -> SkillPlanItem {
    // Derive the source path from the checkout root so the plan
    // reflects the same single source of truth `apply` uses. When
    // the source is genuinely absent (manifest-only entry),
    // `source` is `None` and the derived path is the only path
    // the plan reports — `apply` then re-validates absence on
    // disk.
    let source_path = source
        .map(|s| s.source_path.clone())
        .or_else(|| derived_source_path(paths, name).ok());
    let source_tree_hash = source.map(|s| s.tree_hash.clone());
    let skill_name = source.map(|s| s.skill_name.clone());
    let owned_tree_hash = owned.map(|o| o.tree_hash.clone());
    let target_tree_hash = match &destination {
        DestinationState::Present { tree_hash } => Some(tree_hash.clone()),
        _ => None,
    };
    let action = match (source, owned, &destination) {
        (Some(_), _, DestinationState::NotRegular { .. }) => SkillAction::Conflict,
        (Some(_), None, DestinationState::Absent) => SkillAction::Install,
        (Some(src), None, DestinationState::Present { tree_hash }) => {
            if *tree_hash == src.tree_hash {
                SkillAction::Adopt
            } else {
                SkillAction::Conflict
            }
        }
        (Some(_), Some(_), DestinationState::Absent) => SkillAction::Install,
        (Some(src), Some(own), DestinationState::Present { tree_hash }) => {
            if *tree_hash == src.tree_hash {
                SkillAction::UpToDate
            } else if *tree_hash == own.tree_hash {
                SkillAction::Update
            } else {
                SkillAction::Conflict
            }
        }
        (None, Some(_), DestinationState::Absent) => SkillAction::Ghost,
        (None, Some(_), DestinationState::NotRegular { .. }) => SkillAction::Conflict,
        (None, Some(own), DestinationState::Present { tree_hash }) => {
            if *tree_hash == own.tree_hash {
                SkillAction::Remove
            } else {
                SkillAction::PreserveModified
            }
        }
        (None, None, _) => SkillAction::Conflict,
    };
    SkillPlanItem {
        name: name.to_string(),
        action,
        skill_name,
        source_tree_hash,
        target_tree_hash,
        owned_tree_hash,
        source_path: source_path.unwrap_or_default(),
        target_path,
    }
}

// ---- Apply ------------------------------------------------------------------

pub fn apply(
    paths: &Paths,
    mut state: State,
    items: Vec<SkillPlanItem>,
) -> Result<(State, Vec<SkillOutcome>)> {
    // Always validate the source root up front. The plan already
    // did this once; we re-validate to catch a source root that
    // was removed or symlinked out between plan and apply. A
    // missing source root is an explicit error regardless of the
    // plan's composition — even an all-Ghost plan must surface
    // a missing source root so the user can fix the configured
    // checkout instead of silently churning the manifest.
    require_skills_source(paths)?;

    let mut outcomes = Vec::new();
    let original_state = state.clone();
    for item in items {
        outcomes.push(apply_one(paths, &mut state, item));
    }
    // Manifest persistence is scoped to actual changes so a
    // fully failed apply pass does not rewrite `state.json` with
    // an unchanged value.
    if state != original_state {
        write_state(&paths.state_file, &state)?;
    }
    Ok((state, outcomes))
}

/// Apply all skill actions from the plan with cooperative
/// cancellation and progress reporting.
///
/// Cancel checkpoints are at the row boundary only: before each
/// row's `apply_one` (so a pre-cancel pass writes nothing for the
/// row) and after each row commits (so a successful row is
/// persisted before reporting the safe checkpoint). The row
/// itself is **non-cancellable**: the Skills `Update` path runs
/// the staged-tree backup/rename/recovery as one unit, and
/// tearing it down mid-row would leave the install in a state
/// the recovery path cannot roll back. The contract is the same
/// as the spec: "No cancel inside skills two rename
/// backup/publication/recovery".
///
/// Persistence: the row path persists exactly once per row (when
/// a row actually changed `state`). The final legacy-style
/// `write_state` is replaced by a per-row write that fires when
/// `state != last_persisted`, so a cancelled run still has every
/// completed row on disk. Manifest persistence failure is
/// surfaced as `Finish::Failed` with the partial state and the
/// explicit "manifest may have changed" message; no rollback
/// and no further rows are run.
pub fn apply_controlled(
    paths: &Paths,
    mut state: State,
    items: Vec<SkillPlanItem>,
    token: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> OperationReport<(State, Vec<SkillOutcome>)> {
    if let Err(e) = require_skills_source(paths) {
        return OperationReport {
            finish: Finish::Failed,
            partial: (state, Vec::new()),
            error: Some(e.to_string()),
        };
    }
    let total = items.len();
    let mut outcomes = Vec::new();
    let mut last_persisted = state.clone();
    for (idx, item) in items.into_iter().enumerate() {
        if run_cancel_checked(
            token,
            sink,
            Progress {
                stage: "skills: row",
                item: Some(item.name.clone()),
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
        let outcome = apply_one(paths, &mut state, item);
        outcomes.push(outcome);
        if state != last_persisted {
            if let Err(e) = write_state(&paths.state_file, &state) {
                return OperationReport {
                    finish: Finish::Failed,
                    partial: (state, outcomes),
                    error: Some(format!(
                        "skills manifest write failed after row; manifest may have changed and was not saved: {}",
                        e
                    )),
                };
            }
            last_persisted = state.clone();
        }
    }
    sink(Progress {
        stage: "skills: row",
        item: None,
        processed: total,
        total: Some(total),
    });
    OperationReport {
        finish: Finish::Completed,
        partial: (state, outcomes),
        error: None,
    }
}

/// Result of per-item source-side revalidation. `Present` carries
/// the live `tree_hash`, the verified `SKILL.md` identity, and
/// the file bytes the installer is about to publish; `Absent`
/// means the live source path is genuinely missing; `NotRegular`
/// records the live observation for fail-closed branches.
enum SourceLive {
    Present {
        tree_hash: String,
        skill_name: String,
        entries: Vec<FileEntry>,
    },
    Absent,
    NotRegular {
        detail: String,
    },
}

/// Re-validate the source path the planner used. Always re-derives
/// the source path from the configured checkout so a hand-crafted
/// `SkillPlanItem` cannot point at a path outside the configured
/// checkout's `skills/` directory.
fn revalidate_source(paths: &Paths, name: &str) -> std::result::Result<SourceLive, String> {
    validate_skill_name(name).map_err(|e| format!("refusing plan name `{name}`: {e}"))?;
    let derived = derived_source_path(paths, name)
        .map_err(|e| format!("cannot derive source path for `{name}`: {e}"))?;
    match fs::symlink_metadata(&derived) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Ok(SourceLive::NotRegular {
                    detail: format!(
                        "{} became a symlink after plan; refresh to re-plan",
                        derived.display()
                    ),
                });
            }
            if !meta.is_dir() {
                return Ok(SourceLive::NotRegular {
                    detail: format!(
                        "{} is no longer a directory after plan; refresh to re-plan",
                        derived.display()
                    ),
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SourceLive::Absent),
        Err(e) => return Err(format!("stat {}: {}", derived.display(), e)),
    }
    // Source is a regular directory: read its tree and verify
    // the SKILL.md identity still matches the directory name.
    let entries = read_tree(&derived).map_err(|e| e.to_string())?;
    let tree_hash = compute_tree_hash(&entries);
    let skill_name = read_skill_name(&derived).map_err(|e| e.to_string())?;
    if skill_name != name {
        return Err(format!(
            "{}: SKILL.md `name: {}` no longer matches directory; refresh to re-plan",
            derived.display(),
            skill_name
        ));
    }
    Ok(SourceLive::Present {
        tree_hash,
        skill_name,
        entries,
    })
}

/// Result of per-item target-side revalidation. Mirrors the
/// `DestinationState` enum used at plan time but is the live,
/// post-revalidation observation.
#[derive(Debug)]
enum TargetLive {
    Absent,
    NotRegular { detail: String },
    Present { tree_hash: String },
}

fn revalidate_target(target_path: &Path) -> TargetLive {
    match inspect_destination(target_path) {
        DestinationState::Absent => TargetLive::Absent,
        DestinationState::NotRegular { detail } => TargetLive::NotRegular { detail },
        DestinationState::Present { tree_hash } => TargetLive::Present { tree_hash },
    }
}

fn apply_one(paths: &Paths, state: &mut State, item: SkillPlanItem) -> SkillOutcome {
    let SkillPlanItem {
        name,
        action,
        skill_name,
        source_tree_hash,
        target_tree_hash,
        owned_tree_hash,
        source_path,
        target_path,
    } = item;

    // Validate the supplied name; cross-check that the item's
    // source/target paths equal the paths the planner/apply
    // derive, so a hand-crafted `SkillPlanItem` cannot smuggle a
    // traversal path into the installer.
    if let Err(detail) = validate_skill_name(&name) {
        return SkillOutcome {
            name,
            action: "error".to_string(),
            detail,
            ok: false,
        };
    }
    let expected_source = match derived_source_path(paths, &name) {
        Ok(p) => p,
        Err(detail) => {
            return SkillOutcome {
                name,
                action: "error".to_string(),
                detail,
                ok: false,
            }
        }
    };
    let expected_target = match derived_target_path(paths, &name) {
        Ok(p) => p,
        Err(detail) => {
            return SkillOutcome {
                name,
                action: "error".to_string(),
                detail,
                ok: false,
            }
        }
    };
    if source_path != expected_source {
        return SkillOutcome {
            name: name.clone(),
            action: "error".to_string(),
            detail: format!(
                "plan source path `{}` does not match derived `{}`; refusing crafted path",
                source_path.display(),
                expected_source.display()
            ),
            ok: false,
        };
    }
    if target_path != expected_target {
        return SkillOutcome {
            name: name.clone(),
            action: "error".to_string(),
            detail: format!(
                "plan target path `{}` does not match derived `{}`; refusing crafted path",
                target_path.display(),
                expected_target.display()
            ),
            ok: false,
        };
    }

    // Manifest drift check: if the plan was built when the
    // manifest recorded a particular `owned_tree_hash`, the live
    // manifest must still record the same value. A drift means
    // another process touched the manifest, and we must refuse to
    // honor the stale plan.
    let manifest_hash = state
        .installed_skills
        .get(&name)
        .map(|o| o.tree_hash.clone());
    if owned_tree_hash.as_ref() != manifest_hash.as_ref() {
        return SkillOutcome {
            name,
            action: "error".to_string(),
            detail: "manifest drifted between plan and apply; refresh to re-plan".to_string(),
            ok: false,
        };
    }

    // Per-item live source/target revalidation.
    let source = match revalidate_source(paths, &name) {
        Ok(s) => s,
        Err(detail) => {
            return SkillOutcome {
                name,
                action: "error".to_string(),
                detail,
                ok: false,
            }
        }
    };
    let target = revalidate_target(&target_path);

    let outcome = match action {
        SkillAction::Install => install_outcome(
            &name,
            &source,
            source_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::Adopt => adopt_outcome(
            &name,
            &source,
            source_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::Update => update_outcome(
            &name,
            &source,
            source_tree_hash.as_ref(),
            owned_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::UpToDate => uptodate_outcome(
            &name,
            &source,
            source_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            owned_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::Remove => remove_outcome(
            &name,
            &source,
            owned_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::PreserveModified => preserve_modified_outcome(
            &name,
            &source,
            owned_tree_hash.as_ref(),
            target_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::Ghost => ghost_outcome(
            &name,
            &source,
            owned_tree_hash.as_ref(),
            &target_path,
            target,
            state,
        ),
        SkillAction::Conflict => conflict_outcome(&name, &target_path, target),
    };
    let _ = (skill_name, target_tree_hash);
    outcome
}

fn install_outcome(
    name: &str,
    source: &SourceLive,
    source_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    let src = match source {
        SourceLive::Present {
            tree_hash,
            skill_name,
            entries,
        } => (tree_hash, skill_name, entries),
        SourceLive::Absent => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: format!(
                    "{} source vanished after plan; refresh to re-plan",
                    target_path.display()
                ),
                ok: false,
            };
        }
        SourceLive::NotRegular { detail } => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: detail.clone(),
                ok: false,
            };
        }
    };
    let (live_source_hash, live_skill_name, entries) = src;
    if Some(live_source_hash) != source_tree_hash_plan {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source changed since plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    match target {
        TargetLive::Absent => match stage_and_publish(entries, target_path) {
            Ok(()) => {
                state.installed_skills.insert(
                    name.to_string(),
                    OwnedSkill {
                        tree_hash: live_source_hash.clone(),
                        skill_name: live_skill_name.clone(),
                    },
                );
                SkillOutcome {
                    name: name.to_string(),
                    action: "installed".to_string(),
                    detail: format!("installed {}", target_path.display()),
                    ok: true,
                }
            }
            Err(detail) => SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail,
                ok: false,
            },
        },
        TargetLive::Present { tree_hash } => {
            if Some(&tree_hash) != target_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target changed since plan; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if &tree_hash != live_source_hash {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "skipped".to_string(),
                    detail: format!(
                        "{} already exists with different bytes; refusing to overwrite",
                        target_path.display()
                    ),
                    ok: true,
                };
            }
            // Safe adoption: byte-identical unowned tree.
            state.installed_skills.insert(
                name.to_string(),
                OwnedSkill {
                    tree_hash,
                    skill_name: live_skill_name.clone(),
                },
            );
            SkillOutcome {
                name: name.to_string(),
                action: "adopted".to_string(),
                detail: format!("adopted existing {}", target_path.display()),
                ok: true,
            }
        }
        TargetLive::NotRegular { detail } => SkillOutcome {
            name: name.to_string(),
            action: "skipped".to_string(),
            detail,
            ok: true,
        },
    }
}

fn adopt_outcome(
    name: &str,
    source: &SourceLive,
    source_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    let src = match source {
        SourceLive::Present {
            tree_hash,
            skill_name,
            ..
        } => (tree_hash, skill_name),
        SourceLive::Absent => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: format!(
                    "{} source vanished after plan; refresh to re-plan",
                    target_path.display()
                ),
                ok: false,
            };
        }
        SourceLive::NotRegular { detail } => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: detail.clone(),
                ok: false,
            };
        }
    };
    let (live_source_hash, live_skill_name) = src;
    if Some(live_source_hash) != source_tree_hash_plan {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source changed since plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    if state.installed_skills.contains_key(name) {
        return SkillOutcome {
            name: name.to_string(),
            action: "skipped".to_string(),
            detail: "manifest already records ownership; refresh to re-plan".to_string(),
            ok: true,
        };
    }
    match target {
        TargetLive::Present { tree_hash } => {
            if Some(&tree_hash) != target_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target changed since plan; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if tree_hash == *live_source_hash {
                state.installed_skills.insert(
                    name.to_string(),
                    OwnedSkill {
                        tree_hash,
                        skill_name: live_skill_name.clone(),
                    },
                );
                SkillOutcome {
                    name: name.to_string(),
                    action: "adopted".to_string(),
                    detail: format!("adopted existing {}", target_path.display()),
                    ok: true,
                }
            } else {
                SkillOutcome {
                    name: name.to_string(),
                    action: "skipped".to_string(),
                    detail: format!(
                        "{} no longer matches source; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: true,
                }
            }
        }
        TargetLive::Absent => SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} disappeared after plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        },
        TargetLive::NotRegular { detail } => SkillOutcome {
            name: name.to_string(),
            action: "skipped".to_string(),
            detail,
            ok: true,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn update_outcome(
    name: &str,
    source: &SourceLive,
    source_tree_hash_plan: Option<&String>,
    owned_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    let entries = match source {
        SourceLive::Present {
            tree_hash,
            skill_name,
            entries,
        } => (tree_hash, skill_name, entries),
        SourceLive::Absent => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: format!(
                    "{} source vanished after plan; refresh to re-plan",
                    target_path.display()
                ),
                ok: false,
            };
        }
        SourceLive::NotRegular { detail } => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: detail.clone(),
                ok: false,
            };
        }
    };
    let (live_source_hash, live_skill_name, entries) = entries;
    if Some(live_source_hash) != source_tree_hash_plan {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source changed since plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    let Some(owned_hash) = owned_tree_hash_plan else {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: "manifest drifted between plan and apply; refresh to re-plan".to_string(),
            ok: false,
        };
    };
    let TargetLive::Present { tree_hash } = target else {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} vanished after plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    };
    if Some(&tree_hash) != target_tree_hash_plan {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} target changed since plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    if &tree_hash != owned_hash {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} target drifted from owned hash; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    match replace_owned_tree(target_path, entries) {
        ReplaceOutcome::Published { retained_backup } => {
            state.installed_skills.insert(
                name.to_string(),
                OwnedSkill {
                    tree_hash: live_source_hash.clone(),
                    skill_name: live_skill_name.clone(),
                },
            );
            let detail = match retained_backup {
                Some(backup) => format!(
                    "updated {} (old tree retained at {}; manual cleanup may be required)",
                    target_path.display(),
                    backup.display()
                ),
                None => format!("updated {}", target_path.display()),
            };
            SkillOutcome {
                name: name.to_string(),
                action: "updated".to_string(),
                detail,
                ok: true,
            }
        }
        ReplaceOutcome::Failed { detail } => SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail,
            ok: false,
        },
        ReplaceOutcome::TargetMissing => SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} vanished after plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn uptodate_outcome(
    name: &str,
    source: &SourceLive,
    source_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    owned_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    let src = match source {
        SourceLive::Present {
            tree_hash,
            skill_name,
            ..
        } => (tree_hash, skill_name),
        SourceLive::Absent => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: format!(
                    "{} source vanished after plan; refresh to re-plan",
                    target_path.display()
                ),
                ok: false,
            };
        }
        SourceLive::NotRegular { detail } => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: detail.clone(),
                ok: false,
            };
        }
    };
    let (live_source_hash, live_skill_name) = src;
    if Some(live_source_hash) != source_tree_hash_plan {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source changed since plan; refresh to re-plan",
                target_path.display()
            ),
            ok: false,
        };
    }
    match target {
        TargetLive::Present { tree_hash } => {
            if Some(&tree_hash) != target_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target changed since plan; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if Some(&tree_hash) != owned_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} manifest drifted from target; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if &tree_hash != live_source_hash {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} source and target hashes disagree on revalidation; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
        }
        TargetLive::Absent => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail: format!(
                    "{} target vanished after plan; refresh to re-plan",
                    target_path.display()
                ),
                ok: false,
            };
        }
        TargetLive::NotRegular { detail } => {
            return SkillOutcome {
                name: name.to_string(),
                action: "error".to_string(),
                detail,
                ok: false,
            };
        }
    }
    state.installed_skills.insert(
        name.to_string(),
        OwnedSkill {
            tree_hash: live_source_hash.clone(),
            skill_name: live_skill_name.clone(),
        },
    );
    SkillOutcome {
        name: name.to_string(),
        action: "kept".to_string(),
        detail: "already in sync".to_string(),
        ok: true,
    }
}

fn remove_outcome(
    name: &str,
    source: &SourceLive,
    owned_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    // `Remove` requires the source to be genuinely absent.
    // Re-validate rather than trusting the plan — the source
    // may have reappeared between plan and apply.
    if let SourceLive::Present { tree_hash, .. } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source reappeared after plan; refresh to re-plan (live hash {})",
                target_path.display(),
                tree_hash
            ),
            ok: false,
        };
    }
    if let SourceLive::NotRegular { detail } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: detail.clone(),
            ok: false,
        };
    }
    let Some(owned_hash) = owned_tree_hash_plan else {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: "manifest drifted between plan and apply; refresh to re-plan".to_string(),
            ok: false,
        };
    };
    match target {
        TargetLive::Present { tree_hash } => {
            if Some(&tree_hash) != target_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target changed since plan; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if &tree_hash != owned_hash {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target drifted from owned hash; refusing to remove",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            // Safe removal: rename target into a scratch-root
            // backup directory (atomic on the same filesystem —
            // a sibling of `Paths.skills_dir` under the opencode
            // config root), then `remove_dir_all` the backup. If
            // the backup removal fails, the data is preserved at
            // a known location rather than partially deleted.
            // The manifest entry is dropped only after the rename
            // into backup succeeds — a rename failure means the
            // target tree is still on disk and ownership must
            // survive so the next plan can re-evaluate. The
            // retained backup path is reported on cleanup-only
            // failure so the user can act manually.
            match rename_then_remove(target_path) {
                Ok(None) => {
                    state.installed_skills.remove(name);
                    SkillOutcome {
                        name: name.to_string(),
                        action: "removed".to_string(),
                        detail: format!("removed {}", target_path.display()),
                        ok: true,
                    }
                }
                Ok(Some(retained)) => {
                    state.installed_skills.remove(name);
                    SkillOutcome {
                        name: name.to_string(),
                        action: "removed".to_string(),
                        detail: format!(
                            "{} removed; previous tree retained at {} (manual cleanup required)",
                            target_path.display(),
                            retained.display()
                        ),
                        ok: true,
                    }
                }
                Err(detail) => SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail,
                    ok: false,
                },
            }
        }
        TargetLive::Absent => {
            state.installed_skills.remove(name);
            SkillOutcome {
                name: name.to_string(),
                action: "removed".to_string(),
                detail: format!("already absent: {}", target_path.display()),
                ok: true,
            }
        }
        TargetLive::NotRegular { detail } => SkillOutcome {
            name: name.to_string(),
            action: "skipped".to_string(),
            detail,
            ok: true,
        },
    }
}

fn preserve_modified_outcome(
    name: &str,
    source: &SourceLive,
    owned_tree_hash_plan: Option<&String>,
    target_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    // `PreserveModified` requires the source to be genuinely
    // absent AND the target to still be present (modified). A
    // reappeared source means the plan's classification is stale;
    // a vanished target means the user's modifications are gone
    // and the manifest entry should simply be cleaned up (Ghost
    // semantics, not PreserveModified).
    if let SourceLive::Present { tree_hash, .. } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source reappeared after plan; refresh to re-plan (live hash {})",
                target_path.display(),
                tree_hash
            ),
            ok: false,
        };
    }
    if let SourceLive::NotRegular { detail } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: detail.clone(),
            ok: false,
        };
    }
    let Some(owned_hash) = owned_tree_hash_plan else {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: "manifest drifted between plan and apply; refresh to re-plan".to_string(),
            ok: false,
        };
    };
    match target {
        TargetLive::Present { tree_hash } => {
            if Some(&tree_hash) != target_tree_hash_plan {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target changed since plan; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            if tree_hash == *owned_hash {
                return SkillOutcome {
                    name: name.to_string(),
                    action: "error".to_string(),
                    detail: format!(
                        "{} target now matches owned hash; refresh to re-plan",
                        target_path.display()
                    ),
                    ok: false,
                };
            }
            state.installed_skills.remove(name);
            SkillOutcome {
                name: name.to_string(),
                action: "released".to_string(),
                detail: format!(
                    "{} was modified externally; preserved",
                    target_path.display()
                ),
                ok: true,
            }
        }
        TargetLive::Absent => {
            // The target vanished under us: there is no
            // "preserved modification" to speak of. Treat as a
            // Ghost cleanup so the manifest entry is dropped and
            // the user sees a clean state on the next plan.
            state.installed_skills.remove(name);
            SkillOutcome {
                name: name.to_string(),
                action: "cleaned".to_string(),
                detail: format!(
                    "{} vanished under PreserveModified; manifest entry dropped",
                    target_path.display()
                ),
                ok: true,
            }
        }
        TargetLive::NotRegular { detail } => SkillOutcome {
            name: name.to_string(),
            action: "skipped".to_string(),
            detail,
            ok: true,
        },
    }
}

fn ghost_outcome(
    name: &str,
    source: &SourceLive,
    owned_tree_hash_plan: Option<&String>,
    target_path: &Path,
    target: TargetLive,
    state: &mut State,
) -> SkillOutcome {
    // `Ghost` requires both the source and the target to be
    // genuinely absent. A reappeared source or a reappeared
    // target means the plan is stale and the action must not
    // happen — we error and let the user refresh.
    if let SourceLive::Present { tree_hash, .. } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} source reappeared after plan; refresh to re-plan (live hash {})",
                target_path.display(),
                tree_hash
            ),
            ok: false,
        };
    }
    if let SourceLive::NotRegular { detail } = source {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: detail.clone(),
            ok: false,
        };
    }
    if owned_tree_hash_plan.is_none() {
        return SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: "manifest drifted between plan and apply; refresh to re-plan".to_string(),
            ok: false,
        };
    }
    match target {
        TargetLive::Absent => {
            state.installed_skills.remove(name);
            SkillOutcome {
                name: name.to_string(),
                action: "cleaned".to_string(),
                detail: format!(
                    "manifest entry had no source or target: {}",
                    target_path.display()
                ),
                ok: true,
            }
        }
        TargetLive::Present { tree_hash } => SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} target reappeared after plan (live hash {}); refresh to re-plan",
                target_path.display(),
                tree_hash
            ),
            ok: false,
        },
        TargetLive::NotRegular { detail } => SkillOutcome {
            name: name.to_string(),
            action: "error".to_string(),
            detail: format!(
                "{} target is not regular ({}); refusing to silently clean manifest",
                target_path.display(),
                detail
            ),
            ok: false,
        },
    }
}

fn conflict_outcome(name: &str, target_path: &Path, target: TargetLive) -> SkillOutcome {
    let detail = match target {
        TargetLive::Present { tree_hash } => format!(
            "{} already exists with different bytes ({}); refusing to overwrite",
            target_path.display(),
            tree_hash
        ),
        TargetLive::NotRegular { detail } => detail,
        TargetLive::Absent => "manifest drift; refresh to re-plan".to_string(),
    };
    SkillOutcome {
        name: name.to_string(),
        action: "skipped".to_string(),
        detail,
        ok: true,
    }
}

// ---- Source revalidation / staging ------------------------------------------
//
// Staging and backup directories are deliberately kept OUTSIDE
// `Paths.skills_dir`. OpenCode's skills scanner walks the entire
// skills tree looking for `SKILL.md` files at any depth, so a
// half-staged tree or a retained backup directory under
// `skills_dir` could be discovered as a misformed or stale skill.
// We place them in a sibling directory under the opencode config
// root (e.g. `<xdg|home>/.config/opencode/.agenthd-staging.<pid>.<n>`)
// and use an `agenthd`-specific hidden prefix so other tools
// sharing the opencode config root can identify and ignore them.

/// Hidden, app-specific prefix for staging/backup directories.
/// Scoped to `agenthd` so a `ls` of the opencode config root
/// makes the source of these temp directories obvious.
const APP_TMP_PREFIX: &str = "agenthd";

/// Derive the directory in which staging/backup scratch
/// directories live. This is the OpenCode config root, a
/// SIBLING of `Paths.skills_dir`:
///
/// ```text
///   <xdg|home>/.config/opencode/skills   -> target's parent
///   <xdg|home>/.config/opencode/         -> scratch root (target.parent().parent())
/// ```
///
/// Placing the scratch root outside the skills dir ensures that
/// a half-staged `.agenthd-staging.<pid>.<n>` tree or a
/// retained `.agenthd-backup.<pid>.<n>` directory cannot be
/// scanned as a skill by OpenCode.
///
/// Fails closed if `target.parent().parent()` cannot be derived
/// — the install refuses to proceed rather than silently
/// spilling the scratch root into an unsafe location.
fn scratch_root(target: &Path) -> std::result::Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("target {} has no parent directory", target.display()))?;
    let grand = parent.parent().ok_or_else(|| {
        format!(
            "target {} parent `{}` has no grandparent directory; refusing to derive a scratch root for staging/backup",
            target.display(),
            parent.display()
        )
    })?;
    Ok(grand.to_path_buf())
}

/// Stage the new tree under a scratch-root temp dir and publish
/// it into `target` via the OS no-replace primitive. The temp
/// dir lives outside `Paths.skills_dir` (see `scratch_root`)
/// so an interrupted stage never exposes a partial SKILL.md
/// tree to the OpenCode skills scanner.
fn stage_and_publish(entries: &[FileEntry], target: &Path) -> std::result::Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("target {} has no parent directory", target.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {}", parent.display(), e))?;
    let scratch = scratch_root(target)?;
    fs::create_dir_all(&scratch)
        .map_err(|e| format!("create scratch root {}: {}", scratch.display(), e))?;
    let staging = unique_staging_dir(&scratch);
    if let Err(e) = write_tree(&staging, entries) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("stage {}: {}", staging.display(), e));
    }
    match rename_no_replace(&staging, target) {
        Ok(()) => Ok(()),
        Err(code) => {
            let _ = fs::remove_dir_all(&staging);
            Err(format!(
                "publish {} -> {} failed with OS error code {}",
                staging.display(),
                target.display(),
                code
            ))
        }
    }
}

/// Outcome of an in-place replace of an owned target tree.
enum ReplaceOutcome {
    /// Publish succeeded. `retained_backup` is `Some(_)` if the
    /// optional post-publish cleanup of the backup directory
    /// failed; the user's previous tree is preserved at that
    /// path so the update is fully reversible even on cleanup
    /// failure.
    Published { retained_backup: Option<PathBuf> },
    /// The target did not exist at publish time (already removed
    /// by another process or drifted between plan and apply).
    TargetMissing,
    /// The publish failed and the target was either never
    /// removed, or was restored from backup. `detail` is the
    /// user-facing failure message; if it includes "retained at"
    /// the backup directory is intentionally preserved because
    /// the restore step also failed.
    Failed { detail: String },
}

/// Replace an owned tree using two OS no-replace renames. Steps:
///
/// 1. Stage the new tree in a scratch-root temp directory
///    OUTSIDE `Paths.skills_dir` (see `scratch_root`).
/// 2. Atomically rename the existing target into a unique
///    scratch-root backup directory. After this step the
///    target path is empty and the previous bytes are
///    isolated.
/// 3. Atomically rename the staged tree into the target path
///    via the OS no-replace primitive.
/// 4. On step 3 failure, atomically rename the backup back to
///    the target via the OS no-replace primitive. The restore
///    leaves the user's installed tree intact.
/// 5. On step 4 failure, leave the backup directory in place
///    and report the retained path so the user can recover
///    manually. The target remains absent — the publish
///    genuinely failed — and we never claim the bytes were
///    restored.
/// 6. On step 3 success, attempt to remove the backup directory.
///    Cleanup failures are reported via `retained_backup` but do
///    not fail the update: the new tree is already in place.
///
/// The two OS renames in steps 2 and 3 are individually atomic
/// (renameat2/RENAME_NOREPLACE on Linux, MoveFileW on Windows);
/// the swap as a whole is NOT atomic, since the brief window
/// between them has the target path absent. We do not claim
/// atomic compare-and-swap semantics — only that each
/// transition is atomic and that the recovery path always
/// either restores the previous tree or reports the retained
/// backup so the user can recover by hand.
fn replace_owned_tree(target: &Path, entries: &[FileEntry]) -> ReplaceOutcome {
    let parent = match target.parent() {
        Some(p) => p,
        None => {
            return ReplaceOutcome::Failed {
                detail: format!("target {} has no parent directory", target.display()),
            }
        }
    };
    if let Err(e) = fs::create_dir_all(parent) {
        return ReplaceOutcome::Failed {
            detail: format!("create {}: {}", parent.display(), e),
        };
    }
    let scratch = match scratch_root(target) {
        Ok(p) => p,
        Err(detail) => return ReplaceOutcome::Failed { detail },
    };
    if let Err(e) = fs::create_dir_all(&scratch) {
        return ReplaceOutcome::Failed {
            detail: format!("create scratch root {}: {}", scratch.display(), e),
        };
    }
    let staging = unique_staging_dir(&scratch);
    if let Err(e) = write_tree(&staging, entries) {
        let _ = fs::remove_dir_all(&staging);
        return ReplaceOutcome::Failed {
            detail: format!("stage {}: {}", staging.display(), e),
        };
    }
    let backup = unique_backup_dir(&scratch);
    // Step 2: atomically move the existing target into the
    // backup directory. If the target is already gone (drift)
    // we report TargetMissing and clean up the staged tree.
    if let Err(code) = rename_no_replace_seamed(target, &backup) {
        let _ = fs::remove_dir_all(&staging);
        if is_not_found_code(code) {
            return ReplaceOutcome::TargetMissing;
        }
        return ReplaceOutcome::Failed {
            detail: format!(
                "rename {} -> {} failed with OS error code {}",
                target.display(),
                backup.display(),
                code
            ),
        };
    }
    // Step 3: atomically publish the staged tree into place.
    if let Err(code) = rename_no_replace_seamed(&staging, target) {
        // Step 4: attempt to restore from backup via the same
        // primitive. If the backup was somehow lost between steps
        // 2 and 3, this will report TargetMissing on restore.
        let restore = rename_no_replace_seamed(&backup, target);
        let _ = fs::remove_dir_all(&staging);
        match restore {
            Ok(()) => ReplaceOutcome::Failed {
                detail: format!(
                    "publish failed with OS error code {}; restored from backup",
                    code
                ),
            },
            Err(restore_code) => {
                if is_not_found_code(restore_code) {
                    return ReplaceOutcome::Failed {
                        detail: format!(
                            "publish failed (OS error code {}); backup at {} vanished during restore — manual recovery required",
                            code, backup.display()
                        ),
                    };
                }
                ReplaceOutcome::Failed {
                    detail: format!(
                        "publish failed (OS error code {}); restore from backup at {} also failed (OS error code {}) — manual recovery required; do not delete {}",
                        code, backup.display(), restore_code, backup.display()
                    ),
                }
            }
        }
    } else {
        // Step 6: best-effort cleanup. Failure does not undo
        // the publish — the new tree is already at the target.
        let retained = match fs::remove_dir_all(&backup) {
            Ok(()) => None,
            Err(_) => Some(backup.clone()),
        };
        ReplaceOutcome::Published {
            retained_backup: retained,
        }
    }
}

/// Remove an owned target tree safely: rename the target into a
/// unique scratch-root backup directory (atomic on the same
/// filesystem), then `remove_dir_all` the backup. The backup
/// lives outside `Paths.skills_dir` (see `scratch_root`) so a
/// retained backup directory cannot be scanned by OpenCode.
///
/// The manifest update in the caller happens after this
/// function returns; if the cleanup fails, the previous bytes
/// survive in the backup at the reported path and the caller
/// is told where to find them so the user can act manually.
fn rename_then_remove(target: &Path) -> std::result::Result<Option<PathBuf>, String> {
    if target.parent().is_none() {
        return Err(format!(
            "target {} has no parent directory",
            target.display()
        ));
    }
    let scratch = scratch_root(target)?;
    fs::create_dir_all(&scratch)
        .map_err(|e| format!("create scratch root {}: {}", scratch.display(), e))?;
    let backup = unique_backup_dir(&scratch);
    match rename_no_replace_seamed(target, &backup) {
        Ok(()) => {}
        Err(code) => {
            if is_not_found_code(code) {
                return Ok(None);
            }
            return Err(format!(
                "rename {} -> {} failed with OS error code {}",
                target.display(),
                backup.display(),
                code
            ));
        }
    }
    match fs::remove_dir_all(&backup) {
        Ok(()) => Ok(None),
        Err(_) => Ok(Some(backup)),
    }
}

/// Best-effort mapping of OS error codes to "not found". Used so
/// `rename_no_replace` failures on a target the previous step
/// already moved (or that vanished between plan and apply) are
/// not misreported as infrastructure failures.
fn is_not_found_code(code: i32) -> bool {
    #[cfg(target_os = "linux")]
    {
        code == libc::ENOENT
    }
    #[cfg(target_os = "windows")]
    {
        code == 2 // ERROR_FILE_NOT_FOUND
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        code == 0
    }
}

/// Write a tree of `entries` rooted at `root`. Per-file atomic write
/// (temp + rename within the target dir) so a crash mid-write cannot
/// leave a half-formed file at the final path. The outer directory
/// rename at the end is the atomicity boundary.
fn write_tree(root: &Path, entries: &[FileEntry]) -> std::result::Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("create_dir_all {}: {}", root.display(), e))?;
    for entry in entries {
        let path = root.join(entry.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("create_dir_all {}: {}", dir.display(), e))?;
        }
        let parent = path.parent().unwrap_or(root);
        let temp = unique_staging_file(parent);
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| format!("create {}: {}", temp.display(), e))?;
            file.write_all(&entry.bytes)
                .map_err(|e| format!("write {}: {}", temp.display(), e))?;
            file.sync_all()
                .map_err(|e| format!("sync {}: {}", temp.display(), e))?;
        }
        fs::rename(&temp, &path)
            .map_err(|e| format!("rename {} -> {}: {}", temp.display(), path.display(), e))?;
    }
    Ok(())
}

fn unique_staging_dir(parent: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let candidate = parent.join(format!(".{APP_TMP_PREFIX}-staging.{}.{:x}", pid, n));
    if !candidate.exists() {
        return candidate;
    }
    parent.join(format!(
        ".{APP_TMP_PREFIX}-staging.{}.{:x}.fallback",
        pid,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    ))
}

fn unique_backup_dir(parent: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let candidate = parent.join(format!(".{APP_TMP_PREFIX}-backup.{}.{:x}", pid, n));
    if !candidate.exists() {
        return candidate;
    }
    parent.join(format!(
        ".{APP_TMP_PREFIX}-backup.{}.{:x}.fallback",
        pid,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    ))
}

fn unique_staging_file(parent: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{APP_TMP_PREFIX}-tmp.{}.{:x}", pid, n))
}

/// Test seam: allows tests to force specific `rename_no_replace`
/// calls inside `replace_owned_tree` and `rename_then_remove` to
/// return a fixed OS error code. Used to exercise the
/// failure-recovery paths without resorting to flaky filesystem
/// races. The seam is a no-op in release builds; the production
/// `rename_no_replace` runs unchanged.
///
/// The seam matches by destination path suffix: the test arms a
/// (suffix, code) pair, and the next call whose `dst` path ends
/// with the armed suffix returns `code` instead of calling the
/// OS primitive. The arm is consumed on match (single-shot).
#[cfg(test)]
mod rename_seam {
    use std::cell::RefCell;
    use std::path::Path;

    thread_local! {
        // (suffix, code). Single-shot: cleared on first match.
        static ARM: RefCell<Option<(String, i32)>> = const { RefCell::new(None) };
        // (suffix, code). Single-shot: cleared on first match.
        // Matches against the rename src path so tests can fail
        // renames whose dst is a uniquely generated backup
        // directory.
        static SRC_ARM: RefCell<Option<(String, i32)>> = const { RefCell::new(None) };
        // (suffix, code). Persistent: matches every call whose
        // dst path ends with the suffix until cleared.
        static PERSISTENT_ARM: RefCell<Option<(String, i32)>> = const { RefCell::new(None) };
    }

    /// Arm a single-shot failure: the next call whose `dst`
    /// path ends with `suffix` returns `code`.
    pub fn arm_next(suffix: &str, code: i32) {
        ARM.with(|c| *c.borrow_mut() = Some((suffix.to_string(), code)));
    }

    /// Arm a single-shot failure: the next call whose `src`
    /// path ends with `suffix` returns `code`. Used by tests
    /// that need to fail a rename whose dst is a uniquely
    /// generated path (e.g. the backup dir in
    /// `rename_then_remove`) — the dst suffix is not
    /// knowable in advance, but the src is the caller-supplied
    /// target.
    #[allow(dead_code)]
    pub fn arm_next_on_src(suffix: &str, code: i32) {
        SRC_ARM.with(|c| *c.borrow_mut() = Some((suffix.to_string(), code)));
    }

    /// Arm a persistent failure: every call whose `dst` path
    /// ends with `suffix` returns `code` until `clear()`.
    #[allow(dead_code)]
    pub fn arm_persistent(suffix: &str, code: i32) {
        PERSISTENT_ARM.with(|c| *c.borrow_mut() = Some((suffix.to_string(), code)));
    }

    /// Clear all arms.
    pub fn clear() {
        ARM.with(|c| *c.borrow_mut() = None);
        SRC_ARM.with(|c| *c.borrow_mut() = None);
        PERSISTENT_ARM.with(|c| *c.borrow_mut() = None);
    }

    fn matches(dst: &Path, suffix: &str) -> bool {
        dst.to_string_lossy().ends_with(suffix)
    }

    /// Run `f` if no seam matches `dst`; otherwise return the
    /// armed error code.
    pub fn checked<F: FnOnce() -> Result<(), i32>>(
        src: &Path,
        dst: &Path,
        f: F,
    ) -> Result<(), i32> {
        let next = ARM.with(|c| c.borrow_mut().take());
        if let Some((suffix, code)) = next {
            if matches(dst, &suffix) {
                return Err(code);
            }
            // Re-arm if it didn't match this dst.
            ARM.with(|c| *c.borrow_mut() = Some((suffix, code)));
        }
        let src_next = SRC_ARM.with(|c| c.borrow_mut().take());
        if let Some((suffix, code)) = src_next {
            if src.to_string_lossy().ends_with(&suffix) {
                return Err(code);
            }
            // Re-arm if it didn't match this src.
            SRC_ARM.with(|c| *c.borrow_mut() = Some((suffix, code)));
        }
        let persistent = PERSISTENT_ARM.with(|c| c.borrow().clone());
        if let Some((suffix, code)) = persistent {
            if matches(dst, &suffix) {
                return Err(code);
            }
        }
        f()
    }
}

#[cfg(test)]
fn rename_no_replace_seamed(src: &Path, dst: &Path) -> Result<(), i32> {
    rename_seam::checked(src, dst, || rename_no_replace(src, dst))
}

#[cfg(not(test))]
fn rename_no_replace_seamed(src: &Path, dst: &Path) -> Result<(), i32> {
    rename_no_replace(src, dst)
}

// ---- Platform-conditional rename_no_replace --------------------------------
//
// The skills installer needs an OS no-replace primitive for two publish
// steps: (1) staging -> destination on `Install` and `Update`, and (2)
// backup -> destination on the `Update` failure-restore path. Linux
// uses `renameat2` with `RENAME_NOREPLACE`; Windows uses raw
// `MoveFileW`; other platforms fail closed. The same primitive the
// Tools installer uses (which has its own copy for cycle isolation)
// so we duplicate the implementation here rather than import from
// `crate::tools` and create a store -> tools inbound dependency.

#[cfg(target_os = "linux")]
fn rename_no_replace(src: &Path, dst: &Path) -> Result<(), i32> {
    use libc::{renameat2, AT_FDCWD, RENAME_NOREPLACE};
    use std::ffi::CString;
    let src_bytes = src.as_os_str().as_encoded_bytes();
    let dst_bytes = dst.as_os_str().as_encoded_bytes();
    let src_c = match CString::new(src_bytes) {
        Ok(s) => s,
        Err(_) => return Err(libc::EINVAL),
    };
    let dst_c = match CString::new(dst_bytes) {
        Ok(s) => s,
        Err(_) => return Err(libc::EINVAL),
    };
    // Safety: renameat2 is a syscall; we pass valid CStrings and AT_FDCWD.
    let rc = unsafe {
        renameat2(
            AT_FDCWD,
            src_c.as_ptr(),
            AT_FDCWD,
            dst_c.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

#[cfg(target_os = "windows")]
fn rename_no_replace(src: &Path, dst: &Path) -> Result<(), i32> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;

    fn to_wide(p: &Path) -> Vec<u16> {
        OsStr::new(p)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let src_w = to_wide(src);
    let dst_w = to_wide(dst);
    // Safety: MoveFileW is documented as taking two null-terminated UTF-16
    // paths. `to_wide` produces them with the explicit trailing null.
    let ok = unsafe { MoveFileW(src_w.as_ptr(), dst_w.as_ptr()) };
    if ok != 0 {
        Ok(())
    } else {
        // Safety: GetLastError is documented as safe to call immediately
        // after a Win32 API that returns FALSE / null.
        Err(unsafe { GetLastError() } as i32)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn rename_no_replace(_src: &Path, _dst: &Path) -> Result<(), i32> {
    // Fail closed: no proven no-replace primitive on this OS.
    Err(0)
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
