//! Integration test for the `agenthd gui` boot seam.
//!
//! Spawns the `agenthd` binary from an **isolated copy in a
//! `TempDir`**. The copy lives next to a fake companion binary
//! (when the test wants a companion-present run) or next to no
//! companion at all (when the test wants the missing-companion
//! branch). This means the test suite does NOT depend on the
//! contents of the user's `target/` tree: if the user installs
//! or copies a real `agenthd-gui(.exe)` next to the built
//! `agenthd(.exe)`, the missing-companion tests still take the
//! missing-companion path (because the copy under test runs
//! from an empty TempDir) and the companion-present tests
//! still use the fake companion (because the test does not
//! look at the user's `target/` tree at all).
//!
//! Verified contracts:
//!
//! - `agenthd gui` with no companion adjacent: exits with code
//!   `2` and a stderr message naming the missing companion
//!   file. No filesystem side effects (`settings.json`,
//!   `state.json`, the agenthd root, the OpenCode / Pi target
//!   trees, or the source checkout must NOT be created or
//!   written). The companion locator runs before
//!   `resolve_checkout_path`, so the error surfaces before any
//!   `--repo` write can happen.
//! - `agenthd gui --repo <abs-path>` with no companion: same
//!   missing-companion rejection. The companion check is
//!   independent of `--repo`: a valid override cannot turn a
//!   missing-companion rejection into a `settings.json` write.
//! - `agenthd gui --repo <abs-path>` with the fake companion
//!   adjacent: the companion is spawned with the same argv,
//!   `resolve_checkout_path` persists the override to
//!   `settings.json`, the agenthd root / OpenCode / Pi target
//!   trees are **not** created, and the checkout itself is
//!   **not** modified. On Unix the fake script exits `0`, so
//!   the forwarded exit code is `0`. On Windows the fake is a
//!   copy of `cmd.exe` invoked with the GUI argv and a piped
//!   stdin — cmd.exe has nothing to run with those args and
//!   that stdin, so it exits `0`; the boot path forwards that
//!   exit code, and we still verify `settings.json` was
//!   written with the persisted value.
//! - `agenthd gui --repo <abs-path>` (override equals the
//!   already-persisted value): `settings.json` is **not**
//!   rewritten (the save-if-changed branch in
//!   `resolve_checkout_path` keeps its bytes).
//! - `agenthd gui --repo <abs-missing-path>`: the
//!   `validate_checkout_path` failure surfaces as exit code
//!   `1` and a stderr message that names the missing path.
//!   `settings.json` is **not** written.
//!
//! `stdout` / `stdin` are piped so neither the TUI nor a
//! companion GUI ever tries to grab the test harness's
//! controlling terminal. `cargo test --test cli_launch` runs
//! these by default; no `--ignored` flag is needed because the
//! binary itself is built locally and no network is involved.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Path to the `agenthd` binary Cargo built for this test crate.
fn cargo_built_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_agenthd"))
}

/// Companion filename on this host (`agenthd-gui.exe` on Windows,
/// `agenthd-gui` elsewhere).
fn companion_filename() -> &'static str {
    if cfg!(windows) {
        "agenthd-gui.exe"
    } else {
        "agenthd-gui"
    }
}

/// Copy the built `agenthd(.exe)` into an isolated `TempDir` and
/// return `(tempdir, copied_binary_path)`. The companion
/// lookup inspects the directory of the current executable, so
/// running the copy from this `TempDir` guarantees no real
/// companion is found unless the test installs one explicitly.
/// The `TempDir` must be held alive for the duration of the
/// assertions so filesystem state is observable.
fn isolated_binary() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = cargo_built_binary();
    let dst = dir
        .path()
        .join(src.file_name().expect("built binary filename"));
    std::fs::copy(src, &dst).expect("copy built agenthd into isolated TempDir");
    (dir, dst)
}

/// Drop a fake companion binary next to `bin`. On Unix this is
/// a shebang shell script with `exit 0` (the args the GUI
/// branch passes are ignored by the script); on Windows it is
/// a copy of `cmd.exe` (looked up via `ComSpec`, falling back
/// to the canonical path on minimal hosts) renamed to the
/// companion filename. The fake lives inside the test's
/// `TempDir`; nothing is written to the user's `target/` tree.
#[allow(unused_variables)]
fn land_fake_companion(bin_dir: &Path) -> PathBuf {
    let companion = bin_dir.join(companion_filename());

    #[cfg(unix)]
    {
        std::fs::write(&companion, "#!/bin/sh\nexit 0\n").expect("write fake companion script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&companion, std::fs::Permissions::from_mode(0o755))
            .expect("set exec bit on fake companion");
    }
    #[cfg(windows)]
    {
        let comspec = std::env::var("ComSpec")
            .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
        std::fs::copy(&comspec, &companion).expect("copy cmd.exe as fake companion");
    }
    companion
}

/// Run the isolated `agenthd` copy with `HOME` and
/// `XDG_CONFIG_HOME` pointed at isolated temp directories.
/// Returns the exit status, captured stderr, the two isolated
/// paths, and the `TempDir` that owns the binary copy.
///
/// The `TempDir`s MUST stay alive through the assertions —
/// dropping them deletes the trees and turns the "must NOT
/// exist" checks into vacuous passes. Holding them in the
/// returned tuple pins their lifetime to the caller's scope.
fn run_isolated(
    bin: &Path,
    args: &[&str],
) -> (std::process::ExitStatus, String, PathBuf, PathBuf, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&xdg).expect("create xdg");
    // Sentinel: HOME contains exactly this one entry; if the
    // GUI path leaked a write, `read_dir` would surface it.
    std::fs::write(home.join("sentinel.txt"), b"sentinel").expect("write sentinel");

    let output = Command::new(bin)
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

/// List `home`'s direct entries (filenames) so the caller can
/// assert the GUI path did not leak any extra files beyond the
/// sentinel it seeded.
fn home_entries(home: &Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(home)
        .expect("home still readable while TempDir alive")
        .map(|e| e.expect("entry").file_name())
        .collect()
}

/// The GUI mode rejects with a clear error **before** any
/// filesystem side effect. The isolated copy runs from an
/// empty `TempDir` (no companion adjacent), so the boot path
/// takes the missing-companion branch and exits 2 without
/// touching the filesystem.
#[test]
fn agenthd_gui_rejects_with_clear_error_and_no_side_effects() {
    let (bin_dir, bin) = isolated_binary();
    let (status, stderr, home, xdg, _env_dir) = run_isolated(&bin, &["gui"]);

    assert!(
        !status.success(),
        "agenthd gui must exit non-zero, got: {status}"
    );
    // Exit code 2 distinguishes the missing-companion CLI
    // usage rejection from a runtime error (exit 1). The
    // exact code is part of the contract documented in the
    // GUI roadmap; tests in other layers pin it.
    assert_eq!(
        status.code(),
        Some(2),
        "agenthd gui must exit with code 2 (missing-companion CLI usage rejection), got: {status:?}"
    );
    // The error must name the missing artefact and point at
    // the paired-install contract so the failure is
    // actionable.
    assert!(
        stderr.contains("companion binary `agenthd-gui"),
        "stderr must clearly name the missing companion binary, got: {stderr}"
    );
    assert!(
        stderr.contains("paired-install"),
        "stderr must point at the paired-install contract, got: {stderr}"
    );

    // Strict no-side-effect check: HOME contains exactly the
    // sentinel we seeded; XDG is empty; the agenthd root and
    // its settings / state files were never created; the
    // OpenCode / Pi target trees were never created.
    assert_eq!(
        home_entries(&home),
        vec![std::ffi::OsString::from("sentinel.txt")],
        "HOME must contain only the sentinel seeded before the run; \
         any extra entry is a leaked side effect"
    );
    let xdg_entries: Vec<std::ffi::OsString> = std::fs::read_dir(&xdg)
        .expect("xdg still readable")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert!(
        xdg_entries.is_empty(),
        "XDG_CONFIG_HOME must remain empty; any entry is a leaked side effect. got: {xdg_entries:?}"
    );

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

    drop(bin_dir);
}

/// `agenthd gui --repo <abs-path>` rejects the same way. The
/// companion check runs **before** `resolve_checkout_path`, so
/// even a syntactically valid `--repo` does not turn the
/// missing-companion rejection into a `settings.json` write.
/// The override path itself stays untouched (the validator
/// must not have been called).
#[test]
fn agenthd_gui_with_repo_rejects_without_writing_settings() {
    let checkout_dir = tempfile::tempdir().expect("checkout tempdir");
    let would_be_checkout = checkout_dir.path().join("would-be-checkout");
    std::fs::create_dir_all(would_be_checkout.join("agents"))
        .expect("create checkout agents subdir");
    let would_be_checkout_str = would_be_checkout.to_string_lossy().into_owned();

    let (bin_dir, bin) = isolated_binary();
    let (status, stderr, home, _xdg, _env_dir) =
        run_isolated(&bin, &["gui", "--repo", &would_be_checkout_str]);

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
        stderr.contains("companion binary `agenthd-gui"),
        "stderr must clearly name the missing companion binary, got: {stderr}"
    );

    // HOME contains only the sentinel; no settings.json.
    assert_eq!(
        home_entries(&home),
        vec![std::ffi::OsString::from("sentinel.txt")],
        "HOME must contain only the sentinel; `--repo` must not persist when the companion is missing"
    );
    let agenthd_root = home.join(".agenthd");
    assert!(
        !agenthd_root.exists(),
        "agenthd root must NOT be created when GUI mode is rejected with --repo, found: {}",
        agenthd_root.display()
    );
    assert!(
        !agenthd_root.join("settings.json").exists(),
        "settings.json must NOT be written when the GUI companion is missing"
    );

    // The would-be checkout must remain untouched (validator
    // not called): agents/ stays empty.
    let agents_entries: Vec<std::ffi::OsString> =
        std::fs::read_dir(would_be_checkout.join("agents"))
            .expect("checkout agents dir still readable")
            .map(|e| e.expect("entry").file_name())
            .collect();
    assert_eq!(
        agents_entries,
        Vec::<std::ffi::OsString>::new(),
        "checkout agents/ must remain empty after the GUI rejection, got: {agents_entries:?}"
    );

    drop(bin_dir);
    drop(checkout_dir);
}

/// Companion PRESENT + `--repo <isolated-checkout>`: the
/// boot path locates the fake companion, runs
/// `resolve_checkout_path` (which persists the override to
/// `settings.json`), spawns the companion, and forwards its
/// exit code. The agenthd root is created by `save_settings`
/// (it creates the parent directory), but `state.json` /
/// OpenCode / Pi target trees are NOT created (run_gui does
/// not call `ensure_dirs`). The checkout itself is NOT
/// modified. The fake companion exits `0` on both
/// platforms: on Unix the script ignores its argv; on
/// Windows, `cmd.exe` invoked with arbitrary args and a
/// piped stdin has nothing to run and exits `0`.
#[test]
fn agenthd_gui_with_companion_present_persists_repo_without_target_writes() {
    let checkout_dir = tempfile::tempdir().expect("checkout tempdir");
    let checkout = checkout_dir.path().join("checkout");
    std::fs::create_dir_all(checkout.join("agents")).expect("create checkout agents");
    let checkout_str = checkout.to_string_lossy().into_owned();

    let (bin_dir, bin) = isolated_binary();
    land_fake_companion(bin_dir.path());
    let (status, stderr, home, xdg, _env_dir) =
        run_isolated(&bin, &["gui", "--repo", &checkout_str]);

    // The fake companion exits 0 on both platforms: on Unix
    // the script runs `exit 0` (ignoring its argv), on
    // Windows cmd.exe invoked with arbitrary args and a
    // piped stdin has nothing to do and exits 0. The CLI
    // must forward that exit code, proving the spawn
    // happened and the boot path got past
    // `locate_companion`.
    assert_eq!(
        status.code(),
        Some(0),
        "fake companion exits 0; CLI must forward that exit code, got: {status:?}, stderr: {stderr}"
    );

    // settings.json was written and contains the persisted
    // checkout path. agenthd root was created by save_settings
    // (it creates the parent directory); state.json /
    // OpenCode / Pi target trees were NOT created (run_gui
    // does not call ensure_dirs). The path is stored
    // JSON-escaped (e.g. `\\` on Windows), so we look for
    // the parent tempdir basename as a JSON-escape-free
    // marker instead of the raw path.
    let settings_path = home.join(".agenthd").join("settings.json");
    let settings_bytes = std::fs::read(&settings_path).expect("settings.json must be written");
    let settings_text = String::from_utf8_lossy(&settings_bytes);
    let marker = checkout
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .expect("checkout tempdir has a basename");
    assert!(
        settings_text.contains(&marker),
        "settings.json must contain the persisted --repo path's tempdir marker `{marker}`, got: {settings_text}"
    );
    assert!(
        !home.join(".agenthd").join("state.json").exists(),
        "state.json must NOT be written by the GUI branch"
    );
    assert!(
        !xdg.join("opencode").exists(),
        "OpenCode target tree must NOT be created by the GUI branch, found: {}",
        xdg.join("opencode").display()
    );
    assert!(
        !home.join(".pi").exists(),
        "Pi target tree must NOT be created by the GUI branch, found: {}",
        home.join(".pi").display()
    );

    // The checkout itself must not be modified: agents/ stays
    // exactly as we left it (empty).
    let checkout_agents_entries: Vec<std::ffi::OsString> =
        std::fs::read_dir(checkout.join("agents"))
            .expect("checkout agents dir still readable")
            .map(|e| e.expect("entry").file_name())
            .collect();
    assert_eq!(
        checkout_agents_entries,
        Vec::<std::ffi::OsString>::new(),
        "checkout agents/ must remain empty after the GUI run, got: {checkout_agents_entries:?}"
    );

    drop(bin_dir);
    drop(checkout_dir);
}

/// Companion PRESENT + `--repo` equals the already-persisted
/// value: `settings.json` is **not** rewritten. The
/// save-if-changed branch in `resolve_checkout_path` keeps its
/// bytes (we seed the file with compact JSON bytes, which
/// differ from the pretty-printed bytes `save_settings` would
/// write; a rewrite would replace the file with the pretty
/// form and trip the assertion).
///
/// Runs on both Unix and Windows: the fake companion exits `0`
/// in both cases (the script ignores its argv on Unix;
/// `cmd.exe` invoked with arbitrary args and a piped stdin
/// has nothing to run and exits `0` on Windows), so the
/// `cargo install`-style spawn-and-wait contract holds
/// regardless of platform.
#[test]
fn agenthd_gui_with_companion_does_not_rewrite_settings_when_unchanged() {
    let checkout_dir = tempfile::tempdir().expect("checkout tempdir");
    let checkout = checkout_dir.path().join("checkout");
    std::fs::create_dir_all(checkout.join("agents")).expect("create checkout agents");
    let checkout_str = checkout.to_string_lossy().into_owned();

    let (bin_dir, bin) = isolated_binary();
    land_fake_companion(bin_dir.path());

    // Seed settings.json with the compact bytes that
    // `serde_json::to_vec` produces, ahead of the first run.
    // `save_settings` uses `to_vec_pretty`, so a rewrite
    // would change the bytes. We control the seeded bytes
    // exactly so the no-rewrite assertion is deterministic.
    let seed_home = tempfile::tempdir().expect("seed tempdir");
    let agenthd_root = seed_home.path().join(".agenthd");
    std::fs::create_dir_all(&agenthd_root).expect("create seeded agenthd root");
    let settings_file = agenthd_root.join("settings.json");
    let seeded = serde_json::json!({
        "checkout_path": checkout_str,
    });
    let seeded_bytes = serde_json::to_vec(&seeded).expect("seed compact bytes");
    std::fs::write(&settings_file, &seeded_bytes).expect("seed settings.json");

    let output = Command::new(&bin)
        .args(["gui", "--repo", &checkout_str])
        .env("HOME", seed_home.path())
        .env("XDG_CONFIG_HOME", seed_home.path().join("xdg"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn agenthd");
    assert_eq!(
        output.status.code(),
        Some(0),
        "fake companion exits 0; CLI must forward that, got: {:?}, stderr: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let after_bytes = std::fs::read(&settings_file).expect("settings.json still exists");
    assert_eq!(
        seeded_bytes, after_bytes,
        "settings.json bytes must be unchanged when --repo equals the persisted value"
    );

    drop(seed_home);
    drop(bin_dir);
    drop(checkout_dir);
}

/// `agenthd gui --repo <abs-missing-path>` fails closed: the
/// `validate_checkout_path` rejection in `resolve_checkout_path`
/// surfaces as exit code `1` and a stderr message that names
/// the missing path. `settings.json` is **not** written. The
/// fake companion (present adjacent) is irrelevant here — the
/// validator runs before the companion is spawned.
#[test]
fn agenthd_gui_with_companion_present_and_invalid_repo_fails_closed() {
    // The fake companion is irrelevant here — the validator
    // runs before the companion is spawned, so the
    // companion-present branch can never reach it. We still
    // land the fake so the test exercises the same boot path
    // shape as the companion-present test.
    let missing = {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist");
        // Keep `dir` alive until end of scope but DO NOT
        // create `path` — the validator must observe its
        // absence.
        (path, dir)
    };

    let (bin_dir, bin) = isolated_binary();
    land_fake_companion(bin_dir.path());

    let missing_str = missing.0.to_string_lossy().into_owned();
    let (status, stderr, home, xdg, _env_dir) =
        run_isolated(&bin, &["gui", "--repo", &missing_str]);

    assert!(
        !status.success(),
        "agenthd gui --repo <abs-missing> must exit non-zero, got: {status}"
    );
    assert_eq!(
        status.code(),
        Some(1),
        "validation failure must surface as exit 1 (boot-path error), got: {status:?}"
    );
    assert!(
        stderr.contains("configured checkout") || stderr.contains(&missing_str),
        "stderr must surface the configured-checkout validation failure, got: {stderr}"
    );

    // No settings.json write.
    assert_eq!(
        home_entries(&home),
        vec![std::ffi::OsString::from("sentinel.txt")],
        "HOME must contain only the sentinel; --repo validation failure must not write"
    );
    assert!(
        !home.join(".agenthd").join("settings.json").exists(),
        "settings.json must NOT be written when --repo validation fails"
    );
    assert!(
        !home.join(".agenthd").join("state.json").exists(),
        "state.json must NOT be written when --repo validation fails"
    );
    assert!(
        !xdg.join("opencode").exists(),
        "OpenCode target tree must NOT be created when --repo validation fails"
    );

    drop(missing.1);
    drop(bin_dir);
}
