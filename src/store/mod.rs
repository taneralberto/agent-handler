//! Path resolution, the ownership manifest, atomic-I/O primitives, the
//! shared SHA-256 helper, and the legacy `agenthd` migration. These are
//! the building blocks the canonical / sync / plugin submodules compose.
//!
//! Public API is exposed via the parent `crate::store` path; consumers
//! continue to write `use crate::store::{Paths, State, ...}` exactly as
//! before. The submodules hold implementation detail only.

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

/// Filename the bundled OpenCode sidebar plugin writes under the global
/// `plugins/` directory. `Paths::resolve` joins it onto the OpenCode
/// root to produce `paths.plugin_file`. Test setup helpers also reach
/// it via the `pub(super)` visibility below.
pub(in crate::store) const PLUGIN_FILENAME: &str = "agenthd-subagents.tsx";

mod canonical;
mod plugin;
mod sync;

#[cfg(test)]
mod tests;

pub use canonical::{
    delete_canonical, load_canonical, rename_canonical, save_canonical, seed_starters,
    update_bundled_prompts, UpdatePromptOutcome,
};
pub use plugin::{install_plugin, plugin_status, uninstall_plugin, PluginStatus};
// `compute_plan` is re-exported even though the production TUI now
// calls `plan_for` directly. The store unit tests still use the
// combined-view wrapper, so keep the symbol available to them.
#[allow(unused_imports)]
pub use sync::{
    apply_safe, compute_plan, force_install, plan_for, ApplyOutcome, SyncItem, SyncStatus,
    SyncTarget,
};

/// Resolved paths used by the binary.
///
/// `agenthd_root` is always `$HOME/.agenthd` regardless of `XDG_CONFIG_HOME`,
/// because agenthd-owned state lives outside the XDG config tree. The OpenCode
/// target directory, however, still honors `XDG_CONFIG_HOME` with the usual
/// `$HOME/.config` fallback because OpenCode itself uses that location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub agenthd_root: PathBuf,
    pub canonical_dir: PathBuf,
    pub state_file: PathBuf,
    pub target_dir: PathBuf,
    pub pi_target_dir: PathBuf,
    pub plugin_file: PathBuf,
    pub plugin_config: PathBuf,
    /// Parent of the OpenCode global skills directory. Sibling of `target_dir`
    /// (`<xdg|home>/.config/opencode/agents` -> `<xdg|home>/.config/opencode/skills`)
    /// so the no-replace publish primitive stays on the same filesystem volume.
    pub skills_dir: PathBuf,
}

impl Paths {
    /// Resolve paths from explicit env values. Tests pass these in directly.
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
            plugin_file: opencode_root.join("plugins").join(PLUGIN_FILENAME),
            plugin_config: opencode_root.join("tui.json"),
            skills_dir: opencode_root.join("skills"),
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

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.canonical_dir)
            .with_context(|| format!("create {}", self.canonical_dir.display()))?;
        fs::create_dir_all(&self.target_dir)
            .with_context(|| format!("create {}", self.target_dir.display()))?;
        fs::create_dir_all(&self.pi_target_dir)
            .with_context(|| format!("create {}", self.pi_target_dir.display()))?;
        fs::create_dir_all(&self.skills_dir)
            .with_context(|| format!("create {}", self.skills_dir.display()))?;
        Ok(())
    }
}

/// One-shot legacy migration: move `$XDG_CONFIG_HOME/agenthd` or
/// `$HOME/.config/agenthd` to `$HOME/.agenthd` when the latter is missing.
///
/// Safe by construction:
/// - Never overwrites or merges into an existing `$HOME/.agenthd`.
/// - If both locations exist, the new `.agenthd` wins and the legacy directory
///   is left untouched so the user can inspect or remove it manually.
/// - Returns `Ok(true)` only when a rename happened.
///
/// The OpenCode target directory is not touched by this helper.
pub fn migrate_legacy_agenthd(home: &Path, xdg_config_home: Option<&Path>) -> Result<bool> {
    let target = home.join(".agenthd");
    if target.exists() {
        return Ok(false);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = xdg_config_home {
        if !xdg.as_os_str().is_empty() {
            candidates.push(xdg.join("agenthd"));
        }
    }
    candidates.push(home.join(".config").join("agenthd"));
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        move_dir(&candidate, &target).with_context(|| {
            format!(
                "migrate legacy `{}` to `{}`",
                candidate.display(),
                target.display()
            )
        })?;
        return Ok(true);
    }
    Ok(false)
}

fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_dir_recursive(src, dst)?;
            fs::remove_dir_all(src).with_context(|| format!("remove {}", src.display()))?;
            Ok(())
        }
        Err(e) => Err(anyhow!(
            "rename `{}` -> `{}`: {}",
            src.display(),
            dst.display(),
            e
        )),
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("read_dir {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            // Skip symlinks: legacy config should not contain them, and copying
            // blindly could escape the destination.
            continue;
        } else if file_type.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copy `{}` -> `{}`", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// On-disk ownership manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub starters_seeded: bool,
    #[serde(default)]
    pub installed: BTreeMap<String, String>,
    #[serde(default)]
    pub pi_installed: BTreeMap<String, String>,
    #[serde(default)]
    pub plugin_hash: Option<String>,
}

impl State {
    /// Per-target owned hash map. Exposed to the `sync` submodule so the
    /// planner can read both `installed` and `pi_installed` uniformly.
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
/// `sync` and `plugin` submodules so the planner, installer, and
/// bundled-prompt refresh all hash identically.
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
