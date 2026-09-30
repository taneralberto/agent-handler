//! Per-machine checkout-path configuration.
//!
//! `agenthd` has exactly one source of truth for canonical agent
//! definitions: the `agents/` directory inside a single user-configured
//! checkout. The path to that checkout is the only piece of
//! machine-local configuration the runtime persists.
//!
//! The selection lives at `$HOME/.agenthd/settings.json` and is plain
//! JSON, written atomically (sibling temp file + rename) via
//! `super::write_target` so a crash mid-write cannot leave a
//! half-formed manifest.
//!
//! Validation:
//! - The configured path must be absolute, must exist, must be a
//!   directory, and must contain an `agents/` directory. An empty
//!   `agents/` directory is allowed — the user may have cleared it
//!   intentionally and the runtime surfaces that as "no agents"
//!   rather than refusing to start.
//! - Any failure surfaces an explicit error rather than silently
//!   falling back. Losing a configured checkout silently is much
//!   worse than refusing to start.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Filename of the per-machine settings inside the agenthd root.
/// Picked once and never renamed without a migration: this is the
/// single source of truth for "which directory is canonical today?".
pub(super) const SETTINGS_FILENAME: &str = "settings.json";

/// On-disk shape: one field, the absolute path to the configured
/// checkout. Kept deliberately small — every additional persisted
/// field is one more thing that has to be migrated when the contract
/// evolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub checkout_path: String,
}

impl Settings {
    pub fn new(checkout_path: impl Into<String>) -> Self {
        Self {
            checkout_path: checkout_path.into(),
        }
    }
}

/// Read the persisted settings, returning `Ok(None)` when no
/// settings file exists yet. A malformed file is an explicit error so
/// the user can recover by fixing the JSON or removing the file —
/// the implementation never silently falls back on parse failure.
pub fn load_settings(path: &Path) -> Result<Option<Settings>> {
    match fs::read(path) {
        Ok(bytes) => {
            let settings: Settings = serde_json::from_slice(&bytes).map_err(|e| {
                anyhow!(
                    "{} is malformed; remove it to recover: {}",
                    path.display(),
                    e
                )
            })?;
            Ok(Some(settings))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("read {}: {}", path.display(), e)),
    }
}

/// Atomically write the settings so a concurrent reader is
/// guaranteed to see the previous file or the new file, never a
/// torn JSON. The parent directory is created on demand so a fresh
/// `$HOME/.agenthd` can be set up without a separate `mkdir`.
pub fn save_settings(path: &Path, settings: &Settings) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(settings)?;
    super::write_target(path, &bytes)
}

/// Compute the absolute path of the per-machine settings file
/// inside the agenthd root. Centralized so tests and the binary
/// agree on the filename.
pub fn settings_file_path(agenthd_root: &Path) -> PathBuf {
    agenthd_root.join(SETTINGS_FILENAME)
}

/// Confirm a configured checkout is real, is a directory, and
/// carries an `agents/` directory. The path must be absolute so the
/// config stays portable across `cwd` changes, and the directory must
/// not be a symlink so the canonical source is never reachable
/// through an indirection the user did not explicitly author. An
/// empty `agents/` directory is allowed — the user may have cleared
/// it intentionally, and the runtime will surface that as "no
/// agents" rather than refusing to start. The check is intentionally
/// strict on the bits it can verify: a missing or relocated checkout
/// fails closed so the user is told *now* rather than letting the
/// runtime silently fall back.
pub fn validate_checkout_path(checkout_path: &Path) -> Result<()> {
    if !checkout_path.is_absolute() {
        bail!(
            "configured checkout path `{}` must be an absolute path",
            checkout_path.display()
        );
    }
    if !checkout_path.exists() {
        bail!(
            "configured checkout path `{}` does not exist; point settings.json at a checked-out repo or use the Settings menu to set a new path",
            checkout_path.display()
        );
    }
    let meta = fs::symlink_metadata(checkout_path)
        .with_context(|| format!("stat {}", checkout_path.display()))?;
    if meta.file_type().is_symlink() {
        bail!(
            "configured checkout path `{}` is a symlink; refusing to follow links for the canonical source root",
            checkout_path.display()
        );
    }
    if !meta.is_dir() {
        bail!(
            "configured checkout path `{}` is not a directory",
            checkout_path.display()
        );
    }
    let agents = checkout_path.join("agents");
    match fs::symlink_metadata(&agents) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => bail!("`{}/agents` is not a directory", checkout_path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "`{}/agents` does not exist; the checkout must contain an `agents/` directory",
            checkout_path.display()
        ),
        Err(e) => return Err(anyhow!("stat {}: {}", agents.display(), e)),
    }
    Ok(())
}

/// Resolve the canonical `agents/` directory the rest of the app
/// should use, given a `Settings`. The path is validated here (rather
/// than at write time) so a corrupt or stale settings file is caught
/// at startup. A failing validation here is the runtime's "missing
/// checkout fail closed" boundary: callers must surface the error
/// rather than papering over it with an empty canonical set.
pub fn canonical_dir_from(_agenthd_root: &Path, settings: &Settings) -> Result<PathBuf> {
    let raw = &settings.checkout_path;
    if raw.trim().is_empty() {
        bail!("settings.json has no checkout_path configured");
    }
    let path = PathBuf::from(raw);
    validate_checkout_path(&path)?;
    Ok(path.join("agents"))
}

/// Walk upward from `start` looking for a directory whose immediate
/// `agents/` child is a real directory. Returns the absolute path of
/// the first match, or `None`. The walk stops at the filesystem root
/// so a long chain of unrelated ancestors cannot loop forever.
///
/// This is intentionally cheap and side-effect free: it does not
/// call `validate_checkout_path`, which is stricter (refuses
/// symlinks, insists the path is absolute, etc.). Callers that
/// intend to consume the returned path as a configured checkout
/// must still run it through `validate_checkout_path` before
/// persisting.
///
/// The runtime does NOT use this helper to silently infer a
/// configured checkout. The single-source design treats any path it
/// would return as a hint at most; production code surfaces the
/// Settings screen gated with the on-disk truth instead. The helper
/// is kept here (and reachable from the `store` parent) so tests
/// and diagnostic tooling can inspect what the walk would have found
/// without baking the inference into the boot path.
#[cfg_attr(not(test), allow(dead_code))]
pub fn find_checkout_root_from(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start.to_path_buf());
    while let Some(dir) = current {
        let agents = dir.join("agents");
        if agents.is_dir() {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        assert!(load_settings(&settings_file_path(root)).unwrap().is_none());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let settings = Settings::new("/some/abs/path");
        save_settings(&settings_file_path(root), &settings).unwrap();
        let loaded = load_settings(&settings_file_path(root))
            .unwrap()
            .expect("settings file should exist");
        assert_eq!(loaded, settings);
    }

    #[test]
    fn malformed_settings_is_an_error() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let path = settings_file_path(root);
        fs::create_dir_all(root).unwrap();
        fs::write(&path, b"{not-json").unwrap();
        let err = load_settings(&path).unwrap_err().to_string();
        assert!(err.contains("malformed"), "got: {}", err);
    }

    #[test]
    fn canonical_dir_uses_checkout_agents_subdir() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let agents = root.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("scout.md"), b"---\n").unwrap();
        let settings = Settings::new(root.to_str().unwrap());
        let canonical = canonical_dir_from(&PathBuf::from("/tmp/agenthd-test"), &settings).unwrap();
        assert_eq!(canonical, root.join("agents"));
    }

    #[test]
    fn canonical_dir_rejects_missing_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("agents")).unwrap();
        // Non-existent subpath of an existing tempdir so the path
        // is absolute on the host platform (the Unix-style
        // literal `/this/path/...` is NOT absolute on Windows and
        // would trip the absolute-path gate instead of the
        // missing-checkout gate this test is asserting).
        let missing = dir.path().join("does-not-exist");
        let settings = Settings::new(missing.to_string_lossy());
        let err = canonical_dir_from(root, &settings).unwrap_err().to_string();
        assert!(err.contains("does not exist"), "got: {}", err);
    }

    #[test]
    fn canonical_dir_rejects_empty_checkout_path() {
        let settings = Settings::new("");
        let err = canonical_dir_from(&PathBuf::from("/tmp/agenthd-test"), &settings)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no checkout_path"), "got: {}", err);
    }

    #[test]
    fn validate_rejects_missing_path() {
        // Non-existent subpath of an existing tempdir so the path
        // is absolute on the host platform (the Unix-style
        // literal `/this/path/...` is NOT absolute on Windows and
        // would trip the absolute-path gate instead of the
        // missing-checkout gate this test is asserting).
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let err = validate_checkout_path(&missing).unwrap_err().to_string();
        assert!(err.contains("does not exist"), "got: {}", err);
    }

    #[test]
    fn validate_rejects_non_directory() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("not-a-dir");
        fs::write(&file, b"x").unwrap();
        let err = validate_checkout_path(&file).unwrap_err().to_string();
        assert!(err.contains("not a directory"), "got: {}", err);
    }

    #[test]
    fn validate_rejects_missing_agents_subdir() {
        let dir = TempDir::new().unwrap();
        // Empty tempdir, no `agents/` child.
        let err = validate_checkout_path(dir.path()).unwrap_err().to_string();
        assert!(err.contains("does not exist"), "got: {}", err);
    }

    #[test]
    fn validate_accepts_empty_agents_dir() {
        // An empty `agents/` directory is intentional: the user may
        // have cleared it. The runtime surfaces "no agents" through
        // the normal empty-canonical-list path instead of refusing to
        // start.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        validate_checkout_path(dir.path()).unwrap();
    }

    #[test]
    fn validate_accepts_agents_dir_with_non_md_files() {
        // Non-`.md` files in `agents/` are ignored by the loader; the
        // validator only insists on the directory existing.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("readme.txt"), b"hello").unwrap();
        validate_checkout_path(dir.path()).unwrap();
    }

    #[test]
    fn validate_rejects_relative_path() {
        // `validate_checkout_path` is also the entry point for
        // `--repo <path>` on the CLI; relative paths must be refused
        // up front so the user gets a clear error instead of a
        // config that breaks on the next `cd`.
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let rel = match cwd.join(dir.path()).strip_prefix(&cwd) {
            Ok(p) => p.to_path_buf(),
            Err(_) => return,
        };
        let err = validate_checkout_path(&rel).unwrap_err().to_string();
        assert!(
            err.contains("absolute"),
            "expected absolute-path rejection, got: {}",
            err
        );
    }

    #[test]
    fn validate_accepts_a_real_checkout() {
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("scout.md"), b"---\n").unwrap();
        validate_checkout_path(dir.path()).unwrap();
    }

    #[test]
    fn validate_rejects_symlinked_root() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let link = dir.path().join("link");
        // On Windows symlink_dir can fail without
        // SeCreateSymbolicLinkPrivilege; skip in that case so the
        // rest of the suite still runs.
        #[cfg(unix)]
        let made_link = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let made_link = std::os::windows::fs::symlink_dir(&target, &link).is_ok();
        if !made_link {
            // Skip the test in environments that can't create symlinks.
            return;
        }
        let err = validate_checkout_path(&link).unwrap_err().to_string();
        assert!(
            err.contains("symlink") || err.contains("not a directory"),
            "got: {}",
            err
        );
    }

    #[test]
    fn settings_file_path_lives_under_root() {
        let root = PathBuf::from("/tmp/agenthd-settings-test");
        assert_eq!(
            settings_file_path(&root),
            PathBuf::from("/tmp/agenthd-settings-test/settings.json")
        );
    }

    #[test]
    fn find_checkout_root_finds_ancestor_with_agents() {
        // The walk stops at the FIRST directory whose immediate
        // `agents/` child is a directory. With `agents/` inside the
        // deep directory itself, the function returns that deep
        // directory; with `agents/` only inside an ancestor, the
        // walk reaches that ancestor.
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("checkout");
        let deep = root.join("a").join("b").join("c");
        fs::create_dir_all(deep.join("agents")).unwrap();
        assert_eq!(find_checkout_root_from(&deep).unwrap(), deep);

        // Now move the `agents/` directory up so the walk has to
        // traverse ancestors before finding one.
        fs::remove_dir_all(deep.join("agents")).unwrap();
        fs::create_dir_all(root.join("agents")).unwrap();
        assert_eq!(find_checkout_root_from(&deep).unwrap(), root);
    }

    #[test]
    fn find_checkout_root_returns_none_when_no_ancestor_matches() {
        let dir = TempDir::new().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();
        assert!(find_checkout_root_from(&deep).is_none());
    }
}
