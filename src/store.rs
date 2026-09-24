use crate::agent::{canonical_path, starter_agent, Agent, STARTERS};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

const PLUGIN_FILENAME: &str = "agenthd-subagents.tsx";
const PLUGIN_SPEC: &str = "./plugins/agenthd-subagents.tsx";
const PLUGIN_SOURCE: &str = include_str!("../assets/agenthd-subagents.tsx");

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
    fn installed(&self, target: SyncTarget) -> &BTreeMap<String, String> {
        match target {
            SyncTarget::OpenCode => &self.installed,
            SyncTarget::Pi => &self.pi_installed,
        }
    }

    fn installed_mut(&mut self, target: SyncTarget) -> &mut BTreeMap<String, String> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Compute the SHA-256 of a file as lowercase hex. `None` if the file is
/// missing.
pub fn hash_file(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("hash {}: {}", path.display(), e)),
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

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    NotInstalled,
    NotEnabled,
    UpToDate,
    UpdateAvailable,
    Conflict,
}

impl PluginStatus {
    pub fn label(self) -> &'static str {
        match self {
            PluginStatus::NotInstalled => "not installed",
            PluginStatus::NotEnabled => "installed, not enabled",
            PluginStatus::UpToDate => "up to date",
            PluginStatus::UpdateAvailable => "update available",
            PluginStatus::Conflict => "conflict",
        }
    }
}

fn plugin_enabled(path: &Path) -> Result<bool> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(anyhow!("read {}: {}", path.display(), e)),
    };
    let config: serde_json::Value = serde_json::from_str(&contents).with_context(|| {
        format!(
            "parse {}; agenthd only updates JSON config without comments",
            path.display()
        )
    })?;
    Ok(config
        .get("plugin")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|plugin| plugin.as_str() == Some(PLUGIN_SPEC))
        }))
}

fn set_plugin_enabled(path: &Path, enabled: bool) -> Result<()> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_string(),
        Err(e) => return Err(anyhow!("read {}: {}", path.display(), e)),
    };
    let mut config: serde_json::Value = serde_json::from_str(&contents).with_context(|| {
        format!(
            "parse {}; agenthd only updates JSON config without comments",
            path.display()
        )
    })?;
    let object = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("{} must contain a JSON object", path.display()))?;
    let plugins = object
        .entry("plugin")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow!("{}.plugin must be an array", path.display()))?;
    let present = plugins
        .iter()
        .any(|plugin| plugin.as_str() == Some(PLUGIN_SPEC));
    if enabled && !present {
        plugins.push(serde_json::Value::String(PLUGIN_SPEC.to_string()));
    } else if !enabled && present {
        plugins.retain(|plugin| plugin.as_str() != Some(PLUGIN_SPEC));
        if plugins.is_empty() {
            object.remove("plugin");
        }
    } else {
        return Ok(());
    }
    write_target(path, &serde_json::to_vec_pretty(&config)?)
}

pub fn plugin_status(paths: &Paths, state: &State) -> Result<PluginStatus> {
    let source_hash = sha256_hex(PLUGIN_SOURCE.as_bytes());
    let target_hash = match fs::symlink_metadata(&paths.plugin_file) {
        Ok(meta) if meta.file_type().is_file() => hash_file(&paths.plugin_file)?,
        Ok(_) => bail!(
            "plugin {} is not a regular file",
            paths.plugin_file.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(anyhow!("stat {}: {}", paths.plugin_file.display(), e)),
    };
    match target_hash {
        None => Ok(PluginStatus::NotInstalled),
        Some(hash) if hash == source_hash && plugin_enabled(&paths.plugin_config)? => {
            Ok(PluginStatus::UpToDate)
        }
        Some(hash) if hash == source_hash => Ok(PluginStatus::NotEnabled),
        Some(hash) if state.plugin_hash.as_deref() == Some(hash.as_str()) => {
            Ok(PluginStatus::UpdateAvailable)
        }
        Some(_) => Ok(PluginStatus::Conflict),
    }
}

pub fn install_plugin(paths: &Paths, mut state: State) -> Result<(State, PluginStatus)> {
    let status = plugin_status(paths, &state)?;
    if status == PluginStatus::Conflict {
        bail!(
            "plugin {} is unowned or changed externally; resolve it manually",
            paths.plugin_file.display()
        );
    }
    let source_hash = sha256_hex(PLUGIN_SOURCE.as_bytes());
    if !matches!(status, PluginStatus::UpToDate | PluginStatus::NotEnabled) {
        write_target(&paths.plugin_file, PLUGIN_SOURCE.as_bytes())?;
    }
    set_plugin_enabled(&paths.plugin_config, true)?;
    if state.plugin_hash.as_deref() != Some(source_hash.as_str()) {
        state.plugin_hash = Some(source_hash);
        write_state(&paths.state_file, &state)?;
    }
    Ok((state, status))
}

pub fn uninstall_plugin(paths: &Paths, mut state: State) -> Result<State> {
    let target_hash = match fs::symlink_metadata(&paths.plugin_file) {
        Ok(meta) if meta.file_type().is_file() => hash_file(&paths.plugin_file)?,
        Ok(_) => bail!(
            "plugin {} is not a regular file",
            paths.plugin_file.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(anyhow!("stat {}: {}", paths.plugin_file.display(), e)),
    };
    let Some(target_hash) = target_hash else {
        set_plugin_enabled(&paths.plugin_config, false)?;
        if state.plugin_hash.take().is_some() {
            write_state(&paths.state_file, &state)?;
        }
        return Ok(state);
    };
    if state.plugin_hash.as_deref() != Some(target_hash.as_str()) {
        bail!(
            "plugin {} is unowned or changed externally; refusing to remove it",
            paths.plugin_file.display()
        );
    }
    set_plugin_enabled(&paths.plugin_config, false)?;
    fs::remove_file(&paths.plugin_file)
        .with_context(|| format!("remove {}", paths.plugin_file.display()))?;
    state.plugin_hash = None;
    write_state(&paths.state_file, &state)?;
    Ok(state)
}

/// Hash either the on-disk source or its parsed/rendered canonical form.
#[allow(dead_code)]
fn source_hash(agent: &Agent) -> String {
    sha256_hex(agent.render().as_bytes())
}

/// Compute the sync plan.
pub fn compute_plan(paths: &Paths, state: &State) -> Result<Vec<SyncItem>> {
    // Planning is read-only: refreshing the Install/Update screen must not
    // create directories. Mutations create their own parent directories.
    let canonical_names = read_md_filenames(&paths.canonical_dir)?;
    let mut items = Vec::new();
    for target in [SyncTarget::OpenCode, SyncTarget::Pi] {
        let target_dir = target.dir(paths);
        let target_names = read_md_filenames(target_dir)?;
        let mut all_names: BTreeSet<String> = BTreeSet::new();
        all_names.extend(canonical_names.iter().cloned());
        all_names.extend(target_names.iter().cloned());
        all_names.extend(state.installed(target).keys().cloned());

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
pub fn apply_safe(
    paths: &Paths,
    mut state: State,
    items: Vec<SyncItem>,
) -> Result<(State, Vec<ApplyOutcome>)> {
    let original_state = state.clone();
    let mut outcomes = Vec::new();
    for item in items {
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
    for target in [SyncTarget::OpenCode, SyncTarget::Pi] {
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

fn write_state(path: &Path, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    write_target(path, &bytes)
}

/// Lint helper used by tests: collect a summary of parsed agents without
/// bailing on bad files.
#[cfg(test)]
#[allow(dead_code)]
pub fn lint(paths: &Paths) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(e) = load_canonical(paths) {
        errors.push(e.to_string());
    }
    errors
}

/// Load all canonical agents. Errors include the file path.
pub fn load_canonical(paths: &Paths) -> Result<BTreeMap<String, (Agent, PathBuf)>> {
    let mut agents = BTreeMap::new();
    if !paths.canonical_dir.exists() {
        return Ok(agents);
    }
    let mut errors: Vec<String> = Vec::new();
    for entry in fs::read_dir(&paths.canonical_dir)
        .with_context(|| format!("read_dir {}", paths.canonical_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "md")
            .unwrap_or(false)
        {
            continue;
        }
        let meta = entry.file_type()?;
        if !meta.is_file() {
            errors.push(format!("{}: not a regular file", path.display()));
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        match Agent::read(&path) {
            Ok(agent) => {
                agents.insert(stem, (agent, path));
            }
            Err(e) => errors.push(format!("{}: {}", path.display(), e)),
        }
    }
    if !errors.is_empty() {
        bail!(
            "canonical directory has invalid agents:\n{}",
            errors.join("\n")
        );
    }
    Ok(agents)
}

/// Seed starter agents only when missing. Existing canonical files are never
/// overwritten — the user's edits win on every subsequent run. Returns
/// whether any canonical file was written.
pub fn seed_starters(paths: &Paths, mut state: State) -> Result<(bool, State)> {
    paths.ensure_dirs()?;
    let original_state = state.clone();
    let mut wrote_anything = false;
    for starter in STARTERS {
        let path = canonical_path(&paths.canonical_dir, starter.name)?;
        let new_bytes = starter_agent(starter).render().into_bytes();
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.file_type().is_file() {
                    bail!(
                        "{} exists and is not a regular file; refusing to seed",
                        path.display()
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                write_target(&path, &new_bytes)?;
                wrote_anything = true;
            }
            Err(e) => return Err(anyhow!("stat {}: {}", path.display(), e)),
        }
    }
    state.starters_seeded = true;
    if state != original_state {
        write_state(&paths.state_file, &state)?;
    }
    Ok((wrote_anything, state))
}

/// Save an agent's current canonical bytes.
///
/// `prior_hash` is the SHA-256 of the canonical file observed by the caller
/// (e.g. when the editor was opened). Passing `None` for a name that already
/// exists on disk is a collision and is rejected; passing `Some(h)` whose value
/// does not match the current on-disk hash is a stale-write race and is also
/// rejected. The caller is expected to reload from disk before retrying.
pub fn save_canonical(paths: &Paths, agent: &Agent, prior_hash: Option<&str>) -> Result<()> {
    agent.validate()?;
    let path = canonical_path(&paths.canonical_dir, &agent.name)?;
    let current_hash = hash_file(&path)?;
    match (&current_hash, prior_hash) {
        (Some(_), None) => bail!(
            "refusing to overwrite existing `{}`; pick a new name or open it from the agents list first",
            path.display()
        ),
        (Some(actual), Some(expected)) if actual != expected => bail!(
            "`{}` changed on disk since this edit started; reload to pick up the latest version",
            path.display()
        ),
        _ => {}
    }
    write_target(&path, agent.render().as_bytes())
}

/// Rename an agent in canonical storage.
pub fn rename_canonical(paths: &Paths, old_name: &str, new_name: &str) -> Result<()> {
    Agent::validate_name(new_name)?;
    let old_path = canonical_path(&paths.canonical_dir, old_name)?;
    let new_path = canonical_path(&paths.canonical_dir, new_name)?;
    if new_path.exists() {
        bail!("cannot rename to `{}`: file already exists", new_name);
    }
    let bytes = fs::read(&old_path).with_context(|| format!("read {}", old_path.display()))?;
    write_target(&new_path, &bytes)?;
    if let Err(e) = fs::remove_file(&old_path) {
        let _ = fs::remove_file(&new_path);
        return Err(anyhow!(
            "rename failed: wrote {} but could not remove {}: {}",
            new_path.display(),
            old_path.display(),
            e
        ));
    }
    Ok(())
}

/// One row in the `update_bundled_prompts` report.
///
/// The UI summarizes these into a single status-bar message; the per-file
/// detail is exposed for tests and future per-row views, even though the
/// current summary path doesn't include it inline.
#[derive(Debug, Clone)]
pub struct UpdatePromptOutcome {
    pub name: String,
    pub action: String,
    #[allow(dead_code)]
    pub detail: String,
    pub ok: bool,
}

/// Refresh the prompt body of every bundled canonical agent that already
/// exists on disk. Each starter's prompt is sourced from `STARTERS`; the
/// canonical file's existing description, mode, model, and permissions are
/// preserved (they are read from disk and re-written alongside the new
/// prompt body).
///
/// Semantics:
/// - Only the eight bundled names in `STARTERS` are touched. Any
///   other file in the canonical directory — user-created agents or
///   anything else — is left byte-for-byte unchanged.
/// - Missing canonical files are reported as `"skipped"` rather than
///   created. Startup seeding owns creation; this command only refreshes
///   prompts that the user has accepted.
/// - Each save uses the existing `save_canonical` path, so the agent is
///   re-validated and the file is written atomically via a sibling temp +
///   rename. An I/O failure on a later file does not roll back an
///   already-written earlier file — partial progress is preserved.
/// - Returns one `UpdatePromptOutcome` per starter so the UI can report
///   successes and failures distinctly.
pub fn update_bundled_prompts(paths: &Paths) -> Result<Vec<UpdatePromptOutcome>> {
    paths.ensure_dirs()?;
    let mut outcomes = Vec::new();
    for starter in STARTERS {
        let path = paths.canonical_dir.join(format!("{}.md", starter.name));
        let prior_hash = match hash_file(&path) {
            Ok(Some(h)) => Some(h),
            Ok(None) => {
                outcomes.push(UpdatePromptOutcome {
                    name: starter.name.to_string(),
                    action: "skipped".to_string(),
                    detail: "file does not exist; startup seeding owns creation".to_string(),
                    ok: true,
                });
                continue;
            }
            Err(e) => {
                outcomes.push(UpdatePromptOutcome {
                    name: starter.name.to_string(),
                    action: "error".to_string(),
                    detail: format!("hash {}: {}", path.display(), e),
                    ok: false,
                });
                continue;
            }
        };
        let agent = match Agent::read(&path) {
            Ok(a) => a,
            Err(e) => {
                outcomes.push(UpdatePromptOutcome {
                    name: starter.name.to_string(),
                    action: "error".to_string(),
                    detail: format!("read {}: {}", path.display(), e),
                    ok: false,
                });
                continue;
            }
        };
        // Skip the write when the prompt is already current. A single user
        // confirmation should not produce eight no-op writes that still bump
        // mtime on disk.
        if agent.prompt == starter.prompt {
            outcomes.push(UpdatePromptOutcome {
                name: starter.name.to_string(),
                action: "kept".to_string(),
                detail: "prompt already current".to_string(),
                ok: true,
            });
            continue;
        }
        let mut updated = agent;
        updated.prompt = starter.prompt.to_string();
        match save_canonical(paths, &updated, prior_hash.as_deref()) {
            Ok(()) => {
                outcomes.push(UpdatePromptOutcome {
                    name: starter.name.to_string(),
                    action: "updated".to_string(),
                    detail: "prompt body refreshed".to_string(),
                    ok: true,
                });
            }
            Err(e) => {
                outcomes.push(UpdatePromptOutcome {
                    name: starter.name.to_string(),
                    action: "error".to_string(),
                    detail: format!("save {}: {}", path.display(), e),
                    ok: false,
                });
            }
        }
    }
    Ok(outcomes)
}

/// Delete a canonical agent file.
pub fn delete_canonical(paths: &Paths, name: &str) -> Result<()> {
    Agent::validate_name(name)?;
    let path = canonical_path(&paths.canonical_dir, name)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("delete {}: {}", path.display(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Mode, PermissionAction};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn setup_paths(dir: &TempDir) -> Paths {
        let paths = Paths {
            agenthd_root: dir.path().join(".agenthd"),
            canonical_dir: dir.path().join(".agenthd").join("agents"),
            state_file: dir.path().join(".agenthd").join("state.json"),
            target_dir: dir.path().join(".config").join("opencode").join("agents"),
            pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
            plugin_file: dir
                .path()
                .join(".config")
                .join("opencode")
                .join("plugins")
                .join(PLUGIN_FILENAME),
            plugin_config: dir.path().join(".config").join("opencode").join("tui.json"),
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
        };
        paths.ensure_dirs().unwrap();
        paths
    }

    fn read_target(paths: &Paths, name: &str) -> Option<String> {
        let path = paths.target_dir.join(format!("{}.md", name));
        fs::read_to_string(&path).ok()
    }

    fn read_pi_target(paths: &Paths, name: &str) -> Option<String> {
        let path = paths.pi_target_dir.join(format!("{}.md", name));
        fs::read_to_string(&path).ok()
    }

    #[test]
    fn paths_resolve_agenthd_root_follows_home_only() {
        let p = Paths::resolve(Some("/tmp/abc"), Some("/tmp/home")).unwrap();
        assert_eq!(p.agenthd_root, PathBuf::from("/tmp/home/.agenthd"));
        assert_eq!(p.canonical_dir, PathBuf::from("/tmp/home/.agenthd/agents"));
        assert_eq!(p.state_file, PathBuf::from("/tmp/home/.agenthd/state.json"));
        assert_eq!(p.target_dir, PathBuf::from("/tmp/abc/opencode/agents"));
        assert_eq!(p.pi_target_dir, PathBuf::from("/tmp/home/.pi/agent/agents"));
    }

    #[test]
    fn paths_resolve_target_falls_back_to_home_config() {
        let p = Paths::resolve(None, Some("/tmp/home")).unwrap();
        assert_eq!(p.agenthd_root, PathBuf::from("/tmp/home/.agenthd"));
        assert_eq!(
            p.target_dir,
            PathBuf::from("/tmp/home/.config/opencode/agents")
        );
        assert_eq!(p.pi_target_dir, PathBuf::from("/tmp/home/.pi/agent/agents"));
        assert_eq!(
            p.skills_dir,
            PathBuf::from("/tmp/home/.config/opencode/skills")
        );
    }

    #[test]
    fn paths_resolve_does_not_pull_agenthd_under_xdg() {
        let p = Paths::resolve(Some("/xdg/override"), Some("/home/me")).unwrap();
        assert_eq!(p.agenthd_root, PathBuf::from("/home/me/.agenthd"));
        assert!(p.canonical_dir.starts_with("/home/me/.agenthd"));
        assert!(!p.canonical_dir.starts_with("/xdg/override"));
        assert_eq!(p.target_dir, PathBuf::from("/xdg/override/opencode/agents"));
        assert_eq!(p.pi_target_dir, PathBuf::from("/home/me/.pi/agent/agents"));
        assert_eq!(p.skills_dir, PathBuf::from("/xdg/override/opencode/skills"));
    }

    #[test]
    fn paths_reject_empty_xdg() {
        assert!(Paths::resolve(Some(""), Some("/tmp")).is_err());
    }

    #[test]
    fn paths_reject_missing_home() {
        // HOME is required for the agenthd root, even if XDG is set.
        assert!(Paths::resolve(Some("/xdg"), None).is_err());
        assert!(Paths::resolve(None, None).is_err());
    }

    #[test]
    fn paths_reject_empty_home() {
        assert!(Paths::resolve(Some("/xdg"), Some("")).is_err());
        assert!(Paths::resolve(None, Some("")).is_err());
    }

    #[test]
    fn migrate_legacy_agenthd_moves_xdg_layout_to_home() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let xdg = home.join("xdg");
        fs::create_dir_all(xdg.join("agenthd").join("agents")).unwrap();
        let legacy_state = xdg.join("agenthd").join("state.json");
        let legacy_marker = xdg.join("agenthd").join("agents").join("custom.md");
        fs::write(&legacy_state, b"{\"installed\":{}}").unwrap();
        fs::write(&legacy_marker, b"hello").unwrap();

        let migrated = migrate_legacy_agenthd(home, Some(xdg.as_path())).unwrap();
        assert!(migrated, "expected a migration to occur");
        assert!(home.join(".agenthd").is_dir());
        assert!(home.join(".agenthd/agents/custom.md").is_file());
        assert_eq!(
            fs::read_to_string(home.join(".agenthd/state.json")).unwrap(),
            "{\"installed\":{}}"
        );
        assert!(!xdg.join("agenthd").exists());

        // Idempotent: a second call is a no-op.
        let again = migrate_legacy_agenthd(home, Some(xdg.as_path())).unwrap();
        assert!(!again);
    }

    #[test]
    fn migrate_legacy_agenthd_moves_home_config_layout_to_home() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".config/agenthd/agents")).unwrap();
        fs::write(home.join(".config/agenthd/state.json"), b"{}").unwrap();
        fs::write(home.join(".config/agenthd/agents/extra.md"), b"x").unwrap();

        let migrated = migrate_legacy_agenthd(home, None).unwrap();
        assert!(migrated);
        assert!(home.join(".agenthd").is_dir());
        assert!(home.join(".agenthd/agents/extra.md").is_file());
        assert!(!home.join(".config/agenthd").exists());
    }

    #[test]
    fn migrate_legacy_agenthd_noop_when_nothing_legacy() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let migrated = migrate_legacy_agenthd(home, None).unwrap();
        assert!(!migrated);
        assert!(!home.join(".agenthd").exists());
    }

    #[test]
    fn migrate_legacy_agenthd_preserves_existing_home_layout() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".agenthd/agents")).unwrap();
        let current = home.join(".agenthd/agents/keep.md");
        fs::write(&current, b"current").unwrap();

        // Also create a legacy dir that should NOT be touched.
        fs::create_dir_all(home.join(".config/agenthd/agents")).unwrap();
        let legacy = home.join(".config/agenthd/agents/legacy.md");
        fs::write(&legacy, b"legacy").unwrap();

        let migrated = migrate_legacy_agenthd(home, None).unwrap();
        assert!(!migrated, "must not overwrite existing .agenthd");
        assert_eq!(fs::read_to_string(&current).unwrap(), "current");
        assert_eq!(fs::read_to_string(&legacy).unwrap(), "legacy");
        assert!(home.join(".config/agenthd").is_dir());
    }

    #[test]
    fn migrate_legacy_agenthd_prefers_xdg_legacy_over_home_config() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let xdg = home.join("xdg");
        fs::create_dir_all(xdg.join("agenthd/agents")).unwrap();
        fs::write(xdg.join("agenthd/agents/from_xdg.md"), b"xdg").unwrap();
        fs::create_dir_all(home.join(".config/agenthd/agents")).unwrap();
        fs::write(home.join(".config/agenthd/agents/from_home.md"), b"home").unwrap();

        let migrated = migrate_legacy_agenthd(home, Some(xdg.as_path())).unwrap();
        assert!(migrated);
        // The XDG candidate was moved.
        assert!(home.join(".agenthd/agents/from_xdg.md").is_file());
        assert!(!home.join(".agenthd/agents/from_home.md").exists());
        // The HOME/.config/agenthd candidate is left alone (priority is XDG).
        assert!(home.join(".config/agenthd/agents/from_home.md").is_file());
    }

    #[test]
    fn migrate_legacy_agenthd_ignores_legacy_files_that_are_not_dirs() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        // A stray file at the legacy path is not a directory and must be skipped.
        fs::create_dir_all(home.join(".config")).unwrap();
        fs::write(home.join(".config/agenthd"), b"stray").unwrap();
        let migrated = migrate_legacy_agenthd(home, None).unwrap();
        assert!(!migrated);
        assert!(home.join(".config/agenthd").is_file());
        assert!(!home.join(".agenthd").exists());
    }

    #[test]
    fn compute_plan_is_read_only() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().to_str().unwrap();
        let paths = Paths::resolve(None, Some(home)).unwrap();

        assert!(compute_plan(&paths, &State::default()).unwrap().is_empty());
        assert!(!paths.canonical_dir.exists());
        assert!(!paths.target_dir.exists());
    }

    #[test]
    fn plugin_install_and_uninstall_are_owned_and_safe() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();

        assert_eq!(
            plugin_status(&paths, &state).unwrap(),
            PluginStatus::NotInstalled
        );
        let (state, prior) = install_plugin(&paths, state).unwrap();
        assert_eq!(prior, PluginStatus::NotInstalled);
        assert_eq!(
            plugin_status(&paths, &state).unwrap(),
            PluginStatus::UpToDate
        );
        assert_eq!(
            fs::read_to_string(&paths.plugin_file).unwrap(),
            PLUGIN_SOURCE
        );
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&paths.plugin_config).unwrap()).unwrap();
        assert_eq!(config["plugin"][0], PLUGIN_SPEC);

        let state = uninstall_plugin(&paths, state).unwrap();
        assert!(!paths.plugin_file.exists());
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&paths.plugin_config).unwrap()).unwrap();
        assert!(config.get("plugin").is_none());
        assert!(state.plugin_hash.is_none());
    }

    #[test]
    fn seed_starters_is_idempotent_and_preserves_existing() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (wrote, _) = seed_starters(&paths, State::default()).unwrap();
        assert!(wrote);

        let scout = paths.canonical_dir.join("scout.md");
        let bytes = fs::read(&scout).unwrap();
        // Pre-existing user file: rename to a sentinel and re-seed.
        fs::write(&scout, b"# existing user file\n").unwrap();
        let (_wrote2, state) =
            seed_starters(&paths, State::load(&paths.state_file).unwrap()).unwrap();
        assert_eq!(fs::read(&scout).unwrap(), b"# existing user file\n");
        assert!(state.starters_seeded);

        // Restore from earlier bytes so we keep that file valid; we already overwrote.
        fs::write(&scout, &bytes).unwrap();
        // Re-seed; should still preserve.
        let (_wrote3, _) = seed_starters(&paths, State::load(&paths.state_file).unwrap()).unwrap();
        assert!(!fs::read(&scout).unwrap().is_empty());
    }

    #[test]
    fn seed_starters_adds_new_roles_without_overwriting_user_files() {
        // Simulates an old canonical directory from before delegate, oracle,
        // orchestrator, planner, and researcher existed: only scout, reviewer,
        // and worker are present with user-modified sentinel bytes. Seeding
        // must add the five new starters and preserve existing bytes.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);

        let sentinels = [
            ("scout", b"# user-edited scout\nkeep me\n".to_vec()),
            ("reviewer", b"# user-edited reviewer\nkeep me\n".to_vec()),
            ("worker", b"# user-edited worker\nkeep me\n".to_vec()),
        ];
        for (name, bytes) in &sentinels {
            fs::write(paths.canonical_dir.join(format!("{}.md", name)), bytes).unwrap();
        }

        let (wrote, state) = seed_starters(&paths, State::default()).unwrap();
        assert!(wrote, "expected the five new starters to be written");
        assert!(state.starters_seeded);

        // Existing user-edited files are not overwritten.
        for (name, expected) in &sentinels {
            let actual = fs::read(paths.canonical_dir.join(format!("{}.md", name))).unwrap();
            assert_eq!(
                &actual, expected,
                "user-edited `{}` was modified by seed_starters",
                name
            );
        }

        // The five new starters are seeded with parseable canonical content.
        for name in [
            "delegate",
            "oracle",
            "orchestrator",
            "planner",
            "researcher",
        ] {
            let path = paths.canonical_dir.join(format!("{}.md", name));
            assert!(path.exists(), "missing seeded file `{}`", name);
            let agent = Agent::read(&path).unwrap_or_else(|e| {
                panic!("seeded `{}` does not parse as a valid agent: {}", name, e)
            });
            assert_eq!(
                agent.mode,
                if name == "orchestrator" {
                    Mode::primary
                } else {
                    Mode::subagent
                }
            );
            assert!(agent.model.is_none());
            assert!(!agent.description.trim().is_empty());
            assert!(!agent.prompt.trim().is_empty());
        }

        // A second invocation touches nothing: nothing was written.
        let (wrote2, _) = seed_starters(&paths, State::load(&paths.state_file).unwrap()).unwrap();
        assert!(!wrote2);
        for (name, expected) in &sentinels {
            let actual = fs::read(paths.canonical_dir.join(format!("{}.md", name))).unwrap();
            assert_eq!(&actual, expected);
        }
    }

    #[test]
    fn seed_starters_does_not_overwrite_user_planner() {
        // A user who hand-wrote a `planner.md` (or who is migrating from a
        // Pi-format planner file) must not have their bytes clobbered by a
        // normal startup seed. startup seeding only writes missing files.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);

        let planner_body =
            b"---\ndescription: custom planner\nmode: subagent\n---\nuser-defined body\n";
        fs::write(paths.canonical_dir.join("planner.md"), planner_body).unwrap();

        let (wrote, _) = seed_starters(&paths, State::default()).unwrap();
        // The other seven starters were missing, so they were written. The
        // user's planner.md must remain untouched regardless.
        assert!(wrote, "the seven missing starters should be seeded");
        assert_eq!(
            fs::read(paths.canonical_dir.join("planner.md")).unwrap(),
            planner_body,
            "user planner.md must survive startup seeding byte-for-byte"
        );
        // And the seeded starter bytes parse cleanly so future refresh
        // rounds can act on them.
        for name in [
            "delegate",
            "oracle",
            "orchestrator",
            "researcher",
            "reviewer",
            "scout",
            "worker",
        ] {
            Agent::read(&paths.canonical_dir.join(format!("{}.md", name)))
                .unwrap_or_else(|e| panic!("seeded `{}` invalid: {}", name, e));
        }
    }

    #[test]
    fn apply_safe_fresh_install() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let state = State::default();
        let (_, state) = seed_starters(&paths, state).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let statuses: Vec<&str> = plan.iter().map(|i| i.status.label()).collect();
        assert!(statuses.iter().all(|s| *s == "not installed"));
        let (state, outcomes) = apply_safe(&paths, state, plan).unwrap();
        assert!(outcomes.iter().all(|o| o.ok));
        let after = compute_plan(&paths, &state).unwrap();
        assert!(after
            .iter()
            .all(|i| matches!(i.status, SyncStatus::UpToDate)));
        for starter in STARTERS {
            assert!(read_target(&paths, starter.name).is_some());
        }
    }

    #[test]
    fn pi_sync_renders_pi_subagents() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        assert!(plan
            .iter()
            .filter(|item| item.target == SyncTarget::Pi)
            .all(|item| matches!(item.status, SyncStatus::NotInstalled)));

        let (state, _) = apply_safe(&paths, state, plan).unwrap();
        let scout = read_pi_target(&paths, "scout").unwrap();
        assert!(scout.contains("name: scout"));
        assert!(scout.contains("tools: read, grep, find, ls, contact_supervisor, bash"));
        assert!(!scout.contains("mode:"));
        assert_eq!(state.pi_installed.len(), STARTERS.len());
        assert!(compute_plan(&paths, &state)
            .unwrap()
            .iter()
            .filter(|item| item.target == SyncTarget::Pi)
            .all(|item| matches!(item.status, SyncStatus::UpToDate)));
    }

    #[test]
    fn apply_safe_noop_when_up_to_date() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let first = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, first).unwrap();
        let second = compute_plan(&paths, &state).unwrap();
        let (state, outcomes) = apply_safe(&paths, state, second).unwrap();
        assert!(outcomes.iter().all(|o| o.action == "kept"));
        assert_eq!(state.installed.len(), STARTERS.len());
    }

    #[test]
    fn apply_safe_noop_preserves_manifest_bytes() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let install_plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, install_plan).unwrap();
        let compact_manifest = serde_json::to_vec(&state).unwrap();
        fs::write(&paths.state_file, &compact_manifest).unwrap();

        let noop_plan = compute_plan(&paths, &state).unwrap();
        let (_, outcomes) = apply_safe(&paths, state.clone(), noop_plan).unwrap();
        assert!(outcomes.iter().all(|o| o.action == "kept"));
        assert_eq!(fs::read(&paths.state_file).unwrap(), compact_manifest);
    }

    #[test]
    fn apply_safe_update_when_target_matches_last_installed() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let first = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, first).unwrap();

        // Modify canonical and verify safe update.
        let scout_path = paths.canonical_dir.join("scout.md");
        let prior = hash_file(&scout_path).unwrap();
        let mut agent = Agent::read(&scout_path).unwrap();
        agent.prompt.push_str("\nUpdated.");
        save_canonical(&paths, &agent, prior.as_deref()).unwrap();

        let plan = compute_plan(&paths, &state).unwrap();
        let scout = plan.iter().find(|i| i.filename == "scout.md").unwrap();
        assert_eq!(scout.status, SyncStatus::UpdateAvailable);
        let (_, outcomes) = apply_safe(&paths, state, plan).unwrap();
        let scout_outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
        assert_eq!(scout_outcome.action, "updated");
        let target = read_target(&paths, "scout").unwrap();
        assert!(target.contains("Updated."));
    }

    #[test]
    fn safe_sync_preserves_unowned_target() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        // Add an unowned target.
        let unowned = paths.target_dir.join("external.md");
        let original = "---\ndescription: external\nmode: subagent\n---\nbody\n";
        fs::write(&unowned, original).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let external = plan.iter().find(|i| i.filename == "external.md").unwrap();
        assert_eq!(external.status, SyncStatus::Unowned);
        let (_, outcomes) = apply_safe(&paths, state, plan).unwrap();
        let external_outcome = outcomes
            .iter()
            .find(|o| o.filename == "external.md")
            .unwrap();
        assert_eq!(external_outcome.action, "skipped");
        assert_eq!(fs::read_to_string(&unowned).unwrap(), original);
    }

    #[test]
    fn safe_sync_preserves_externally_modified_owned_target() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();

        // Externally modify the target.
        let target = paths.target_dir.join("scout.md");
        let original = fs::read_to_string(&target).unwrap();
        let mutated = original.replace("read-only codebase scout", "tampered");
        fs::write(&target, mutated.as_bytes()).unwrap();

        let plan = compute_plan(&paths, &state).unwrap();
        let scout = plan.iter().find(|i| i.filename == "scout.md").unwrap();
        assert_eq!(scout.status, SyncStatus::Conflict);
        let (_, outcomes) = apply_safe(&paths, state, plan).unwrap();
        let scout_outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
        assert_eq!(scout_outcome.action, "skipped");
        assert_eq!(fs::read_to_string(&target).unwrap(), mutated);
    }

    #[test]
    fn safe_removal_only_when_target_unchanged() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();

        // Delete canonical, leaving target intact.
        delete_canonical(&paths, "scout").unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let scout = plan.iter().find(|i| i.filename == "scout.md").unwrap();
        assert_eq!(scout.status, SyncStatus::Remove);
        let (state, _) = apply_safe(&paths, state, plan).unwrap();
        assert!(!paths.target_dir.join("scout.md").exists());

        // Modify a target that belongs to agenthd, then delete its canonical.
        let reviewer = paths.canonical_dir.join("reviewer.md");
        let prior = hash_file(&reviewer).unwrap();
        save_canonical(&paths, &Agent::read(&reviewer).unwrap(), prior.as_deref()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();
        let target = paths.target_dir.join("reviewer.md");
        let original = fs::read_to_string(&target).unwrap();
        fs::write(&target, original.replace("disciplined", "tampered")).unwrap();
        delete_canonical(&paths, "reviewer").unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let reviewer = plan.iter().find(|i| i.filename == "reviewer.md").unwrap();
        assert_eq!(reviewer.status, SyncStatus::PreserveModified);
        let (state, _) = apply_safe(&paths, state, plan).unwrap();
        assert!(paths.target_dir.join("reviewer.md").exists());
        assert!(!state.installed.contains_key("reviewer.md"));
    }

    #[test]
    fn force_install_overwrites_conflict() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();

        let target = paths.target_dir.join("scout.md");
        let original = fs::read_to_string(&target).unwrap();
        let mutated = original.replace("read-only", "tampered");
        fs::write(&target, mutated.as_bytes()).unwrap();

        let plan = compute_plan(&paths, &state).unwrap();
        let scout = plan.iter().find(|i| i.filename == "scout.md").unwrap();
        assert_eq!(scout.status, SyncStatus::Conflict);

        let (state, outcome) =
            force_install(&paths, state, SyncTarget::OpenCode, "scout.md").unwrap();
        assert!(outcome.ok);
        assert!(fs::read_to_string(&target).unwrap().contains("read-only"));
        assert!(state.installed.contains_key("scout.md"));
    }

    #[test]
    fn stale_manifest_entries_are_cleaned_up() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();

        // Wipe both canonical and target, leaving stale manifest.
        for starter in STARTERS {
            fs::remove_file(paths.canonical_dir.join(format!("{}.md", starter.name))).unwrap();
            fs::remove_file(paths.target_dir.join(format!("{}.md", starter.name))).unwrap();
            fs::remove_file(paths.pi_target_dir.join(format!("{}.md", starter.name))).unwrap();
        }
        let plan = compute_plan(&paths, &state).unwrap();
        assert!(plan.iter().all(|i| matches!(i.status, SyncStatus::Unowned)));
        let (state, _) = apply_safe(&paths, state, plan).unwrap();
        assert!(state.installed.is_empty());
    }

    #[test]
    fn state_recovery_when_source_and_target_match() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (_, _) = apply_safe(&paths, state, plan).unwrap();

        // Simulate losing the manifest.
        fs::remove_file(&paths.state_file).unwrap();
        let state = State::load(&paths.state_file).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        assert!(plan
            .iter()
            .all(|i| matches!(i.status, SyncStatus::UpToDate)));
        let (state, outcomes) = apply_safe(&paths, state, plan).unwrap();
        assert_eq!(state.installed.len(), STARTERS.len());
        assert!(outcomes.iter().all(|o| o.action == "kept"));
    }

    #[test]
    fn canonical_crud_and_rename() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        let mut new_agent = Agent::new_default("helper".to_string()).unwrap();
        new_agent.description = "A new helper agent".to_string();
        new_agent.prompt = "Do helpful things.".to_string();
        save_canonical(&paths, &new_agent, None).unwrap();
        assert!(paths.canonical_dir.join("helper.md").exists());

        let helper_path = paths.canonical_dir.join("helper.md");
        let prior = hash_file(&helper_path).unwrap();
        new_agent.prompt = "Do helpful things, more carefully.".to_string();
        save_canonical(&paths, &new_agent, prior.as_deref()).unwrap();
        let bytes = fs::read_to_string(&helper_path).unwrap();
        assert!(bytes.contains("more carefully"));

        rename_canonical(&paths, "helper", "assistant").unwrap();
        assert!(!paths.canonical_dir.join("helper.md").exists());
        assert!(paths.canonical_dir.join("assistant.md").exists());

        // Cannot rename onto existing file.
        let err = rename_canonical(&paths, "assistant", "scout");
        assert!(err.is_err());

        delete_canonical(&paths, "assistant").unwrap();
        assert!(!paths.canonical_dir.join("assistant.md").exists());
    }

    #[test]
    fn rejects_non_regular_target() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let first = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, first).unwrap();
        // Replace target with a directory.
        fs::remove_file(paths.target_dir.join("scout.md")).unwrap();
        fs::create_dir(paths.target_dir.join("scout.md")).unwrap();
        let result = compute_plan(&paths, &state);
        assert!(result.is_err());
    }

    #[test]
    fn atomic_write_target_replaces_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("foo.md");
        fs::write(&target, b"old").unwrap();
        write_target(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn save_canonical_rejects_collision_when_prior_is_none() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        let mut agent = Agent::read(&paths.canonical_dir.join("scout.md")).unwrap();
        // Pretend the user typed a name that collides with an existing starter.
        agent.name = "scout".to_string();
        let err = save_canonical(&paths, &agent, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to overwrite"));
        // The original bytes are untouched.
        assert!(fs::read_to_string(paths.canonical_dir.join("scout.md"))
            .unwrap()
            .contains("read-only codebase scout"));
    }

    #[test]
    fn save_canonical_rejects_stale_write() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        let scout_path = paths.canonical_dir.join("scout.md");
        let stale_hash = hash_file(&scout_path).unwrap();
        // Another writer mutates the file directly, bypassing save_canonical.
        let original = fs::read_to_string(&scout_path).unwrap();
        let external = original.replace("read-only codebase scout", "externally rewritten");
        fs::write(&scout_path, &external).unwrap();

        // The original editor tries to save with the stale hash.
        let mut agent = Agent::read(&scout_path).unwrap();
        agent.prompt = "Editor edit".to_string();
        let err = save_canonical(&paths, &agent, stale_hash.as_deref())
            .unwrap_err()
            .to_string();
        assert!(err.contains("changed on disk"));
        // The post-conflict bytes win, not the editor's stale draft.
        assert!(fs::read_to_string(&scout_path)
            .unwrap()
            .contains("externally rewritten"));
    }

    #[test]
    fn ghost_manifest_entry_has_distinct_reason() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut state = State::default();
        state
            .installed
            .insert("ghost.md".to_string(), "abc".to_string());
        let plan = compute_plan(&paths, &state).unwrap();
        let item = plan.iter().find(|i| i.filename == "ghost.md").unwrap();
        assert_eq!(item.status, SyncStatus::Unowned);
        assert!(item.reason().contains("stale manifest"));
    }

    #[test]
    fn force_install_adopts_when_target_now_matches_canonical() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, state) = seed_starters(&paths, State::default()).unwrap();
        let plan = compute_plan(&paths, &state).unwrap();
        let (state, _) = apply_safe(&paths, state, plan).unwrap();

        let target = paths.target_dir.join("scout.md");
        let original = fs::read_to_string(&target).unwrap();
        fs::write(&target, original.replace("read-only", "tampered")).unwrap();
        // Someone else syncs back to the canonical before we confirm.
        fs::write(&target, &original).unwrap();

        let (state, outcome) =
            force_install(&paths, state, SyncTarget::OpenCode, "scout.md").unwrap();
        assert_eq!(outcome.action, "kept");
        assert!(state.installed.contains_key("scout.md"));
    }

    #[test]
    fn malformed_state_is_reported() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        fs::create_dir_all(paths.state_file.parent().unwrap()).unwrap();
        fs::write(&paths.state_file, b"{not json").unwrap();
        let err = State::load(&paths.state_file).unwrap_err().to_string();
        assert!(err.contains("malformed"));
    }

    #[test]
    fn new_default_user_agent_has_safe_defaults() {
        let agent = Agent::new_default("helper".to_string()).unwrap();
        assert_eq!(agent.mode, Mode::subagent);
        assert_eq!(agent.permissions.get("edit"), Some(&PermissionAction::Ask));
        assert_eq!(agent.permissions.get("bash"), Some(&PermissionAction::Ask));
        assert_eq!(
            agent.permissions.get("external_directory"),
            Some(&PermissionAction::Ask)
        );
        assert!(agent.model.is_none());
    }

    #[test]
    fn plan_includes_manifest_only_target_as_unowned() {
        // Sanity: a manifest-only entry for an absent target is reported.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let mut state = State::default();
        state
            .installed
            .insert("ghost.md".to_string(), "abc".to_string());
        let plan = compute_plan(&paths, &state).unwrap();
        assert!(plan.iter().any(|i| i.filename == "ghost.md"));
        assert_eq!(
            plan.iter()
                .find(|i| i.filename == "ghost.md")
                .unwrap()
                .status,
            SyncStatus::Unowned
        );
    }

    #[test]
    fn plan_includes_targets_with_duplicate_unowned_state() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        fs::write(paths.target_dir.join("extra.md"), b"hello").unwrap();
        let state = State::default();
        let plan = compute_plan(&paths, &state).unwrap();
        assert!(plan.iter().any(|i| i.filename == "extra.md"));
    }

    #[test]
    fn source_hash_matches_render() {
        let agent = starter_agent(&STARTERS[0]);
        let h1 = source_hash(&agent);
        let h2 = sha256_hex(agent.render().as_bytes());
        assert_eq!(h1, h2);
    }

    #[test]
    fn load_canonical_collects_invalid_files() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        fs::write(
            paths.canonical_dir.join("good.md"),
            starter_agent(&STARTERS[0]).render(),
        )
        .unwrap();
        fs::write(paths.canonical_dir.join("bad.md"), b"not frontmatter").unwrap();
        let err = load_canonical(&paths).unwrap_err().to_string();
        assert!(err.contains("bad.md"));
    }

    #[test]
    fn update_bundled_prompts_replaces_prompt_keeps_other_fields() {
        // Description, mode, model, and permissions must survive the refresh.
        // Only the prompt body should change.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        // Customize scout's non-prompt fields AND its prompt so the refresh
        // actually has work to do. Otherwise the row would be reported as
        // "kept" and we would not be exercising the write path at all.
        let scout_path = paths.canonical_dir.join("scout.md");
        let prior = hash_file(&scout_path).unwrap();
        let mut agent = Agent::read(&scout_path).unwrap();
        let original_description = agent.description.clone();
        let original_mode = agent.mode;
        let original_model = agent.model.clone();
        let original_permissions = agent.permissions.clone();
        assert_eq!(original_mode, Mode::subagent);
        // Add a permission override that must survive the refresh.
        agent
            .permissions
            .insert("read".to_string(), PermissionAction::Deny);
        // Mutate the prompt so the refresh actually writes this file.
        agent.prompt = "user-edited scout prompt that should be replaced\n".to_string();
        save_canonical(&paths, &agent, prior.as_deref()).unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        let scout = outcomes
            .iter()
            .find(|o| o.name == "scout")
            .expect("scout outcome");
        assert!(scout.ok, "scout outcome not ok: {:?}", scout);
        assert_eq!(scout.action, "updated");

        let refreshed = Agent::read(&scout_path).unwrap();
        // Prompt is the bundled prompt body for scout.
        assert_eq!(
            refreshed.prompt,
            STARTERS.iter().find(|s| s.name == "scout").unwrap().prompt
        );
        // Other fields preserved.
        assert_eq!(refreshed.description, original_description);
        assert_eq!(refreshed.mode, original_mode);
        assert_eq!(refreshed.model, original_model);
        assert_eq!(
            refreshed.permissions.get("read"),
            Some(&PermissionAction::Deny),
            "user permission override must survive the refresh"
        );
        // Sanity: the customized prompt body really did differ before the
        // refresh, so the "updated" outcome is meaningful.
        assert_ne!(agent.prompt, refreshed.prompt);
        let _ = original_permissions; // referenced for clarity
    }

    #[test]
    fn update_bundled_prompts_reports_kept_when_already_current() {
        // Freshly-seeded files already match STARTERS, so the run is a no-op
        // and the report should say "kept" for each row.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        assert_eq!(outcomes.len(), STARTERS.len());
        for outcome in &outcomes {
            assert!(outcome.ok, "outcome not ok: {:?}", outcome);
            assert_eq!(
                outcome.action, "kept",
                "expected `kept` for fresh starter, got {:?}",
                outcome
            );
        }
    }

    #[test]
    fn update_bundled_prompts_skips_missing_files_and_updates_others() {
        // If the user deleted a bundled canonical, that row must be skipped
        // (startup seeding owns creation). The other rows are still refreshed.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        // Delete scout so the refresh must skip it.
        fs::remove_file(paths.canonical_dir.join("scout.md")).unwrap();

        // Pre-mutate another starter so we can detect the refresh applied.
        let worker_path = paths.canonical_dir.join("worker.md");
        let prior = hash_file(&worker_path).unwrap();
        let mut agent = Agent::read(&worker_path).unwrap();
        agent.prompt = "stale worker prompt\n".to_string();
        save_canonical(&paths, &agent, prior.as_deref()).unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        let scout = outcomes.iter().find(|o| o.name == "scout").unwrap();
        assert!(scout.ok);
        assert_eq!(scout.action, "skipped");
        assert!(
            scout.detail.contains("startup seeding"),
            "skip detail should explain the contract: {:?}",
            scout.detail
        );
        // scout.md still does not exist.
        assert!(!paths.canonical_dir.join("scout.md").exists());

        // worker.md was refreshed to the bundled prompt.
        let refreshed = Agent::read(&worker_path).unwrap();
        let bundled_worker = STARTERS.iter().find(|s| s.name == "worker").unwrap();
        assert_eq!(refreshed.prompt, bundled_worker.prompt);
    }

    #[test]
    fn update_bundled_prompts_leaves_user_created_agents_untouched() {
        // User-created agents (anything not in `STARTERS`) must not be
        // touched by the refresh — matching is by `STARTERS` membership,
        // not by file-name proximity.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        // Custom agent — must survive verbatim.
        let helper_path = paths.canonical_dir.join("helper.md");
        let helper_body = "---\ndescription: helper\nmode: subagent\n---\ncustom helper body\n";
        fs::write(&helper_path, helper_body).unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        assert!(
            outcomes.iter().all(|o| o.name != "helper"),
            "custom agent must not be in the refresh set: {:?}",
            outcomes
        );

        // helper.md is byte-for-byte unchanged.
        assert_eq!(fs::read_to_string(&helper_path).unwrap(), helper_body);
    }

    #[test]
    fn update_bundled_prompts_refreshes_planner_prompt_preserving_frontmatter() {
        // `planner` is part of `STARTERS`, so a user-customized planner.md
        // must be refreshed back to the bundled prompt body on confirm,
        // while description, mode, model, and permissions are preserved.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        let planner_path = paths.canonical_dir.join("planner.md");
        let prior = hash_file(&planner_path).unwrap();
        let mut agent = Agent::read(&planner_path).unwrap();
        let original_description = agent.description.clone();
        let original_mode = agent.mode;
        let original_model = agent.model.clone();
        let original_permissions = agent.permissions.clone();
        assert_eq!(original_mode, Mode::subagent);
        // User-set permission override must survive the refresh.
        agent
            .permissions
            .insert("read".to_string(), PermissionAction::Deny);
        // Mutate the prompt so the refresh actually has work to do.
        agent.prompt = "user-edited planner prompt that should be replaced\n".to_string();
        save_canonical(&paths, &agent, prior.as_deref()).unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        let planner = outcomes
            .iter()
            .find(|o| o.name == "planner")
            .expect("planner outcome");
        assert!(planner.ok, "planner outcome not ok: {:?}", planner);
        assert_eq!(planner.action, "updated");

        let refreshed = Agent::read(&planner_path).unwrap();
        // Prompt body is the bundled planner body.
        let bundled_planner = STARTERS.iter().find(|s| s.name == "planner").unwrap();
        assert_eq!(refreshed.prompt, bundled_planner.prompt);
        // Other frontmatter fields preserved.
        assert_eq!(refreshed.description, original_description);
        assert_eq!(refreshed.mode, original_mode);
        assert_eq!(refreshed.model, original_model);
        assert_eq!(
            refreshed.permissions.get("read"),
            Some(&PermissionAction::Deny),
            "user permission override must survive the refresh"
        );
        // The customized prompt body really did differ before the refresh.
        assert_ne!(agent.prompt, refreshed.prompt);
        // Read-only permissions are still in force after the refresh: the
        // user override on `read` (Deny) wins over the bundled default
        // (Allow), while edit/task are still denied and bash stays at ask.
        assert_eq!(
            refreshed.permissions.get("edit"),
            Some(&PermissionAction::Deny)
        );
        assert_eq!(
            refreshed.permissions.get("task"),
            Some(&PermissionAction::Deny)
        );
        assert_eq!(
            refreshed.permissions.get("bash"),
            Some(&PermissionAction::Ask)
        );
        assert_eq!(
            refreshed.permissions.get("read"),
            Some(&PermissionAction::Deny),
            "user permission override must survive the refresh"
        );
        let _ = original_permissions; // referenced for clarity
    }

    #[test]
    fn update_bundled_prompts_continues_after_per_file_failure() {
        // Simulate an external edit on scout.md between read and save so the
        // save_canonical stale-write guard rejects that one file. The other
        // five starters must still be refreshed.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        // Mutate every bundled file so the refresh wants to write all of
        // them, then externally rewrite scout.md's on-disk bytes after our
        // helper captures the prior hash. The simplest way to provoke the
        // stale-write path here is to make scout.md unreadable to the parser
        // by replacing it with bytes that fail frontmatter parsing. A parse
        // failure aborts only that one row.
        let scout_path = paths.canonical_dir.join("scout.md");
        // Capture prior hash for every starter so we can verify the
        // post-refresh content where appropriate.
        let worker_path = paths.canonical_dir.join("worker.md");
        let prior_worker = hash_file(&worker_path).unwrap();
        let mut worker_agent = Agent::read(&worker_path).unwrap();
        worker_agent.prompt = "stale worker prompt\n".to_string();
        save_canonical(&paths, &worker_agent, prior_worker.as_deref()).unwrap();

        // Make scout unparseable so the per-file read step fails for that
        // row only.
        fs::write(&scout_path, b"not a valid frontmatter document").unwrap();

        let outcomes = update_bundled_prompts(&paths).unwrap();
        let scout = outcomes.iter().find(|o| o.name == "scout").unwrap();
        assert!(!scout.ok, "scout should be reported as errored");
        assert_eq!(scout.action, "error");
        assert!(
            scout.detail.contains("read") || scout.detail.contains("frontmatter"),
            "scout error should mention the read/parse failure: {:?}",
            scout.detail
        );

        // The other starters were still processed independently.
        let worker = outcomes.iter().find(|o| o.name == "worker").unwrap();
        assert!(worker.ok, "worker should still be refreshed: {:?}", worker);
        let bundled_worker = STARTERS.iter().find(|s| s.name == "worker").unwrap();
        assert_eq!(
            Agent::read(&worker_path).unwrap().prompt,
            bundled_worker.prompt
        );

        // scout.md was not rewritten by the refresh: the stale bytes are
        // still on disk, so the user can recover manually.
        assert_eq!(
            fs::read_to_string(&scout_path).unwrap(),
            "not a valid frontmatter document"
        );
    }

    #[test]
    fn update_bundled_prompts_writes_exactly_when_prompt_differs() {
        // Regression guard: a starter whose prompt already matches must be
        // reported as "kept" with no write (no spurious mtime bump). A
        // starter whose prompt has been edited by the user must be reported
        // as "updated" with the bundled prompt restored.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let (_, _) = seed_starters(&paths, State::default()).unwrap();

        // Capture the original prompt bytes so we can detect mtime bumps.
        let delegate_path = paths.canonical_dir.join("delegate.md");
        let delegate_before = fs::metadata(&delegate_path).unwrap().modified().unwrap();

        // Edit scout's prompt so the refresh wants to write it.
        let scout_path = paths.canonical_dir.join("scout.md");
        let prior = hash_file(&scout_path).unwrap();
        let mut scout = Agent::read(&scout_path).unwrap();
        scout.prompt = "the user changed this on purpose\n".to_string();
        save_canonical(&paths, &scout, prior.as_deref()).unwrap();

        // Sleep long enough that any write produces a strictly newer mtime
        // on coarse-grained filesystems. macOS HFS+ has 1-second mtime
        // granularity, so use a small sleep here.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let outcomes = update_bundled_prompts(&paths).unwrap();
        let delegate = outcomes.iter().find(|o| o.name == "delegate").unwrap();
        assert_eq!(delegate.action, "kept", "delegate was a no-op");
        let delegate_after = fs::metadata(&delegate_path).unwrap().modified().unwrap();
        assert_eq!(
            delegate_before, delegate_after,
            "kept rows must not bump the file mtime"
        );

        let scout_outcome = outcomes.iter().find(|o| o.name == "scout").unwrap();
        assert_eq!(scout_outcome.action, "updated", "scout must be rewritten");
        let scout_after = fs::metadata(&scout_path).unwrap().modified().unwrap();
        assert!(
            scout_after > delegate_before,
            "updated row must produce a newer mtime"
        );
    }

    #[test]
    fn _ensure_mode_action_in_scope() {
        // Smoke test that the imports stay in scope.
        let mut map: BTreeMap<String, Mode> = BTreeMap::new();
        map.insert("a".into(), Mode::subagent);
        let _action = PermissionAction::Allow;
        assert!(map.contains_key("a"));
    }
}
