//! Third-party OpenCode skill installer (v1: install-only).
//!
//! Bundled static catalog (`DEFAULT_CATALOG`) is the single source of truth.
//! Adding another tool is one entry plus a tool-specific validation step
//! **only if** the default `SKILL.md` `name:` identity check is insufficient.
//!
//! v1 never updates, replaces, force-installs, adopts unowned targets, or
//! removes anything that already exists at the destination. Any existing
//! target — regular directory, regular file, or symlink — is a conflict that
//! refuses to install, a no-op.
//!
//! Module layout (preventive for future per-tool views):
//!
//! - `mod.rs` — shared types, the default catalog, and the install flow.
//! - `pi_psql/` — the bundled tool's catalog entry. A future per-tool
//!   view (config / execute UI) would live at `pi_psql/view.rs` and
//!   only when actually implemented; v1 ships no view.
//! - `tests.rs` — installer unit tests. The full coverage lives here
//!   and is preserved verbatim across the move (including the Windows
//!   npm-direct-`node` fix and the seam tests).

mod pi_psql;

use crate::store::Paths;
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One row in the bundled static catalog.
///
/// `pin_tag` and `expected_sha` are the verified immutable pin: `expected_sha`
/// is the peeled commit SHA at that tag, recorded as a `const` so the runtime
/// can refuse to publish from a tag that has moved. `node_min` is a full
/// `(major, minor, patch)` tuple compared against `node --version` to catch
/// engines.node legs below the catalog entry's bound (e.g. yargs 18's
/// `^22.12.0` requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMin {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolCatalogEntry {
    pub repo: &'static str,
    pub skill_name: &'static str,
    pub destination_subpath: &'static str,
    pub pin_tag: &'static str,
    pub expected_sha: &'static str,
    pub node_min: NodeMin,
}

/// Bundled static catalog. Adding a tool is one entry plus, only if needed,
/// a tool-specific validation step. The catalog is composed of one
/// `&[ToolCatalogEntry]` per tool module under `tools/` — adding a new tool
/// is one `pub const ENTRY` in a new `tools/<tool>/mod.rs` plus, only if
/// needed, a tool-specific validation step. `pi-psql` ships first; its
/// verified tag/SHA pair lives in `tools::pi_psql::ENTRY`.
pub const DEFAULT_CATALOG: &[ToolCatalogEntry] = &[pi_psql::ENTRY];

/// Status of a catalog entry at the destination. v1 has no `UpdateAvailable`
/// or `UpToDate` — the destination is either present (installed) or absent
/// (not installed), and any pre-existing entry other than a successful prior
/// install is a `Conflict` that the installer refuses to overwrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    NotInstalled,
    Installed,
    PrerequisitesMissing,
    IdentityMismatch,
    InstallFailed,
    Conflict,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            ToolStatus::NotInstalled => "not installed",
            ToolStatus::Installed => "installed",
            ToolStatus::PrerequisitesMissing => "prerequisites missing",
            ToolStatus::IdentityMismatch => "identity mismatch",
            ToolStatus::InstallFailed => "install failed",
            ToolStatus::Conflict => "conflict",
        }
    }
}

/// One row in the Tools screen.
#[derive(Debug, Clone)]
pub struct ToolItem {
    pub entry: &'static ToolCatalogEntry,
    pub status: ToolStatus,
    /// Human-readable explanation surfaced alongside the status. Kept on
    /// the row so callers can render it without re-running the install.
    #[allow(dead_code)]
    pub detail: String,
    pub destination: PathBuf,
}

/// Result of an `install_tool` run.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub status: ToolStatus,
    pub detail: String,
}

/// Per-spawn spec for the spawn runner closure. The runner receives explicit
/// argv, a working directory, and a timeout — never a shell, never a string
/// that gets re-parsed.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: &'static str,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct SpawnOutput {
    pub success: bool,
    /// Process exit code. `None` when the child was terminated by a signal
    /// or killed by the timeout watchdog. Mirrors `Child::status::code`.
    #[allow(dead_code)]
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Spawn runner closure type. The default implementation shells out via
/// `std::process::Command` with a timeout and reap; tests substitute a
/// canned-output mock so argv can be asserted exactly.
pub type SpawnRunner<'a> = &'a mut dyn FnMut(&SpawnSpec) -> Result<SpawnOutput>;

/// Rename runner closure type. The default implementation calls the OS
/// no-replace primitive (`renameat2` with `RENAME_NOREPLACE` on Linux, raw
/// `MoveFileW` on Windows); tests substitute a mock that returns the
/// desired `errno` / `GetLastError` value.
///
/// `Ok(())` means the rename succeeded atomically; `Err(code)` is the OS
/// error code (positive on both Linux and Windows).
pub type RenameRunner<'a> = &'a mut dyn FnMut(&Path, &Path) -> Result<(), i32>;

/// Default timeouts in seconds.
const GIT_TIMEOUT_SECS: u64 = 30;
const NPM_TIMEOUT_SECS: u64 = 120;
/// Maximum number of stderr lines surfaced in failure messages.
const STDERR_TAIL_LINES: usize = 20;

/// Compute the absolute destination for a catalog entry.
pub fn destination_for(paths: &Paths, entry: &ToolCatalogEntry) -> PathBuf {
    paths.skills_dir.join(entry.destination_subpath)
}

/// Compute the absolute staging directory for a catalog entry. Staging is
/// always adjacent to the destination on the same volume so the no-replace
/// primitive stays valid on both Linux (RENAME_NOREPLACE) and Windows
/// (MoveFileW requires same volume).
pub fn staging_for(paths: &Paths, entry: &ToolCatalogEntry) -> PathBuf {
    destination_for(paths, entry).with_extension(format!(
        ".staging-{}-{}",
        std::process::id(),
        next_staging_counter()
    ))
}

fn next_staging_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Inspect the destination to compute the current status for a catalog
/// entry. Pre-flight (git/node/npm/node_min) is **not** part of this read —
/// it runs lazily inside `install_tool` so `Tools` always renders without
/// spawning anything.
pub fn tool_status(paths: &Paths, entry: &'static ToolCatalogEntry) -> Result<ToolItem> {
    let destination = destination_for(paths, entry);
    let status = match fs::symlink_metadata(&destination) {
        Ok(meta) if meta.file_type().is_dir() => ToolStatus::Installed,
        Ok(meta) if meta.file_type().is_symlink() => ToolStatus::Conflict,
        Ok(meta) if meta.file_type().is_file() => ToolStatus::Conflict,
        Ok(_) => ToolStatus::Conflict,
        Err(e) if e.kind() == io::ErrorKind::NotFound => ToolStatus::NotInstalled,
        Err(e) => return Err(anyhow!("stat {}: {}", destination.display(), e)),
    };
    Ok(ToolItem {
        entry,
        status,
        detail: status.label().to_string(),
        destination,
    })
}

/// Public install entry point with default spawn/rename runners.
pub fn install_tool(paths: &Paths, entry: &ToolCatalogEntry) -> Result<ToolOutcome> {
    install_tool_with(
        paths,
        entry,
        &mut |spec| spawn_command(spec),
        &mut |src, dst| rename_no_replace(src, dst),
    )
}

/// Install `entry` into `paths.skills_dir/<destination_subpath>`. The eight
/// steps are run strictly in order; any step's failure removes the staging
/// directory (when one exists) and short-circuits with a typed outcome.
///
/// `spawn` and `rename` are injected so tests can assert argv exactly and
/// stub the OS no-replace primitive at the FFI/syscall layer.
pub fn install_tool_with(
    paths: &Paths,
    entry: &ToolCatalogEntry,
    spawn: SpawnRunner<'_>,
    rename: RenameRunner<'_>,
) -> Result<ToolOutcome> {
    install_tool_at(&staging_for(paths, entry), paths, entry, spawn, rename)
}

/// Install `entry` using an explicit `staging` directory. Tests use this to
/// place a sentinel at a known staging path and exercise the "preexisting
/// staging" rejection path without depending on the process-global staging
/// counter.
pub fn install_tool_at(
    staging: &Path,
    paths: &Paths,
    entry: &ToolCatalogEntry,
    spawn: SpawnRunner<'_>,
    rename: RenameRunner<'_>,
) -> Result<ToolOutcome> {
    let destination = destination_for(paths, entry);

    // Step 1: Pre-flight. Fail closed and never touch the destination.
    if let Err(detail) = preflight(spawn, entry) {
        return Ok(ToolOutcome {
            status: ToolStatus::PrerequisitesMissing,
            detail,
        });
    }

    // Step 2: Resolve tag -> peeled SHA via `git ls-remote`.
    let peeled = match ls_remote_peeled(spawn, entry) {
        Ok(sha) => sha,
        Err(detail) => {
            return Ok(ToolOutcome {
                status: ToolStatus::InstallFailed,
                detail,
            })
        }
    };

    // Sanity: the peeled SHA must match what we recorded in DEFAULT_CATALOG.
    // This is a defense-in-depth check before staging; the authoritative
    // pin check is step 4 (HEAD == expected_sha after checkout).
    if peeled != entry.expected_sha {
        return Ok(ToolOutcome {
            status: ToolStatus::InstallFailed,
            detail: format!(
                "tag {} peeled to {} does not match catalog pin {}; refusing to install",
                entry.pin_tag, peeled, entry.expected_sha
            ),
        });
    }

    // Step 3: Stage on the same filesystem / volume as the destination,
    // adjacent to the target.
    //
    // `stage` reserves the staging path via exclusive `create_dir`. If it
    // bails (e.g. a preexisting sibling occupies that path), we never
    // owned it, so the unconditional cleanup below would have clobbered
    // unrelated data. `stage` is responsible for cleaning up its own
    // internal failures (after it has taken ownership), so on Err we
    // simply forward.
    if let Err(detail) = stage(spawn, entry, staging) {
        return Ok(ToolOutcome {
            status: ToolStatus::InstallFailed,
            detail,
        });
    }
    // From this point on, the installer owns `staging`.

    // Step 4: Verify pinned SHA at HEAD.
    if let Err(detail) = verify_pinned_sha(spawn, staging, entry) {
        cleanup_staging(staging);
        return Ok(ToolOutcome {
            status: ToolStatus::InstallFailed,
            detail,
        });
    }

    // Step 5: Identity check (default SKILL.md `name:` only).
    if let Err(detail) = identity_check(staging, entry) {
        cleanup_staging(staging);
        return Ok(ToolOutcome {
            status: ToolStatus::IdentityMismatch,
            detail,
        });
    }

    // Step 6: Install deps via `npm ci`.
    if let Err(detail) = npm_ci(spawn, staging) {
        cleanup_staging(staging);
        return Ok(ToolOutcome {
            status: ToolStatus::InstallFailed,
            detail,
        });
    }

    // Step 7: Publish via the OS no-replace primitive.
    match rename(staging, &destination) {
        Ok(()) => {
            // staging is consumed by the rename; nothing to clean up.
            Ok(ToolOutcome {
                status: ToolStatus::Installed,
                detail: format!("published {}", destination.display()),
            })
        }
        Err(code) => {
            cleanup_staging(staging);
            // EEXIST (17) on Linux and ERROR_ALREADY_EXISTS (183) on Windows.
            if code == libc_const_eexist() || code == windows_const_already_exists() {
                Ok(ToolOutcome {
                    status: ToolStatus::Conflict,
                    detail: format!("{} already exists", destination.display()),
                })
            } else if code == libc_const_exdev() || code == windows_const_not_same_device() {
                Ok(ToolOutcome {
                    status: ToolStatus::InstallFailed,
                    detail: format!(
                        "{} is on a different filesystem than staging; refusing to copy",
                        destination.display()
                    ),
                })
            } else {
                Ok(ToolOutcome {
                    status: ToolStatus::InstallFailed,
                    detail: format!("publish failed with OS error code {}", code),
                })
            }
        }
    }
}

// ---------- Pre-flight (Step 1) ----------

/// Resolve the executable name to invoke for npm on this platform.
///
/// On Windows, npm is launched through `node` with the absolute path to
/// `npm-cli.js` prepended to argv — see `build_npm_spec` for the full
/// rationale. The function still exists (rather than being inlined) so
/// argv tests have a single source of truth and any future launcher
/// change stays in one place.
#[cfg(target_os = "windows")]
fn npm_program() -> &'static str {
    // npm is launched via `node <abs-path-to-npm-cli.js> ...`. The npm
    // shim `npm.cmd` is deliberately NOT used because Windows would
    // invoke `cmd.exe /c` to interpret it, violating the strict
    // no-shell contract documented at TOOL_INSTALLER_PLAN.md:219.
    "node"
}

#[cfg(not(target_os = "windows"))]
fn npm_program() -> &'static str {
    "npm"
}

/// Build the `SpawnSpec` that invokes npm for the current platform.
///
/// On Windows, the installer avoids the `npm.cmd` shim entirely (Windows
/// would invoke `cmd.exe /c` to interpret it, which violates the
/// project's strict no-shell contract — see `TOOL_INSTALLER_PLAN.md`
/// § Install path). Instead, this function spawns `node` with the
/// absolute path to `npm-cli.js` as `argv[0]`. The path is resolved by
/// walking PATH for `npm.cmd` entries and using each launcher's parent
/// directory to derive `<parent>/node_modules/npm/bin/npm-cli.js` — see
/// `resolve_npm_cli_js` and `resolve_npm_cli_js_in` for the rules,
/// including the absolute-path guarantee.
///
/// On non-Windows platforms, this spawns `npm` directly with
/// `argv_suffix` as its argv.
///
/// `argv_suffix` is concatenated into the final `Vec<String>` without
/// further processing. The current callers pass only `--version` and
/// `["ci", "--omit=dev", "--ignore-scripts"]`. No shell is involved at
/// any layer, so no shell-metacharacter constraints apply — but the
/// argv entries must still be hardcoded literals (no user input), per
/// the no-shell contract.
///
/// Returns `Err` on Windows when `npm-cli.js` cannot be located next to
/// any `npm.cmd` in PATH. The caller treats that as
/// `PrerequisitesMissing` — a clean, actionable failure that names the
/// missing prerequisite.
fn build_npm_spec(
    argv_suffix: &[&str],
    cwd: Option<PathBuf>,
    timeout: Duration,
) -> Result<SpawnSpec, String> {
    let mut args: Vec<String> = Vec::with_capacity(argv_suffix.len() + 1);
    #[cfg(target_os = "windows")]
    {
        let cli = resolve_npm_cli_js().ok_or_else(|| {
            // No `npm:` prefix here on purpose: callers (`preflight`,
            // `npm_ci`) wrap the error with their own `npm:` / `npm ci:`
            // context. Prefixing here would produce `npm: npm: cannot
            // locate npm-cli.js ...` in the user-visible detail.
            "cannot locate npm-cli.js next to any npm.cmd in PATH; \
             reinstall Node.js so the canonical npm layout is restored"
                .to_string()
        })?;
        args.push(cli.display().to_string());
    }
    for a in argv_suffix {
        args.push((*a).to_string());
    }
    Ok(SpawnSpec {
        program: npm_program(),
        args,
        cwd,
        timeout,
    })
}

/// Locate `npm-cli.js` next to an `npm.cmd` entry in PATH.
///
/// Walks PATH entries in order. For each entry whose joined `npm.cmd`
/// is a regular file, derives the canonical candidate
/// `<launcher_parent>/node_modules/npm/bin/npm-cli.js` and returns it if
/// the file exists.
///
/// **We deliberately do NOT spawn the `npm.cmd` shim or parse its
/// contents.** Spawning it would invoke `cmd.exe`, and parsing it would
/// re-introduce the shell we are avoiding. The launcher is used purely
/// as a layout hint — its directory is expected to be the canonical
/// npm install root, with `node_modules/npm/bin/npm-cli.js` next to it.
/// This matches both standard Windows npm layouts:
///
///   - `C:\Program Files\nodejs\npm.cmd`
///     → `C:\Program Files\nodejs\node_modules\npm\bin\npm-cli.js`
///   - `C:\Users\<u>\AppData\Roaming\npm\npm.cmd`
///     → `C:\Users\<u>\AppData\Roaming\npm\node_modules\npm\bin\npm-cli.js`
///
/// **Absolute-path guarantee.** Relative PATH entries (`.`, `bin`,
/// `subdir/...`) are absolutized against the process cwd before the
/// candidate is computed. Without this step, a relative PATH entry would
/// produce a relative candidate; that candidate would still resolve
/// correctly when `Command::new` reads it (CreateProcess resolves
/// against the **invocation-time** cwd), but `npm ci` runs with `cwd =
/// staging`, not the installer's cwd. A relative argv[0] under `node`
/// would then look for `npm-cli.js` relative to the staging dir and fail
/// silently with `Error { kind: NotFound }`. Absolutizing at discovery
/// time avoids that whole class of bug regardless of the spawn cwd.
///
/// Returns `None` when no PATH entry resolves to a usable
/// `npm-cli.js`, or when the process cwd is not available. The caller
/// (`build_npm_spec`) maps `None` to `PrerequisitesMissing`.
///
/// The pure helper is `resolve_npm_cli_js_in(&OsStr)`; the
/// environment-reading wrapper is `resolve_npm_cli_js()`. The split lets
/// tests exercise the discovery logic without touching the process PATH
/// or the process cwd.
#[cfg(target_os = "windows")]
fn resolve_npm_cli_js_in(path: &std::ffi::OsStr) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for entry in std::env::split_paths(path) {
        // Absolutize the entry first so the candidate path is always
        // absolute. We do NOT canonicalize (no symlink resolution, no
        // `\\?\` UNC prefix); we only prepend cwd if the entry is
        // relative. This keeps the candidate usable as an argv[0] for
        // `node` without surprises.
        let abs_entry = if entry.is_absolute() {
            entry
        } else {
            cwd.join(&entry)
        };
        let launcher = abs_entry.join("npm.cmd");
        let meta = match std::fs::metadata(&launcher) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let parent = match launcher.parent() {
            Some(p) => p,
            None => continue,
        };
        let candidate = parent
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js");
        if std::fs::metadata(&candidate)
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Some(candidate);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn resolve_npm_cli_js() -> Option<PathBuf> {
    // Test seam: tests can force a specific result for the current
    // thread (see `discovery_seam`). In production we read PATH from
    // the process environment.
    #[cfg(test)]
    {
        if let Some(forced) = discovery_seam::get() {
            return forced;
        }
    }
    let path = std::env::var_os("PATH")?;
    resolve_npm_cli_js_in(&path)
}

/// Test-only seam for `resolve_npm_cli_js`.
///
/// Production reads PATH from the process environment, but parallel
/// tests cannot mutate that safely. This seam lets a test force a
/// specific return value for the duration of its scope (the override is
/// `thread_local`, so concurrent tests are isolated). `set(None)`
/// restores the production behaviour.
///
/// `Some(None)` simulates "no npm-cli.js anywhere in PATH" → preflight
/// must return `PrerequisitesMissing` mentioning npm.
/// `Some(Some(p))` simulates "npm-cli.js at p" → preflight must succeed
/// and the resulting `SpawnSpec.args[0]` must equal `p`.
#[cfg(all(target_os = "windows", test))]
mod discovery_seam {
    use std::cell::RefCell;
    use std::path::PathBuf;
    thread_local! {
        static OVERRIDE: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
    }
    pub fn get() -> Option<Option<PathBuf>> {
        OVERRIDE.with(|c| c.borrow().clone())
    }
    pub fn set(value: Option<Option<PathBuf>>) {
        OVERRIDE.with(|c| *c.borrow_mut() = value);
    }
}

/// Drop-guard wrapper for the npm-cli.js discovery seam.
///
/// Two flavors share this single Drop type so the test surface stays
/// tiny and parallel-safe:
///
/// - `fake_npm_cli_guard()` forces the seam to `Some(Some(path))`,
///   making `resolve_npm_cli_js()` return a known npm-cli.js path
///   regardless of the host's PATH. Use this in tests that exercise
///   the installer's full pipeline and assume npm preflight succeeds.
/// - `fake_npm_missing_guard()` forces the seam to `Some(None)`,
///   simulating "npm not installed" / unsupported layout. Use this in
///   tests that assert the `PrerequisitesMissing` failure path with an
///   actionable "npm-cli.js not in PATH" detail.
///
/// Both restore the previous seam value on drop, so a panic in the
/// test body cannot leak state into the next test on this thread.
///
/// On non-Windows the seam does not exist; both guards compile to
/// empty structs that Drop as a no-op, and `build_npm_spec` falls back
/// to a direct `npm` spawn.
#[cfg(all(target_os = "windows", test))]
struct FakeSeamGuard {
    saved: Option<Option<std::path::PathBuf>>,
}

#[cfg(all(target_os = "windows", test))]
impl Drop for FakeSeamGuard {
    fn drop(&mut self) {
        discovery_seam::set(self.saved.clone());
    }
}

#[cfg(all(target_os = "windows", test))]
fn fake_npm_cli_guard() -> FakeSeamGuard {
    let saved = discovery_seam::get();
    // A stable, host-independent path used only to drive
    // `build_npm_spec`. Tests that exercise the OS (i.e. that actually
    // spawn `node`) look up the real path themselves via
    // `resolve_npm_cli_js()` and skip when not available.
    discovery_seam::set(Some(Some(std::path::PathBuf::from(
        r"C:\fake-for-test\node_modules\npm\bin\npm-cli.js",
    ))));
    FakeSeamGuard { saved }
}

#[cfg(all(target_os = "windows", test))]
fn fake_npm_missing_guard() -> FakeSeamGuard {
    let saved = discovery_seam::get();
    discovery_seam::set(Some(None));
    FakeSeamGuard { saved }
}

#[cfg(not(target_os = "windows"))]
struct FakeSeamGuard;

#[cfg(not(target_os = "windows"))]
fn fake_npm_cli_guard() -> FakeSeamGuard {
    FakeSeamGuard
}

#[cfg(not(target_os = "windows"))]
fn fake_npm_missing_guard() -> FakeSeamGuard {
    FakeSeamGuard
}

fn preflight(spawn: SpawnRunner<'_>, entry: &ToolCatalogEntry) -> Result<(), String> {
    // git --version
    let git = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "git",
            args: vec!["--version".to_string()],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("git: {}", e))?;
    if !git.success {
        return Err(format!(
            "git --version failed: {}",
            stderr_tail(&git.stderr)
        ));
    }

    // node --version (parse + compare full tuple against entry.node_min)
    let node = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "node",
            args: vec!["--version".to_string()],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("node: {}", e))?;
    if !node.success {
        return Err(format!(
            "node --version failed: {}",
            stderr_tail(&node.stderr)
        ));
    }
    let parsed = parse_node_version(&String::from_utf8_lossy(&node.stdout))
        .ok_or_else(|| "node --version output is not parseable".to_string())?;
    if !node_meets(parsed, entry.node_min) {
        return Err(format!(
            "node {} is below the required minimum {}.{}.{} (per skill's engines.node)",
            format_version(parsed),
            entry.node_min.major,
            entry.node_min.minor,
            entry.node_min.patch,
        ));
    }

    // npm --version. On Windows this spawns `node <abs-npm-cli.js> --version`
    // (no `npm.cmd` — see build_npm_spec); on other platforms `npm --version`.
    let npm_spec = build_npm_spec(&["--version"], None, Duration::from_secs(GIT_TIMEOUT_SECS))
        .map_err(|e| format!("npm: {}", e))?;
    let npm = run_with_timeout(spawn, &npm_spec).map_err(|e| format!("npm: {}", e))?;
    if !npm.success {
        return Err(format!(
            "npm --version failed: {}",
            stderr_tail(&npm.stderr)
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NodeVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

fn format_version(v: NodeVersion) -> String {
    format!("{}.{}.{}", v.major, v.minor, v.patch)
}

/// Parse `node --version` output (`vX.Y.Z` or `vX.Y.Z-something`) into a
/// tuple. Returns `None` for anything that does not parse — caller treats
/// that as a preflight failure.
fn parse_node_version(text: &str) -> Option<NodeVersion> {
    // Strip leading "v" and any whitespace, then take everything up to the
    // first non-digit/dot character. This accepts `v22.12.0`, `v22.12.0\n`,
    // and `v22.12.0-something` while rejecting partial / non-version lines.
    let trimmed = text.trim().trim_start_matches('v');
    let mut digit_count = 0;
    for ch in trimmed.chars() {
        if ch == '.' || ch.is_ascii_digit() {
            digit_count += 1;
            continue;
        }
        break;
    }
    if digit_count == 0 {
        return None;
    }
    let head = &trimmed[..digit_count];
    let mut parts = head.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    Some(NodeVersion {
        major,
        minor,
        patch,
    })
}

fn node_meets(actual: NodeVersion, required: NodeMin) -> bool {
    if actual.major != required.major {
        return actual.major > required.major;
    }
    if actual.minor != required.minor {
        return actual.minor > required.minor;
    }
    actual.patch >= required.patch
}

// ---------- Step 2: ls-remote peeled SHA ----------

fn ls_remote_peeled(spawn: SpawnRunner<'_>, entry: &ToolCatalogEntry) -> Result<String, String> {
    // The authoritative pin check is step 4 (HEAD == expected_sha after
    // checkout). Step 2 is a defense-in-depth sanity check: confirm the
    // tag still resolves to the commit SHA we recorded, so a tag moved
    // upstream is caught before we burn time on a fetch.
    //
    // `git ls-remote <repo> <pin_tag>` emits one line for both annotated
    // and lightweight tags — but for an *annotated* tag, that line is the
    // tag object SHA, not the commit SHA. The peeled commit line
    // (`refs/tags/<tag>^{}`) is only emitted when the remote returns
    // multiple refs for the refspec. We therefore call BOTH
    // `ls-remote <tag>` (verify tag is found; covers lightweight tags
    // where the returned SHA already is the commit) AND
    // `ls-remote <tag>^{}` (get the peeled commit for annotated tags).
    // The non-empty one wins. `ls-remote <repo> <sha>` is intentionally
    // not used — many servers do not expose arbitrary commit SHAs through
    // refs.
    let mut tag_seen = false;
    let mut from_tag_filter: Option<String> = None;

    let out = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "git",
            args: vec![
                "ls-remote".to_string(),
                entry.repo.to_string(),
                entry.pin_tag.to_string(),
            ],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("ls-remote spawn: {}", e))?;
    if !out.success {
        return Err(format!(
            "git ls-remote failed: {}",
            stderr_tail(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for sha in parse_shas(&text) {
        tag_seen = true;
        from_tag_filter = Some(sha);
    }

    // Now ask explicitly for the peeled commit. Annotated tags return one
    // line; lightweight tags return nothing through this refspec.
    let peeled_out = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "git",
            args: vec![
                "ls-remote".to_string(),
                entry.repo.to_string(),
                format!("{}^{{}}", entry.pin_tag),
            ],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("ls-remote peel spawn: {}", e))?;
    let peeled_text = String::from_utf8_lossy(&peeled_out.stdout);
    let explicit_peeled = parse_shas(&peeled_text).into_iter().next();

    if !tag_seen && explicit_peeled.is_none() {
        return Err(format!(
            "tag {} was not found at {}",
            entry.pin_tag, entry.repo
        ));
    }

    explicit_peeled
        .or(from_tag_filter)
        .ok_or_else(|| format!("no SHA returned for tag {}", entry.pin_tag))
}

/// Parse 40-hex SHAs out of arbitrary `git ls-remote` text. Returns them
/// in input order so the caller can pick the right one (e.g. the peeled
/// line for annotated tags, the single line for lightweight tags).
fn parse_shas(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(hash) = line.split_whitespace().next() {
            if is_40_hex(hash) {
                out.push(hash.to_ascii_lowercase());
            }
        }
    }
    out
}

fn is_40_hex(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || (b'A'..=b'F').contains(&b))
}

// ---------- Step 3: stage ----------

fn stage(spawn: SpawnRunner<'_>, entry: &ToolCatalogEntry, staging: &Path) -> Result<(), String> {
    // Reserve the staging path via exclusive `create_dir`. We must NEVER
    // remove a preexisting path at this location: if the user (or some
    // prior unrelated process) has a regular dir, file, or symlink at
    // the computed staging path, deleting it would clobber unrelated
    // data. The unique PID+counter suffix makes collisions very rare,
    // but the safety contract does not depend on that.
    //
    // `fs::create_dir` returns AlreadyExists for any pre-existing path
    // at the destination — dir, file, or symlink — which is exactly the
    // bail signal we want. Once the empty dir is created here, the
    // installer owns it and `cleanup_staging` is safe on failure paths.
    match fs::create_dir(staging) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "staging path {} already exists; refusing to clobber it",
                staging.display()
            ));
        }
        Err(e) => {
            return Err(format!("create staging {}: {}", staging.display(), e));
        }
    }

    // Helper that wraps a single `git` step. Any failure here is a
    // failure of THIS install attempt, so the staging dir (which we
    // own from this point on) is cleaned up before returning.
    let mut run_git = |label: &'static str, argv: Vec<String>| -> Result<(), String> {
        let out = run_with_timeout(
            spawn,
            &SpawnSpec {
                program: "git",
                args: argv,
                cwd: None,
                timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
            },
        )
        .map_err(|e| format!("git {} spawn: {}", label, e))?;
        if !out.success {
            return Err(format!(
                "git {} failed: {}",
                label,
                stderr_tail(&out.stderr)
            ));
        }
        Ok(())
    };

    // git init <staging>
    if let Err(detail) = run_git(
        "init",
        vec!["init".to_string(), staging.display().to_string()],
    ) {
        cleanup_staging(staging);
        return Err(detail);
    }

    // git -C <staging> remote add origin <repo>
    if let Err(detail) = run_git(
        "remote add",
        vec![
            "-C".to_string(),
            staging.display().to_string(),
            "remote".to_string(),
            "add".to_string(),
            "origin".to_string(),
            entry.repo.to_string(),
        ],
    ) {
        cleanup_staging(staging);
        return Err(detail);
    }

    // git -C <staging> fetch --depth=1 origin <pin_tag>
    // Fetch by tag, NOT by SHA — many servers refuse unreachable commit SHAs.
    if let Err(detail) = run_git(
        "fetch",
        vec![
            "-C".to_string(),
            staging.display().to_string(),
            "fetch".to_string(),
            "--depth=1".to_string(),
            "origin".to_string(),
            entry.pin_tag.to_string(),
        ],
    ) {
        cleanup_staging(staging);
        return Err(detail);
    }

    // git -C <staging> checkout FETCH_HEAD
    if let Err(detail) = run_git(
        "checkout",
        vec![
            "-C".to_string(),
            staging.display().to_string(),
            "checkout".to_string(),
            "FETCH_HEAD".to_string(),
        ],
    ) {
        cleanup_staging(staging);
        return Err(detail);
    }

    Ok(())
}

// ---------- Step 4: pinned SHA ----------

fn verify_pinned_sha(
    spawn: SpawnRunner<'_>,
    staging: &Path,
    entry: &ToolCatalogEntry,
) -> Result<(), String> {
    let out = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "git",
            args: vec![
                "-C".to_string(),
                staging.display().to_string(),
                "rev-parse".to_string(),
                "HEAD".to_string(),
            ],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("git rev-parse spawn: {}", e))?;
    if !out.success {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            stderr_tail(&out.stderr)
        ));
    }
    let head = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|s| s.trim().to_ascii_lowercase())
        .ok_or_else(|| "git rev-parse HEAD produced no output".to_string())?;
    if head != entry.expected_sha.to_ascii_lowercase() {
        return Err(format!(
            "HEAD is {} but catalog pin is {}; refusing to install",
            head, entry.expected_sha
        ));
    }
    Ok(())
}

// ---------- Step 5: identity check (SKILL.md frontmatter name) ----------

fn identity_check(staging: &Path, entry: &ToolCatalogEntry) -> Result<(), String> {
    let skill = staging.join("SKILL.md");
    let bytes = fs::read(&skill).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            format!("staged tree has no SKILL.md at {}", skill.display())
        } else {
            format!("read {}: {}", skill.display(), e)
        }
    })?;
    let text =
        std::str::from_utf8(&bytes).map_err(|e| format!("SKILL.md is not valid UTF-8: {}", e))?;
    let found = parse_skill_name(text)
        .ok_or_else(|| "SKILL.md frontmatter is malformed or missing `name:`".to_string())?;
    if found != entry.skill_name {
        return Err(format!(
            "SKILL.md frontmatter `name: {}` does not match catalog skill `{}`",
            found, entry.skill_name
        ));
    }
    Ok(())
}

/// Parse the `name:` scalar from a SKILL.md frontmatter block. The skill
/// frontmatter is YAML between two `---` markers; we only need the `name:`
/// line, which must be a flat scalar (no nested mapping). Quoted scalars
/// have their surrounding quotes stripped.
pub(crate) fn parse_skill_name(text: &str) -> Option<String> {
    // Normalize CRLF so Windows-authored files parse identically.
    let text = if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    };
    let body = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = body.find("\n---")?;
    let front = &body[..end];
    for raw_line in front.lines() {
        let line = raw_line.trim_start();
        let Some(rest) = line.strip_prefix("name:") else {
            continue;
        };
        let rest = rest.trim_start();
        // Drop a trailing inline comment if any.
        let rest = rest.split(" #").next().unwrap_or(rest);
        let value = rest.trim();
        if value.is_empty() {
            return None;
        }
        // Strip a single layer of matching quotes.
        let stripped = if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        return Some(stripped.to_string());
    }
    None
}

// ---------- Step 6: npm ci ----------

fn npm_ci(spawn: SpawnRunner<'_>, staging: &Path) -> Result<(), String> {
    // On Windows this spawns `node <abs-npm-cli.js> ci --omit=dev
    // --ignore-scripts` with cwd = staging. The absolute path to the
    // CLI script is resolved once per call (see build_npm_spec); we do
    // NOT rely on `npm.cmd` (which would invoke cmd.exe and violate the
    // no-shell contract).
    let spec = build_npm_spec(
        &["ci", "--omit=dev", "--ignore-scripts"],
        Some(staging.to_path_buf()),
        Duration::from_secs(NPM_TIMEOUT_SECS),
    )
    .map_err(|e| format!("npm ci: {}", e))?;
    let out = run_with_timeout(spawn, &spec).map_err(|e| format!("npm ci spawn: {}", e))?;
    if !out.success {
        return Err(format!(
            "npm ci --omit=dev --ignore-scripts failed: {}",
            stderr_tail(&out.stderr)
        ));
    }
    Ok(())
}

// ---------- Step 7: publish ----------

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
        Err(io::Error::last_os_error().raw_os_error().unwrap_or(0))
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
        // The rename primitive reports failure by returning `Err(err)`;
        // `Ok(Err(err))` would typecheck as `Result<Result<(), i32>, ()>`,
        // which is not the function's contract — and is also exactly the
        // caller-side trap that hides the conflict from the installer
        // (`Ok(())` matches the success arm regardless of inner Err).
        // GetLastError returns u32; cast to i32 so the function's
        // `Result<(), i32>` contract holds and the installer's typed
        // match arm sees the conflict code.
        Err(unsafe { GetLastError() } as i32)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn rename_no_replace(_src: &Path, _dst: &Path) -> Result<(), i32> {
    // Fail closed: no proven no-replace primitive on this OS.
    Err(0)
}

#[cfg(target_os = "linux")]
fn libc_const_eexist() -> i32 {
    libc::EEXIST
}

#[cfg(not(target_os = "linux"))]
fn libc_const_eexist() -> i32 {
    0
}

#[cfg(target_os = "linux")]
fn libc_const_exdev() -> i32 {
    libc::EXDEV
}

#[cfg(not(target_os = "linux"))]
fn libc_const_exdev() -> i32 {
    0
}

#[cfg(target_os = "windows")]
fn windows_const_already_exists() -> i32 {
    windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32
}

#[cfg(not(target_os = "windows"))]
fn windows_const_already_exists() -> i32 {
    0
}

#[cfg(target_os = "windows")]
fn windows_const_not_same_device() -> i32 {
    windows_sys::Win32::Foundation::ERROR_NOT_SAME_DEVICE as i32
}

#[cfg(not(target_os = "windows"))]
fn windows_const_not_same_device() -> i32 {
    0
}

// ---------- Shared spawn helper ----------

fn run_with_timeout(spawn: SpawnRunner<'_>, spec: &SpawnSpec) -> Result<SpawnOutput> {
    spawn(spec).with_context(|| format!("spawn `{}`", spec.program))
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[start..].join("\n")
}

fn cleanup_staging(staging: &Path) {
    let _ = fs::remove_dir_all(staging);
}

// ---------- Real-spawn implementation ----------

fn spawn_command(spec: &SpawnSpec) -> Result<SpawnOutput> {
    let mut cmd = Command::new(spec.program);
    cmd.args(spec.args.iter().map(|s| s.as_str()));
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn `{}`", spec.program))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("`{}` stdout was not piped", spec.program))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("`{}` stderr was not piped", spec.program))?;

    // Drain stdout/stderr concurrently so a chatty child cannot fill the
    // OS pipe buffer (typically 64 KiB on Linux) and deadlock waiting for
    // the parent to read. Both threads terminate when the child's write
    // end closes (after the child exits); we join them after reaping.
    // A 1 MiB cap per stream prevents a runaway child from pinning memory
    // — any bytes past the cap are dropped, which is fine for the only
    // downstream consumer (stderr_tail in failure messages).
    const OUTPUT_CAP_BYTES: usize = 1 << 20;
    let stdout_handle = std::thread::spawn(move || drain_capped(stdout, OUTPUT_CAP_BYTES));
    let stderr_handle = std::thread::spawn(move || drain_capped(stderr, OUTPUT_CAP_BYTES));

    let start = Instant::now();
    let timeout = spec.timeout;

    // Reap loop: try_wait until the child exits or the timeout elapses.
    // On timeout we kill+wait so the child does not leak as a zombie and
    // the drain threads see EOF on their pipes.
    let status = loop {
        match child
            .try_wait()
            .with_context(|| format!("try_wait `{}`", spec.program))?
        {
            Some(status) => break status,
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    break child
                        .wait()
                        .with_context(|| format!("wait `{}`", spec.program))?;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let timed_out = start.elapsed() >= timeout;

    let stdout = stdout_handle.join().unwrap_or_else(|_| Vec::new());
    let stderr = stderr_handle.join().unwrap_or_else(|_| Vec::new());

    if timed_out {
        bail!("`{}` timed out after {}s", spec.program, timeout.as_secs());
    }

    Ok(SpawnOutput {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr,
    })
}

/// Read `reader` to a buffer, capping at `cap` bytes. Bytes past the cap
/// are discarded. This bounds memory for hostile / runaway child output.
fn drain_capped<R: Read>(mut reader: R, cap: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < cap {
                    let room = cap - buf.len();
                    let take = n.min(room);
                    buf.extend_from_slice(&chunk[..take]);
                }
                // else: drop excess bytes silently — the cap is the
                // contract; we deliberately avoid `read_to_end` here.
            }
            Err(_) => break,
        }
    }
    buf
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;
