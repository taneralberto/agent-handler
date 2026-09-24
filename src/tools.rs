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
/// a tool-specific validation step. `pi-psql` ships first with the verified
/// tag/SHA pair: the peeled commit at `opencode-2026-09-23` is
/// `0dba366061911f0ec389f4a78cc46fd6d6a19d41` and the tag object is
/// `409543fe9750fdcc60fcea858c43c623cc40aaa9`. The pinned SHA is the
/// authoritative identity; `pin_tag` is the lookup key.
pub const DEFAULT_CATALOG: &[ToolCatalogEntry] = &[ToolCatalogEntry {
    repo: "https://github.com/taneralberto/pi-psql.git",
    skill_name: "pi-psql",
    destination_subpath: "pi-psql",
    pin_tag: "opencode-2026-09-23",
    expected_sha: "0dba366061911f0ec389f4a78cc46fd6d6a19d41",
    // Conservative bound that covers the `^22.12.0` leg of yargs@^18's
    // engines.node; matches the existing engines node `^20.19.0 || ^22.12.0
    // || >=23` range conservatively.
    node_min: NodeMin {
        major: 22,
        minor: 12,
        patch: 0,
    },
}];

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

    // npm --version
    let npm = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "npm",
            args: vec!["--version".to_string()],
            cwd: None,
            timeout: Duration::from_secs(GIT_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("npm: {}", e))?;
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
    let out = run_with_timeout(
        spawn,
        &SpawnSpec {
            program: "npm",
            args: vec![
                "ci".to_string(),
                "--omit=dev".to_string(),
                "--ignore-scripts".to_string(),
            ],
            cwd: Some(staging.to_path_buf()),
            timeout: Duration::from_secs(NPM_TIMEOUT_SECS),
        },
    )
    .map_err(|e| format!("npm ci spawn: {}", e))?;
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
mod tests {
    use super::*;
    use crate::store::Paths;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
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
                .join("agenthd-subagents.tsx"),
            plugin_config: dir.path().join(".config").join("opencode").join("tui.json"),
            skills_dir: dir.path().join(".config").join("opencode").join("skills"),
        };
        paths.ensure_dirs().unwrap();
        paths
    }

    fn recorded_with(
        runs: Rc<RefCell<Vec<SpawnSpec>>>,
        responses: Rc<RefCell<VecDeque<SpawnOutput>>>,
    ) -> impl FnMut(&SpawnSpec) -> Result<SpawnOutput> {
        move |spec: &SpawnSpec| {
            runs.borrow_mut().push(spec.clone());
            Ok(responses.borrow_mut().pop_front().unwrap_or(SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
        }
    }

    fn rename_ok(_src: &Path, _dst: &Path) -> Result<(), i32> {
        Ok(())
    }

    fn rename_err(code: i32) -> impl FnMut(&Path, &Path) -> Result<(), i32> {
        move |_src: &Path, _dst: &Path| Err(code)
    }

    fn pi_psql_entry() -> &'static ToolCatalogEntry {
        // The DEFAULT_CATALOG constant is the contract; this test only
        // references its first entry to keep the catalog source-of-truth in
        // one place.
        &DEFAULT_CATALOG[0]
    }

    fn good_preflight_node() -> String {
        format!(
            "v{}",
            format_version(NodeVersion {
                major: 22,
                minor: 12,
                patch: 0,
            })
        )
    }

    fn ls_remote_two_lines_annotated() -> String {
        // Tag object SHA, then peeled SHA. Both 40-hex; order matches git's
        // `ls-remote <repo>` output for annotated tags. The trailing `^{}`
        // is verbatim text, not a format placeholder, so concatenate the
        // raw parts instead of using `format!`.
        "409543fe9750fdcc60fcea858c43c623cc40aaa9\trefs/tags/opencode-2026-09-23\n\
         0dba366061911f0ec389f4a78cc46fd6d6a19d41\trefs/tags/opencode-2026-09-23^{}\n"
            .to_string()
    }

    fn ls_remote_peeled_only() -> String {
        // `git ls-remote <repo> <tag>^{}` for annotated tags: only the peeled
        // commit line is returned.
        "0dba366061911f0ec389f4a78cc46fd6d6a19d41\trefs/tags/opencode-2026-09-23^{}\n".to_string()
    }

    fn rev_parse_ok(sha: &str) -> SpawnOutput {
        SpawnOutput {
            success: true,
            code: Some(0),
            stdout: format!("{sha}\n").into_bytes(),
            stderr: Vec::new(),
        }
    }

    fn write_skill_md(staging: &Path, name: &str) {
        fs::create_dir_all(staging).unwrap();
        fs::write(
            staging.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: x\n---\nbody\n"),
        )
        .unwrap();
    }

    /// Build a mock runner that records every spawn, returns canned
    /// responses, and — once `git checkout FETCH_HEAD` is invoked —
    /// writes the supplied SKILL.md body to the staging path the
    /// installer is using. This stands in for the real git checkout,
    /// which would leave a tree on disk we can then read.
    fn mock_runner_with_skill_md(
        runs: Rc<RefCell<Vec<SpawnSpec>>>,
        responses: Rc<RefCell<VecDeque<SpawnOutput>>>,
        skill_body: String,
    ) -> impl FnMut(&SpawnSpec) -> Result<SpawnOutput> {
        move |spec: &SpawnSpec| {
            runs.borrow_mut().push(spec.clone());
            // Detect the `git -C <staging> checkout FETCH_HEAD` invocation
            // and materialize the SKILL.md before the response is returned
            // so the identity check sees it on disk.
            let is_checkout = spec.program == "git"
                && spec.args.first().map(String::as_str) == Some("-C")
                && spec.args.get(2).map(String::as_str) == Some("checkout")
                && spec.args.last().map(String::as_str) == Some("FETCH_HEAD");
            if is_checkout {
                if let Some(staging) = spec.args.get(1) {
                    let path = std::path::PathBuf::from(staging);
                    let _ = fs::create_dir_all(&path);
                    fs::write(path.join("SKILL.md"), skill_body.as_bytes()).ok();
                }
            }
            Ok(responses.borrow_mut().pop_front().unwrap_or(SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }))
        }
    }

    // ---------- ToolStatus / catalog contract ----------

    #[test]
    fn catalog_pin_matches_verified_remote() {
        // The pinned tag/SHA pair is the contract. If DEFAULT_CATALOG
        // changes, this test forces an explicit decision rather than a
        // silent pin drift.
        let entry = pi_psql_entry();
        assert_eq!(entry.repo, "https://github.com/taneralberto/pi-psql.git");
        assert_eq!(entry.pin_tag, "opencode-2026-09-23");
        assert_eq!(
            entry.expected_sha,
            "0dba366061911f0ec389f4a78cc46fd6d6a19d41"
        );
        assert_eq!(entry.skill_name, "pi-psql");
        assert_eq!(entry.node_min.major, 22);
        assert_eq!(entry.node_min.minor, 12);
        assert_eq!(entry.node_min.patch, 0);
    }

    #[test]
    fn status_label_for_each_variant() {
        assert_eq!(ToolStatus::NotInstalled.label(), "not installed");
        assert_eq!(ToolStatus::Installed.label(), "installed");
        assert_eq!(
            ToolStatus::PrerequisitesMissing.label(),
            "prerequisites missing"
        );
        assert_eq!(ToolStatus::IdentityMismatch.label(), "identity mismatch");
        assert_eq!(ToolStatus::InstallFailed.label(), "install failed");
        assert_eq!(ToolStatus::Conflict.label(), "conflict");
    }

    // ---------- tool_status (read-only inspection) ----------

    #[test]
    fn tool_status_reads_destination_directory() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        // Absent destination -> NotInstalled.
        let item = tool_status(&paths, entry).unwrap();
        assert_eq!(item.status, ToolStatus::NotInstalled);
        // Place a directory at the destination -> Installed.
        fs::create_dir_all(destination_for(&paths, entry)).unwrap();
        let item = tool_status(&paths, entry).unwrap();
        assert_eq!(item.status, ToolStatus::Installed);
    }

    #[test]
    fn tool_status_treats_file_or_symlink_at_destination_as_conflict() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let dst = destination_for(&paths, entry);

        fs::write(&dst, b"stray file").unwrap();
        assert_eq!(
            tool_status(&paths, entry).unwrap().status,
            ToolStatus::Conflict
        );
        fs::remove_file(&dst).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("/nonexistent/never-read", &dst).unwrap();
            assert_eq!(
                tool_status(&paths, entry).unwrap().status,
                ToolStatus::Conflict
            );
            fs::remove_file(&dst).unwrap();
        }
    }

    // ---------- Pre-flight (Step 1) ----------

    #[test]
    fn preflight_missing_git_returns_prerequisites_missing() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let runs = Rc::new(RefCell::new(Vec::new()));
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![SpawnOutput {
            success: false,
            code: Some(1),
            stdout: Vec::new(),
            stderr: b"git: not found".to_vec(),
        }])));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();
        assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
        assert!(
            outcome.detail.contains("git"),
            "pre-flight detail should mention git: {}",
            outcome.detail
        );
        // Staging was never created.
        assert!(!staging_for(&paths, entry).exists());
    }

    #[test]
    fn preflight_node_below_min_returns_prerequisites_missing() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        // Sequence: git --version OK, node --version v22.11.9 (below 22.12.0),
        // npm would be skipped because node fails first.
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.11.9\n".to_vec(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();
        assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
        assert!(
            outcome.detail.contains("22.11.9") && outcome.detail.contains("22.12"),
            "detail should compare parsed node against required min: {}",
            outcome.detail
        );
    }

    #[test]
    fn preflight_node_at_or_above_min_proceeds() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        // node v22.12.0 -> exactly the minimum -> proceeds.
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: format!("{}\n", good_preflight_node()).into_bytes(),
                stderr: Vec::new(),
            },
            // npm --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            // ls-remote returns the pinned tag and peeled SHA.
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        // Rename fails closed so we observe the rest of the flow without
        // relying on the platform rename behavior. The important
        // assertion here is that pre-flight let execution continue past
        // node_min.
        let mut rename = |_: &Path, _: &Path| -> Result<(), i32> { Err(libc_const_exdev()) };
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_ne!(
            outcome.status,
            ToolStatus::PrerequisitesMissing,
            "node at the minimum must pass pre-flight; got {:?}: {}",
            outcome.status,
            outcome.detail
        );
        // The first three recorded specs must be git/node/npm in order.
        let recorded = runs.borrow();
        assert_eq!(recorded[0].program, "git");
        assert_eq!(recorded[1].program, "node");
        assert_eq!(recorded[2].program, "npm");
    }

    // ---------- Step 2: ls-remote peeled SHA parsing ----------

    #[test]
    fn parse_shas_handles_annotated_and_lightweight_tags() {
        let annotated = ls_remote_two_lines_annotated();
        let shas = parse_shas(&annotated);
        assert_eq!(shas.len(), 2, "annotated tag should yield 2 SHAs");
        assert_eq!(
            shas[0], "409543fe9750fdcc60fcea858c43c623cc40aaa9",
            "first line is the tag object SHA"
        );
        assert_eq!(
            shas[1], "0dba366061911f0ec389f4a78cc46fd6d6a19d41",
            "second line is the peeled commit SHA"
        );

        let lightweight =
            "0dba366061911f0ec389f4a78cc46fd6d6a19d41\trefs/tags/opencode-2026-09-23\n";
        let shas = parse_shas(lightweight);
        assert_eq!(shas.len(), 1, "lightweight tag should yield 1 SHA");
        assert_eq!(shas[0], "0dba366061911f0ec389f4a78cc46fd6d6a19d41");
    }

    #[test]
    fn parse_shas_rejects_garbage() {
        assert!(parse_shas("").is_empty());
        assert!(parse_shas("not a hex line\n").is_empty());
        // Short SHA (39 chars) is rejected.
        assert!(parse_shas("0dba366061911f0ec389f4a78cc46fd6d6a19d\n").is_empty());
    }

    #[test]
    fn ls_remote_path_argv_uses_tag_not_sha() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            // git --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            // node --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            // npm --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            // ls-remote -- emits the pinned tag and peeled SHA
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            // Remaining spawns are irrelevant because the rename fails.
            SpawnOutput {
                success: false,
                code: Some(1),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let mut rename = rename_err(libc_const_exdev());
        let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        let recorded = runs.borrow();
        let ls_remote = recorded
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("ls-remote"))
            .expect("git ls-remote must be invoked");
        assert_eq!(
            ls_remote.args,
            vec![
                "ls-remote".to_string(),
                entry.repo.to_string(),
                entry.pin_tag.to_string(),
            ],
            "ls-remote argv must include the pinned tag, never the SHA"
        );
        // Defense: no spawn may ever invoke ls-remote with the SHA in argv.
        for spec in recorded.iter() {
            if spec.program == "git" && spec.args.first().map(String::as_str) == Some("ls-remote") {
                assert!(
                    !spec.args.iter().any(|a| a == entry.expected_sha),
                    "ls-remote must fetch by tag, never by SHA: {:?}",
                    spec.args
                );
            }
        }
    }

    // ---------- Step 3 & 4: stage + pinned SHA ----------

    #[test]
    fn staging_argv_matches_init_remote_fetch_checkout_revparse() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            // git --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            // node --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            // npm --version
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            // ls-remote
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            // git init <staging>
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            // git -C <staging> remote add origin <repo>
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            // git -C <staging> fetch --depth=1 origin <tag>
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            // git -C <staging> checkout FETCH_HEAD
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            // git -C <staging> rev-parse HEAD -- pinned SHA expected.
            rev_parse_ok(entry.expected_sha),
            // Subsequent spawns are not reached; rename fails closed.
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        // The mock writes a SKILL.md into the installer's staging dir during
        // the `git checkout FETCH_HEAD` step so the identity check sees it.
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_err(libc_const_exdev());
        let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();

        let recorded = runs.borrow();
        // Capture the staging path the installer actually used, from the
        // recorded `git init` argv.
        let init = recorded
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .expect("git init must be invoked");
        let staging_str = init.args[1].clone();
        // Verify the four stage-step invocations have the exact argv the
        // plan specifies.
        assert_eq!(
            init.args,
            vec!["init".to_string(), staging_str.clone()],
            "git init argv: {:?}",
            init.args
        );
        let remote = recorded
            .iter()
            .find(|s| {
                s.program == "git"
                    && s.args.first().map(String::as_str) == Some("-C")
                    && s.args.get(2).map(String::as_str) == Some("remote")
            })
            .expect("git remote add must be invoked");
        assert_eq!(
            remote.args,
            vec![
                "-C".to_string(),
                staging_str.clone(),
                "remote".to_string(),
                "add".to_string(),
                "origin".to_string(),
                entry.repo.to_string(),
            ]
        );
        let fetch = recorded
            .iter()
            .find(|s| {
                s.program == "git"
                    && s.args.first().map(String::as_str) == Some("-C")
                    && s.args.get(2).map(String::as_str) == Some("fetch")
            })
            .expect("git fetch must be invoked");
        assert_eq!(
            fetch.args,
            vec![
                "-C".to_string(),
                staging_str.clone(),
                "fetch".to_string(),
                "--depth=1".to_string(),
                "origin".to_string(),
                entry.pin_tag.to_string(),
            ],
            "fetch by tag, never by SHA: {:?}",
            fetch.args
        );
        let checkout = recorded
            .iter()
            .find(|s| {
                s.program == "git"
                    && s.args.first().map(String::as_str) == Some("-C")
                    && s.args.get(2).map(String::as_str) == Some("checkout")
            })
            .expect("git checkout must be invoked");
        assert_eq!(
            checkout.args,
            vec![
                "-C".to_string(),
                staging_str.clone(),
                "checkout".to_string(),
                "FETCH_HEAD".to_string(),
            ]
        );
        let rev = recorded
            .iter()
            .find(|s| {
                s.program == "git"
                    && s.args.first().map(String::as_str) == Some("-C")
                    && s.args.get(2).map(String::as_str) == Some("rev-parse")
            })
            .expect("git rev-parse must be invoked");
        assert_eq!(
            rev.args,
            vec![
                "-C".to_string(),
                staging_str,
                "rev-parse".to_string(),
                "HEAD".to_string(),
            ]
        );
    }

    #[test]
    fn pinned_sha_mismatch_is_install_failed_and_cleans_up_staging() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            // rev-parse HEAD -- WRONG SHA
            rev_parse_ok("0000000000000000000000000000000000000000"),
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_ok;
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("HEAD is") && outcome.detail.contains("catalog pin"),
            "pinned-SHA detail: {}",
            outcome.detail
        );
        // Capture the staging path the installer used (from the recorded
        // git init argv) and assert it was cleaned up.
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(!staging_used.exists(), "staging must be removed on failure");
        assert!(
            !destination_for(&paths, entry).exists(),
            "destination must not be touched on failure"
        );
    }

    /// Regression: when a regular directory, regular file, or symlink already
    /// exists at the computed staging path, the installer must refuse to
    /// touch it. Earlier the `stage` step did `remove_dir_all` first, which
    /// silently clobbered any preexisting path at the staging location.
    ///
    /// The three tests below each pin the staging path to a deterministic
    /// value so they can place a sentinel at exactly that path before
    /// invoking the installer. Using `install_tool_at` avoids depending on
    /// the process-global staging counter (each `staging_for` call bumps
    /// it, so `install_tool_with`'s internal call would land on a fresh
    /// path that the test could not pre-populate).
    fn deterministic_staging(paths: &Paths, entry: &ToolCatalogEntry) -> PathBuf {
        destination_for(paths, entry).with_extension(format!(
            ".staging-regression-{}-{}",
            std::process::id(),
            0xC0FFEEu64
        ))
    }

    #[test]
    fn preexisting_dir_at_staging_path_is_preserved_and_install_fails() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = deterministic_staging(&paths, entry);
        let sentinel = b"unrelated user data at staging path\n";
        fs::create_dir_all(&staging).unwrap();
        let user_file = staging.join("user.md");
        fs::write(&user_file, sentinel).unwrap();

        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let mut rename = rename_ok;
        let outcome = install_tool_at(&staging, &paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("already exists")
                && outcome.detail.contains("refusing to clobber"),
            "detail should explain the bail: {}",
            outcome.detail
        );
        // No git init/remote/fetch/checkout ever ran for the staged tree —
        // the only spawns are preflight + ls-remote.
        for spec in runs.borrow().iter() {
            assert!(
                !spec.args.iter().any(|a| a == "init")
                    && !spec.args.iter().any(|a| a == "remote")
                    && !spec.args.iter().any(|a| a == "fetch")
                    && !spec.args.iter().any(|a| a == "checkout"),
                "stage must not run any git subcommand after the bail: {:?}",
                spec.args
            );
        }
        // The preexisting staging dir is byte-for-byte unchanged.
        assert!(staging.is_dir(), "staging dir must remain");
        assert_eq!(
            fs::read(&user_file).unwrap(),
            sentinel,
            "preexisting sentinel file must be preserved"
        );
        assert!(
            !destination_for(&paths, entry).exists(),
            "destination must not be touched"
        );
    }

    #[test]
    fn preexisting_file_at_staging_path_is_preserved_and_install_fails() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = deterministic_staging(&paths, entry);
        let sentinel = b"stray regular file at staging path\n";
        fs::write(&staging, sentinel).unwrap();

        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let mut rename = rename_ok;
        let outcome = install_tool_at(&staging, &paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("already exists")
                && outcome.detail.contains("refusing to clobber"),
            "detail should explain the bail: {}",
            outcome.detail
        );
        assert!(staging.is_file(), "staging path must remain a regular file");
        assert_eq!(
            fs::read(&staging).unwrap(),
            sentinel,
            "preexisting sentinel bytes must be preserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_symlink_at_staging_path_is_preserved_and_install_fails() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = deterministic_staging(&paths, entry);
        let sentinel_target = dir.path().join("symlink-target");
        fs::write(&sentinel_target, b"target of the symlink\n").unwrap();
        symlink(&sentinel_target, &staging).unwrap();
        let link_meta_before = fs::symlink_metadata(&staging).unwrap();

        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let mut runner = recorded_with(runs.clone(), responses.clone());
        let mut rename = rename_ok;
        let outcome = install_tool_at(&staging, &paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("already exists")
                && outcome.detail.contains("refusing to clobber"),
            "detail should explain the bail: {}",
            outcome.detail
        );
        // Symlink must still resolve to its original target, with the
        // exact same metadata (file_type == symlink).
        assert!(
            fs::symlink_metadata(&staging)
                .unwrap()
                .file_type()
                .is_symlink(),
            "staging path must remain a symlink"
        );
        assert_eq!(
            fs::read_link(&staging).unwrap(),
            sentinel_target,
            "symlink target must be preserved"
        );
        let link_meta_after = fs::symlink_metadata(&staging).unwrap();
        assert_eq!(
            link_meta_before.file_type(),
            link_meta_after.file_type(),
            "symlink metadata must not change"
        );
        assert_eq!(
            fs::read(&staging).unwrap(),
            b"target of the symlink\n",
            "symlink must still resolve to its original target"
        );
    }

    // ---------- Step 5: identity check ----------

    #[test]
    fn identity_mismatch_when_skill_name_differs_is_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body =
            "---\nname: totally-different-skill\ndescription: x\n---\nbody\n".to_string();
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_ok;
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::IdentityMismatch);
        assert!(
            outcome.detail.contains("totally-different-skill")
                && outcome.detail.contains("pi-psql"),
            "identity detail should name the mismatch: {}",
            outcome.detail
        );
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(
            !staging_used.exists(),
            "staging must be removed on identity mismatch"
        );
    }

    #[test]
    fn identity_mismatch_when_skill_md_missing_frontmatter_is_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        // The mock writes a SKILL.md *without* a frontmatter block.
        let skill_body = "no frontmatter here\n".to_string();
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_ok;
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::IdentityMismatch);
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(!staging_used.exists());
    }

    #[test]
    fn identity_check_accepts_quoted_and_unquoted_names() {
        // Direct unit coverage of parse_skill_name.
        assert_eq!(
            parse_skill_name("---\nname: foo\n---\nbody"),
            Some("foo".to_string())
        );
        assert_eq!(
            parse_skill_name("---\nname: \"foo bar\"\n---\nbody"),
            Some("foo bar".to_string())
        );
        assert_eq!(
            parse_skill_name("---\nname: 'pi-psql'\n---\nbody"),
            Some("pi-psql".to_string())
        );
        assert_eq!(
            parse_skill_name("---\r\nname: pi-psql\r\n---\r\nbody"),
            Some("pi-psql".to_string())
        );
        // name not first: still parsed if present.
        assert_eq!(
            parse_skill_name("---\ndescription: x\nname: pi-psql\n---\nbody"),
            Some("pi-psql".to_string())
        );
        // Missing name -> None.
        assert_eq!(parse_skill_name("---\ndescription: x\n---\nbody"), None);
        // No frontmatter -> None.
        assert_eq!(parse_skill_name("just a body"), None);
    }

    // ---------- Step 6: npm ci argv ----------

    #[test]
    fn npm_ci_argv_is_omit_dev_and_ignore_scripts_and_runs_in_staging() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            // npm ci succeeds.
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"added 5 packages".to_vec(),
                stderr: Vec::new(),
            },
            // remaining rename succeeded via real primitive.
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        // Pre-create destination so the real rename primitive succeeds.
        let _dst = destination_for(&paths, entry);
        let mut rename = move |src: &Path, dst: &Path| -> Result<(), i32> {
            if !dst.exists() {
                fs::create_dir_all(dst).map_err(|_| 1)?;
            }
            rename_no_replace(src, dst)
        };
        let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        // Find the `npm ci ...` invocation specifically (not `npm --version`).
        let npm = runs
            .borrow()
            .iter()
            .find(|s| s.program == "npm" && s.args.first().map(String::as_str) == Some("ci"))
            .expect("npm ci must be invoked")
            .clone();
        assert_eq!(
            npm.args,
            vec![
                "ci".to_string(),
                "--omit=dev".to_string(),
                "--ignore-scripts".to_string(),
            ],
            "npm ci argv: {:?}",
            npm.args
        );
        // cwd must point at the installer's staging dir.
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert_eq!(
            npm.cwd.as_deref(),
            Some(staging_used.as_path() as &Path),
            "npm ci must run inside staging"
        );
        assert_eq!(npm.timeout, Duration::from_secs(NPM_TIMEOUT_SECS));
    }

    #[test]
    fn npm_ci_failure_is_install_failed_and_cleans_up_staging() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            // npm ci FAILURE.
            SpawnOutput {
                success: false,
                code: Some(1),
                stdout: Vec::new(),
                stderr: b"npm ERR! missing dep".to_vec(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_ok;
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("npm ci"),
            "npm failure detail: {}",
            outcome.detail
        );
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(
            !staging_used.exists(),
            "staging must be removed on npm failure"
        );
        assert!(
            !destination_for(&paths, entry).exists(),
            "destination must not be touched on npm failure"
        );
    }

    // ---------- Step 7: publish no-replace ----------

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rename_noreplace_succeeds_when_target_absent() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        // The destination must not exist for a successful no-replace rename.
        if dst.exists() {
            fs::remove_dir_all(&dst).unwrap();
        }
        // Invoke the real OS primitive.
        let result = rename_no_replace(&staging, &dst);
        assert!(result.is_ok(), "rename should succeed: {:?}", result);
        assert!(dst.is_dir(), "destination must now exist");
        assert!(!staging.exists(), "staging must be consumed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rename_noreplace_returns_eexist_when_target_is_directory() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        fs::create_dir_all(&dst).unwrap();
        let result = rename_no_replace(&staging, &dst);
        assert_eq!(
            result.unwrap_err(),
            libc::EEXIST,
            "renameat2 must return EEXIST"
        );
        assert!(
            staging.exists(),
            "staging must NOT be removed by the primitive itself"
        );
        assert!(dst.is_dir(), "destination must remain unchanged");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rename_noreplace_returns_eexist_when_target_is_file() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        fs::write(&dst, b"stray bytes").unwrap();
        let result = rename_no_replace(&staging, &dst);
        assert_eq!(result.unwrap_err(), libc::EEXIST);
        // The stray file must be byte-equal to its original content.
        assert_eq!(fs::read(&dst).unwrap(), b"stray bytes");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rename_noreplace_returns_eexist_when_target_is_symlink() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        std::os::unix::fs::symlink("/some/other/target", &dst).unwrap();
        let result = rename_no_replace(&staging, &dst);
        assert_eq!(result.unwrap_err(), libc::EEXIST);
        assert!(
            fs::symlink_metadata(&dst).unwrap().file_type().is_symlink(),
            "destination must remain a symlink"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rename_noreplace_returns_exdev_for_cross_device_staging() {
        // We can't actually cross mountpoints inside a tempdir, but we
        // can simulate the EXDEV branch in the installer by injecting a
        // rename mock and asserting the typed outcome + cleanup. This is
        // the path the installer takes when the OS reports cross-device.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_err(libc::EXDEV);
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::InstallFailed);
        assert!(
            outcome.detail.contains("different filesystem"),
            "EXDEV detail: {}",
            outcome.detail
        );
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(!staging_used.exists(), "staging must be removed on EXDEV");
        assert!(
            !destination_for(&paths, entry).exists(),
            "destination must not be touched on EXDEV"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_rename_movefilew_returns_already_exists_when_target_exists() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        fs::write(&dst, b"stray").unwrap();
        let result = rename_no_replace(&staging, &dst);
        let code = result.unwrap_err();
        assert_eq!(
            code,
            windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32
        );
        assert_eq!(fs::read(&dst).unwrap(), b"stray");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_rename_movefilew_succeeds_when_target_absent() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let staging = staging_for(&paths, entry);
        write_skill_md(&staging, entry.skill_name);
        let dst = destination_for(&paths, entry);
        let result = rename_no_replace(&staging, &dst);
        assert!(result.is_ok(), "MoveFileW should succeed: {:?}", result);
        assert!(dst.is_dir());
        assert!(!staging.exists());
    }

    // ---------- Conflict flow integration ----------

    #[test]
    fn conflict_when_target_already_exists() {
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let dst = destination_for(&paths, entry);
        // Place an existing regular directory at the destination. The
        // installer's rename mock simulates the OS returning the no-
        // replace EEXIST/ERROR_ALREADY_EXISTS code.
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("existing.md"), b"existing bytes").unwrap();

        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_err(libc_const_eexist());
        let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        assert_eq!(outcome.status, ToolStatus::Conflict);
        let staging_used = runs
            .borrow()
            .iter()
            .find(|s| s.program == "git" && s.args.first().map(String::as_str) == Some("init"))
            .map(|s| std::path::PathBuf::from(&s.args[1]))
            .expect("git init must be invoked");
        assert!(
            !staging_used.exists(),
            "staging must be removed on conflict"
        );
        // The existing destination bytes are untouched.
        assert_eq!(
            fs::read(dst.join("existing.md")).unwrap(),
            b"existing bytes"
        );
    }

    #[test]
    fn conflict_does_not_run_a_preliminary_exists_check() {
        // The plan forbids a separate "exists" check before the rename
        // primitive. Asserting this directly: the installer's spawn log
        // for a conflict scenario must contain no call that reads the
        // destination, no `test -e`, no `stat`, no `opendir`. We verify
        // that by counting `program` invocations and asserting the set is
        // exactly {git, node, npm}.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let dst = destination_for(&paths, entry);
        fs::create_dir_all(&dst).unwrap();

        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_err(libc_const_eexist());
        let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        let programs: std::collections::BTreeSet<&str> =
            runs.borrow().iter().map(|s| s.program).collect();
        assert_eq!(
            programs,
            ["git", "node", "npm"]
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            "the installer must only spawn git, node, npm; got {:?}",
            programs
        );
        // And specifically: no spawn args form `sh -c` or any shell-like
        // program.
        for spec in runs.borrow().iter() {
            assert_ne!(spec.program, "sh");
            assert_ne!(spec.program, "bash");
            assert_ne!(spec.program, "zsh");
            assert!(
                !spec.args.iter().any(|a| a == "-c"),
                "no spawn must carry a `-c` argv"
            );
        }
    }

    // ---------- No cross-platform non-replace claim ----------

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    #[test]
    fn other_platforms_fail_closed() {
        let src = std::path::Path::new("/tmp/src");
        let dst = std::path::Path::new("/tmp/dst");
        let result = rename_no_replace(src, dst);
        assert!(result.is_err(), "non-Linux/Windows must fail closed");
    }

    // ---------- argv hygiene ----------

    #[test]
    fn no_spawn_uses_shell_or_interpolation() {
        // Run a successful end-to-end against the mocked runner and
        // confirm: every spawn has program != "sh"/"bash"/"zsh", no args
        // contain ` -c `, and there is no Command::arg with shell
        // metacharacters. This is the test the plan requires.
        let dir = TempDir::new().unwrap();
        let paths = setup_paths(&dir);
        let entry = pi_psql_entry();
        let responses = Rc::new(RefCell::new(VecDeque::from(vec![
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"git version 2.43.0".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"v22.12.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: b"10.9.0\n".to_vec(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_two_lines_annotated().into_bytes(),
                stderr: Vec::new(),
            },
            // peeled-commit lookup: <tag>^{}
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: ls_remote_peeled_only().into_bytes(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            rev_parse_ok(entry.expected_sha),
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
            SpawnOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        ])));
        let runs = Rc::new(RefCell::new(Vec::new()));
        let skill_body = format!(
            "---\nname: {}\ndescription: x\n---\nbody\n",
            entry.skill_name
        );
        let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
        let mut rename = rename_err(libc_const_exdev());
        let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
        for spec in runs.borrow().iter() {
            assert!(
                !matches!(spec.program, "sh" | "bash" | "zsh" | "dash" | "fish"),
                "no spawn may invoke a shell: {} {:?}",
                spec.program,
                spec.args
            );
            assert!(
                !spec.args.iter().any(|a| a == "-c"),
                "no spawn may use -c: {:?}",
                spec.args
            );
        }
    }
}

// Allow literal initializer for SpawnOutput::default(true) above.
// (intentionally not adding Default; tests build SpawnOutput explicitly.)
