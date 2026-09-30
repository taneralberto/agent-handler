//! Integration test for the D2 CLI seam.
//!
//! Spawns the `agenthd` binary as a subprocess with isolated
//! `HOME` and `XDG_CONFIG_HOME` directories. Verifies the
//! user-visible contract the D2 closing requires:
//!
//! - `agenthd gui` rejects with a clear non-zero exit and a
//!   stderr message that names the mode keyword. No filesystem
//!   side effects (`settings.json`, `state.json`, or the agenthd
//!   root must NOT be created).
//! - `agenthd gui --repo <abs-path>` rejects the same way. Even
//!   with a valid `--repo`, the GUI mode must error before any
//!   write — the rejection is on `mode`, not on the path.
//!
//! These are pure no-side-effect tests; the spawned process
//! cannot reach a terminal because the test harness does not
//! allocate one (the binary's `TerminalGuard::new` would fail,
//! but it never gets that far — the GUI check happens before
//! `Paths::from_env` or `TerminalGuard::new`).
//!
//! `cargo test --test cli_launch` runs these by default; no
//! `--ignored` flag is needed because the binary itself is
//! built locally and no network is involved.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Path to the `agenthd` binary Cargo built for this test crate.
fn binary_path() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_agenthd"))
}

/// Spawn the binary with `HOME` and `XDG_CONFIG_HOME` pointed at
/// isolated temp directories. Returns the exit status, the
/// captured stderr, the two isolated paths, AND the `TempDir`
/// that owns their lifetime.
///
/// The `TempDir` MUST stay alive for the duration of the
/// assertions the caller wants to make on the filesystem —
/// dropping it deletes the whole tree, which would make any
/// "this file does NOT exist" check vacuous. Holding it in the
/// returned tuple pins its lifetime to the caller's scope so
/// side-effect checks actually observe real on-disk state.
///
/// `stdout` and `stdin` are piped so the TUI never tries to
/// grab the test harness's controlling terminal.
fn run_isolated(args: &[&str]) -> (std::process::ExitStatus, String, PathBuf, PathBuf, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&xdg).expect("create xdg");
    // Sentinel file: a single, recognisable byte in `home` lets the
    // caller assert the tree contains exactly this one entry and
    // nothing else. If the GUI path leaked a write, `read_dir`
    // would surface it.
    std::fs::write(home.join("sentinel.txt"), b"sentinel").expect("write sentinel");

    let output = Command::new(binary_path())
        .args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn agenthd");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output.status, stderr, home, xdg, dir)
}

/// The GUI mode rejects before any filesystem side effects.
#[test]
fn agenthd_gui_rejects_with_clear_error_and_no_side_effects() {
    // `dir` MUST stay alive through the assertions below —
    // dropping it would delete the whole tree and turn the
    // "must NOT exist" checks into vacuous passes.
    let (status, stderr, home, xdg, _dir) = run_isolated(&["gui"]);

    assert!(
        !status.success(),
        "agenthd gui must exit non-zero, got: {status}"
    );
    // Exit code 2 distinguishes a CLI usage rejection (the GUI
    // seam is intentionally unimplemented) from a runtime
    // error. The exact code is part of the contract the boot
    // path documents.
    assert_eq!(
        status.code(),
        Some(2),
        "agenthd gui must exit with code 2 (CLI usage rejection), got: {status:?}"
    );
    assert!(
        stderr.contains("GUI mode is not implemented"),
        "stderr must clearly name the GUI seam, got: {stderr}"
    );

    // Strict no-side-effect check: enumerate HOME and XDG.
    // HOME was seeded with exactly one sentinel file; XDG is
    // an empty directory. If the GUI path leaked any write
    // (`Paths::from_env`, `ensure_dirs`, `save_settings`,
    // OpenCode / Pi target trees, etc.) either tree would
    // surface extra entries. This is the check that would
    // FAIL if side effects occurred — the previous
    // existence-only assertions were vacuous as long as the
    // tempdir lifetime ended before they ran.
    let home_entries = std::fs::read_dir(&home)
        .expect("home still readable while TempDir alive")
        .map(|e| e.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        home_entries,
        vec![std::ffi::OsString::from("sentinel.txt")],
        "HOME must contain only the sentinel seeded before the run; \
         any extra entry is a leaked side effect. got: {home_entries:?}"
    );

    let xdg_entries = std::fs::read_dir(&xdg)
        .expect("xdg still readable while TempDir alive")
        .map(|e| e.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        xdg_entries,
        Vec::<std::ffi::OsString>::new(),
        "XDG_CONFIG_HOME must remain empty; any entry is a leaked side effect. got: {xdg_entries:?}"
    );

    // Belt-and-braces: the previously-named paths must still
    // be absent. With the TempDir alive these would observe
    // real on-disk state.
    let agenthd_root = home.join(".agenthd");
    assert!(
        !agenthd_root.exists(),
        "agenthd root must NOT be created by `agenthd gui`, found: {}",
        agenthd_root.display()
    );
    assert!(
        !agenthd_root.join("settings.json").exists(),
        "settings.json must NOT be written by `agenthd gui`"
    );
    assert!(
        !agenthd_root.join("state.json").exists(),
        "state.json must NOT be written by `agenthd gui`"
    );
    assert!(
        !xdg.join("opencode").join("agents").exists(),
        "OpenCode target tree must NOT be created by `agenthd gui`"
    );
}

/// `agenthd gui --repo <abs-path>` rejects the same way. The
/// GUI mode is rejected on `mode`, not on the path; the override
/// must not turn a future-mode rejection into a write.
#[test]
fn agenthd_gui_with_repo_rejects_without_writing_settings() {
    // Use a real absolute path that would be a valid override
    // if the GUI mode were implemented. The path itself is
    // intentionally NOT created — the GUI check must run before
    // any validator that would touch it. Both `dir` handles
    // must outlive the assertions below; dropping either would
    // delete its tree and make the no-side-effect checks
    // vacuous.
    let checkout_dir = tempfile::tempdir().expect("tempdir");
    let would_be_checkout = checkout_dir.path().join("would-be-checkout");
    std::fs::create_dir_all(would_be_checkout.join("agents")).expect("create checkout agents");

    let would_be_checkout_str = would_be_checkout.to_string_lossy().into_owned();

    let (status, stderr, home, _xdg, _dir) =
        run_isolated(&["gui", "--repo", &would_be_checkout_str]);

    assert!(
        !status.success(),
        "agenthd gui --repo <abs> must exit non-zero, got: {status}"
    );
    assert_eq!(
        status.code(),
        Some(2),
        "agenthd gui --repo <abs> must exit with code 2, got: {status:?}"
    );
    assert!(
        stderr.contains("GUI mode is not implemented"),
        "stderr must clearly name the GUI seam, got: {stderr}"
    );

    // Same strict check: HOME must contain exactly the sentinel
    // we seeded — the override path must not have persisted
    // `settings.json` or anything else. `state.json` /
    // `Paths::ensure_dirs` / `save_settings` / `apply_checkout`
    // must NOT have run.
    let home_entries = std::fs::read_dir(&home)
        .expect("home still readable while TempDir alive")
        .map(|e| e.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        home_entries,
        vec![std::ffi::OsString::from("sentinel.txt")],
        "HOME must contain only the sentinel; `--repo` must not persist when GUI is rejected. got: {home_entries:?}"
    );

    let agenthd_root = home.join(".agenthd");
    assert!(
        !agenthd_root.exists(),
        "agenthd root must NOT be created when GUI mode is rejected with --repo, found: {}",
        agenthd_root.display()
    );
    assert!(
        !agenthd_root.join("settings.json").exists(),
        "settings.json must NOT be written by the rejected GUI path"
    );

    // And the source checkout itself must remain untouched
    // (the validator must not have been called). The agents/
    // subdir we created above stays exactly as we left it;
    // nothing was written into the checkout. `checkout_dir`
    // is held until end of scope so `read_dir` observes the
    // real on-disk tree.
    let agents_entries = std::fs::read_dir(would_be_checkout.join("agents"))
        .expect("checkout agents dir still readable")
        .map(|e| e.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        agents_entries,
        Vec::<std::ffi::OsString>::new(),
        "checkout agents/ must remain empty after the GUI rejection, got: {agents_entries:?}"
    );
}
