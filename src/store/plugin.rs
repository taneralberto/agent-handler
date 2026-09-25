//! Bundled OpenCode sidebar plugin: status, install, uninstall. The
//! plugin source is included as a build-time asset; the install /
//! uninstall paths own a single file under the global `plugins/`
//! directory and a single entry in `tui.json`.
//!
//! Writes use `super::write_target`; uninstall removes the owned plugin file.
//! `State.plugin_hash` in `.agenthd/state.json` marks ownership; `tui.json` holds the registry entry.

use super::{hash_file, sha256_hex, write_state, write_target, Paths, State};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::Path;

pub(super) const PLUGIN_SPEC: &str = "./plugins/agenthd-subagents.tsx";
pub(super) const PLUGIN_SOURCE: &str = include_str!("../../assets/agenthd-subagents.tsx");

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
