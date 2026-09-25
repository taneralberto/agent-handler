//! Canonical storage: seed starters, load the canonical set, save /
//! rename / delete individual agent files, and refresh the prompt body
//! of every bundled starter that already exists.
//!
//! Everything here operates on `Paths.canonical_dir`. Sync of the
//! canonical bytes to the OpenCode / Pi target directories lives in
//! `super::sync`.

use super::{hash_file, write_state, write_target, Paths, State};
use crate::agent::{canonical_path, starter_agent, Agent, STARTERS};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

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

/// Hash either the on-disk source or its parsed/rendered canonical form.
#[allow(dead_code)]
pub(in crate::store) fn source_hash(agent: &Agent) -> String {
    super::sha256_hex(agent.render().as_bytes())
}
