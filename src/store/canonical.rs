//! Canonical storage: load the canonical set, save / rename / delete
//! individual agent files, and open a single canonical agent for
//! editing (with its on-disk SHA-256 captured for the editor's
//! stale-write check).
//!
//! Everything here operates on `Paths.canonical_dir`. The path is
//! always the user-configured checkout's `agents/` directory; the
//! historical local-mode default (`<agenthd_root>/agents`) and the
//! bundled starter seed step are gone. Sync of the canonical bytes
//! to the OpenCode / Pi target directories lives in `super::sync`.

use super::{hash_file, require_canonical_source, sha256_hex, write_target, Paths};
use crate::agent::{canonical_path, Agent};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// Load all canonical agents. Errors include the file path.
///
/// The configured canonical source is validated up front: a missing,
/// moved, or symlinked checkout is an explicit error, never an
/// empty map. A valid empty directory (a freshly-cloned checkout
/// with no `.md` files) still produces an empty map; the runtime
/// surfaces that as "no agents" through the normal empty-list
/// path. The validation gate runs BEFORE any directory iteration
/// so the loader cannot silently return an empty set when the
/// source has been removed out-of-band.
pub fn load_canonical(paths: &Paths) -> Result<BTreeMap<String, (Agent, PathBuf)>> {
    require_canonical_source(paths)?;
    let mut agents = BTreeMap::new();
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

/// Load one existing canonical agent for the editor, capturing the
/// on-disk SHA-256 of the bytes the editor is going to edit.
///
/// The workflow is a fail-closed three-step gate:
///
/// 1. `require_canonical_source` rejects a missing / moved /
///    symlinked checkout before any read so the editor cannot
///    land bytes against a checkout that vanished out-of-band.
/// 2. `Agent::validate_name` rejects traversal-style names
///    (consecutive hyphens, `..`, non-ASCII, leading/trailing
///    hyphens, …) so `name.md` stays inside
///    `paths.canonical_dir`.
/// 3. `symlink_metadata` rejects symlinks and non-regular files
///    so the editor cannot be tricked into editing bytes that
///    live behind indirection the canonical contract forbids.
///
/// The bytes are read **once** into a `Vec<u8>`, then parsed and
/// hashed from that buffer. Hashing from the same bytes the
/// parser consumes pins the editor's stale-write contract to
/// exactly the source the parser saw — a re-read between parse
/// and hash would create a window where an external writer
/// could swap bytes under the editor.
///
/// `Ok((agent, prior_hash))` carries the lowercase hex SHA-256
/// the caller should pass back to `save_canonical` /
/// `workflows::save_agent`. The hash matches `hash_file(path)`
/// for the same on-disk bytes (verified by
/// `load_agent_for_edit_hashes_same_bytes_as_hash_file`).
pub fn load_agent_for_edit(paths: &Paths, name: &str) -> Result<(Agent, String)> {
    require_canonical_source(paths)?;
    Agent::validate_name(name).with_context(|| format!("invalid agent name `{}`", name))?;
    let path = canonical_path(&paths.canonical_dir, name)?;
    let meta = fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!(
            "agent file {} is a symlink; refusing to edit",
            path.display()
        );
    }
    if !meta.file_type().is_file() {
        bail!("agent file {} is not a regular file", path.display());
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let source = std::str::from_utf8(&bytes)
        .map_err(|e| anyhow!("agent file {} is not valid UTF-8: {}", path.display(), e))?;
    let agent = Agent::parse(name, source).with_context(|| format!("parse {}", path.display()))?;
    let prior_hash = sha256_hex(&bytes);
    Ok((agent, prior_hash))
}

/// Save an agent's current canonical bytes.
///
/// `prior_hash` is the SHA-256 of the canonical file observed by the
/// caller (e.g. when the editor was opened). Passing `None` for a
/// name that already exists on disk is a collision and is rejected;
/// passing `Some(h)` whose value does not match the current on-disk
/// hash is a stale-write race and is also rejected. The caller is
/// expected to reload from disk before retrying.
pub fn save_canonical(paths: &Paths, agent: &Agent, prior_hash: Option<&str>) -> Result<()> {
    agent.validate()?;
    // Validate the canonical source BEFORE any write so a missing
    // checkout cannot trigger `write_target`'s parent-directory
    // creation. The previous behavior would silently create a fresh
    // `<agenthd_root>/agents` (or worse, recreate the checkout's
    // `agents/` subtree) the moment a user clicked Save on an agent
    // after the checkout had been removed — turning a missing source
    // into a brand-new one and producing an empty file the next
    // planner pass would then treat as the configured canonical.
    require_canonical_source(paths)?;
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
    require_canonical_source(paths)?;
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

/// Delete a canonical agent file.
pub fn delete_canonical(paths: &Paths, name: &str) -> Result<()> {
    Agent::validate_name(name)?;
    require_canonical_source(paths)?;
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
