//! Path resolution, the ownership manifest, atomic-I/O primitives,
//! and the shared SHA-256 helper. These are the building blocks the
//! canonical / sync / settings submodules compose.
//!
//! Public API is exposed via the parent `crate::store` path; consumers
//! continue to write `use crate::store::{Paths, State, ...}` exactly
//! as before. The submodules hold implementation detail only.
//!
//! Configuration model: there is exactly one source of truth for the
//! canonical agent directory — a single user-configured checkout
//! whose `agents/` subdirectory is read and written by the rest of
//! the app. The path is persisted at `$HOME/.agenthd/settings.json`
//! and re-validated on every read; a missing or moved checkout is an
//! explicit error, never a silent fallback. The historical
//! local/repo toggle, Compare screen, and bundled starter registry
//! are gone — there is no Local mode and no Compare mode.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

mod canonical;
mod settings;
mod skills;
mod sync;

#[cfg(test)]
mod tests;

pub use canonical::{delete_canonical, load_canonical, rename_canonical, save_canonical};
// `find_checkout_root_from` is re-exported so the Settings screen
// can use it as a first-run hint (without persisting or
// auto-confirming) without needing the helper to live in the
// runtime hot path. The runtime never silently walks ancestors to
// infer a checkout; the helper is used only to suggest a starting
// point in the editor buffer.
pub use settings::{
    canonical_dir_from, find_checkout_root_from, load_settings, save_settings, settings_file_path,
    validate_checkout_path, Settings,
};
#[allow(unused_imports)]
pub use skills::{
    apply as apply_skills, plan as plan_skills, OwnedSkill, SkillAction, SkillOutcome,
    SkillPlanItem,
};
#[allow(unused_imports)]
pub use sync::{
    apply_safe, compute_plan, force_install, plan_for, ApplyOutcome, SyncItem, SyncStatus,
    SyncTarget,
};

/// Validate the configured canonical source before any read, write,
/// or plan operation. The check is shared by every store entry point
/// — `load_canonical`, `save_canonical`, `delete_canonical`,
/// `rename_canonical`, `plan_for`, `apply_safe`, `force_install`,
/// and `compute_plan` — so the runtime fails closed (with an
/// explicit error) when the configured checkout has been removed,
/// moved, or replaced with a symlink, instead of silently returning
/// an empty canonical set, accidentally recreating a missing
/// checkout, or letting a precomputed `Remove` plan delete an
/// installed target that has nothing to do with the current source.
///
/// `validate_checkout_path` already enforces the same constraints
/// (absolute path, real directory, real `agents/` subdirectory, no
/// symlinks) on the checkout root; here we drive it from the
/// `paths.canonical_dir` the rest of the store already holds so
/// every entry point can validate without re-deriving the
/// configured checkout root.
pub(in crate::store) fn require_canonical_source(paths: &Paths) -> Result<()> {
    let canonical = &paths.canonical_dir;
    let parent = canonical.parent().ok_or_else(|| {
        anyhow!(
            "canonical source `{}` has no parent directory",
            canonical.display()
        )
    })?;
    validate_checkout_path(parent)
}

/// Resolved paths used by the binary.
///
/// `agenthd_root` is always `$HOME/.agenthd` regardless of
/// `XDG_CONFIG_HOME`, because agenthd-owned state lives outside the
/// XDG config tree. The OpenCode target directory, however, still
/// honors `XDG_CONFIG_HOME` with the usual `$HOME/.config` fallback
/// because OpenCode itself uses that location.
///
/// `canonical_dir` is the directory the rest of the application
/// treats as the source of truth for agent definitions. It is
/// derived from `Settings::checkout_path` and points at the
/// `<checkout>/agents` directory of the user-configured checkout.
/// `Paths::canonical_dir` is the single source of truth for
/// "where is canonical?" — there is no Local/Repo toggle, no
/// separate compare path, and no `~/.agenthd/agents` shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub agenthd_root: PathBuf,
    pub canonical_dir: PathBuf,
    pub state_file: PathBuf,
    pub target_dir: PathBuf,
    pub pi_target_dir: PathBuf,
    /// Parent of the OpenCode global skills directory. Sibling of
    /// `target_dir`
    /// (`<xdg|home>/.config/opencode/agents` ->
    /// `<xdg|home>/.config/opencode/skills`) so the no-replace
    /// publish primitive stays on the same filesystem volume.
    pub skills_dir: PathBuf,
    /// Per-machine settings (currently: the configured checkout
    /// path). The runtime reads/writes it via the `settings`
    /// submodule; the field is kept on `Paths` so callers do not have
    /// to thread a second `PathBuf` through every signature.
    pub settings_file: PathBuf,
}

impl Paths {
    /// Resolve paths from explicit env values. `canonical_dir` is
    /// left as the local default `<agenthd_root>/agents`; the caller
    /// is expected to call `Paths::with_settings` immediately after
    /// to re-point it at the configured checkout.
    pub fn resolve(xdg_config_home: Option<&str>, home: Option<&str>) -> Result<Self> {
        let home_path = match home {
            Some(h) if !h.is_empty() => PathBuf::from(h),
            _ => bail!("HOME is not set"),
        };
        let agenthd_root = home_path.join(".agenthd");
        let target_root = if let Some(value) = xdg_config_home {
            if !value.is_empty() {
                PathBuf::from(value)
            } else {
                bail!("XDG_CONFIG_HOME must not be empty");
            }
        } else {
            home_path.join(".config")
        };
        let opencode_root = target_root.join("opencode");
        Ok(Self {
            canonical_dir: agenthd_root.join("agents"),
            state_file: agenthd_root.join("state.json"),
            target_dir: opencode_root.join("agents"),
            pi_target_dir: home_path.join(".pi").join("agent").join("agents"),
            skills_dir: opencode_root.join("skills"),
            settings_file: settings_file_path(&agenthd_root),
            agenthd_root,
        })
    }

    /// Resolve paths from the current process environment.
    pub fn from_env() -> Result<Self> {
        Self::resolve(
            env::var("XDG_CONFIG_HOME").ok().as_deref(),
            env::var("HOME").ok().as_deref(),
        )
    }

    /// Re-point `canonical_dir` at the configured checkout's
    /// `agents/` directory. The default `canonical_dir` already
    /// points at `<agenthd_root>/agents`; the caller is expected to
    /// replace it with the configured checkout before the first
    /// canonical read. Validation runs here (rather than at write
    /// time) so a corrupt settings file is caught at startup; the
    /// "missing checkout fail closed" contract surfaces the error
    /// rather than papering over it with an empty canonical set.
    pub fn with_settings(mut self, settings: &Settings) -> Result<Self> {
        self.canonical_dir = canonical_dir_from(&self.agenthd_root, settings)?;
        Ok(self)
    }

    /// Create the agenthd root (settings + state parents) and the
    /// output target trees so the binary can write to them on the
    /// first run. The canonical source directory is intentionally
    /// **not** created here: until the user configures a checkout
    /// there is no local canonical, and creating one would silently
    /// turn the historical `<agenthd_root>/agents` default into a
    /// real source. A configured checkout's `agents/` directory
    /// must already exist (see `validate_checkout_path`); the
    /// runtime never has to make it.
    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.agenthd_root)
            .with_context(|| format!("create {}", self.agenthd_root.display()))?;
        fs::create_dir_all(&self.target_dir)
            .with_context(|| format!("create {}", self.target_dir.display()))?;
        fs::create_dir_all(&self.pi_target_dir)
            .with_context(|| format!("create {}", self.pi_target_dir.display()))?;
        fs::create_dir_all(&self.skills_dir)
            .with_context(|| format!("create {}", self.skills_dir.display()))?;
        Ok(())
    }
}

/// On-disk ownership manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub installed: BTreeMap<String, String>,
    #[serde(default)]
    pub pi_installed: BTreeMap<String, String>,
    /// Per-skill ownership for the configured-checkout `skills/`
    /// directory into `Paths.skills_dir`. Keyed by skill directory
    /// name (== `SKILL.md` `name:` value by construction). The
    /// recorded `tree_hash` is the deterministic whole-tree SHA-256
    /// the installer used as the source bytes; the `skill_name` is
    /// the verified identity from `SKILL.md` so identity drift is
    /// detectable.
    ///
    /// `#[serde(default)]` keeps the per-machine state compatible
    /// with `state.json` files written before this field existed:
    /// older manifests load as if the field were empty, and the next
    /// save rebuilds the JSON without losing existing entries.
    #[serde(default)]
    pub installed_skills: BTreeMap<String, skills::OwnedSkill>,
}

impl State {
    /// Per-target owned hash map. Exposed to the `sync` submodule so
    /// the planner can read both `installed` and `pi_installed`
    /// uniformly.
    pub(in crate::store) fn installed(&self, target: SyncTarget) -> &BTreeMap<String, String> {
        match target {
            SyncTarget::OpenCode => &self.installed,
            SyncTarget::Pi => &self.pi_installed,
        }
    }

    /// Mutable counterpart of `installed`; exposed to `sync` only.
    pub(in crate::store) fn installed_mut(
        &mut self,
        target: SyncTarget,
    ) -> &mut BTreeMap<String, String> {
        match target {
            SyncTarget::OpenCode => &mut self.installed,
            SyncTarget::Pi => &mut self.pi_installed,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes).map_err(|e| {
                    anyhow!(
                        "{} is malformed; remove it to recover (collisions will be treated as unowned): {}",
                        path.display(),
                        e
                    )
                })?;
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(anyhow!("read {}: {}", path.display(), e)),
        }
    }
}

/// Compute the SHA-256 of a file as lowercase hex. `None` if the file is
/// missing.
pub fn hash_file(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("hash {}: {}", path.display(), e)),
    }
}

/// Lowercase-hex SHA-256 of an in-memory byte slice. Shared with the
/// `sync` submodule so the planner and installer hash identically.
pub(in crate::store) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Atomically write `bytes` to `target` via a sibling temporary file + rename.
pub fn write_target(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("target {} has no parent directory", target.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temp = unique_temp(parent);
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .with_context(|| format!("create temp {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temp.display()))?;
    }
    if let Err(e) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(anyhow!(
            "rename {} -> {}: {}",
            temp.display(),
            target.display(),
            e
        ));
    }
    Ok(())
}

fn unique_temp(parent: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = process::id();
    for _ in 0..1000 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".tmp.{}.{:x}.{:x}", pid, n, rand_suffix()));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(".tmp.{}.{:x}.fallback", pid, rand_suffix()))
}

fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    hasher.finish()
}

pub(in crate::store) fn write_state(path: &Path, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    write_target(path, &bytes)
}
