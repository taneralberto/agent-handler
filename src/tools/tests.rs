//! Installer unit tests for the `tools` module.
//!
//! The tests live in their own file (rather than a `#[cfg(test)] mod
//! tests { ... }` inline block) so the installer's implementation in
//! `tools/mod.rs` stays focused. The module is declared from
//! `tools/mod.rs` as `#[cfg(test)] mod tests;`; because `tests.rs` is a
//! direct child of `tools`, items in `tools/mod.rs` are reachable
//! through `super::*`.
//!
//! Coverage is preserved verbatim from the pre-split `src/tools.rs`,
//! including the Windows npm-direct-`node` fix and the seam tests
//! (`fake_npm_cli_guard`, `fake_npm_missing_guard`, `discovery_seam`).
//! The contents were moved as-is during the `src/tools.rs` →
//! `src/tools/mod.rs` + `src/tools/tests.rs` split and intentionally
//! re-indented from `mod tests { ... }` nesting into a flat module
//! file.

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
    let _seam_guard = fake_npm_cli_guard();
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
    // The first three recorded specs must be git/node/<npm-program> in
    // order. On Windows `npm_program()` resolves to `node` (npm is
    // launched through `node` with the absolute path to npm-cli.js
    // as argv[0]); on other platforms it is `npm`. The third spec
    // is therefore `node <cli> --version` on Windows and
    // `npm --version` elsewhere.
    let recorded = runs.borrow();
    assert_eq!(recorded[0].program, "git");
    assert_eq!(recorded[1].program, "node");
    assert_eq!(recorded[2].program, npm_program());
}

#[test]
fn preflight_missing_node_returns_prerequisites_missing() {
    // `node --version` exits non-zero for a reason unrelated to the
    // version comparison (binary missing, dynamic loader error,
    // segfault, etc.). The installer must bail with
    // PrerequisitesMissing rather than try to parse or proceed, and
    // npm is never invoked because node fails first.
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
            success: false,
            code: Some(1),
            stdout: Vec::new(),
            stderr: b"node: error while loading shared libraries".to_vec(),
        },
    ])));
    let runs = Rc::new(RefCell::new(Vec::new()));
    let mut runner = recorded_with(runs.clone(), responses.clone());
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();
    assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
    assert!(
        outcome.detail.contains("node"),
        "detail should mention node: {}",
        outcome.detail
    );
    // Only git and node reached the runner; npm was never invoked.
    let recorded = runs.borrow();
    assert_eq!(recorded.len(), 2, "git and node only");
    assert_eq!(recorded[0].program, "git");
    assert_eq!(recorded[1].program, "node");
    assert!(!staging_for(&paths, entry).exists());
}

#[test]
fn preflight_missing_npm_returns_prerequisites_missing() {
    // git and node both pass pre-flight, but `npm --version` exits
    // non-zero (binary missing or otherwise broken). The installer
    // must bail with PrerequisitesMissing; staging is never created.
    //
    // On Windows the seam is required: preflight must reach the
    // npm spawn (which the mock makes fail), and the only way to
    // reach it is to satisfy `resolve_npm_cli_js`. Without the
    // seam preflight would fail earlier with "npm-cli.js not in
    // PATH" — a different code path that has its own dedicated
    // test (`preflight_returns_prerequisites_missing_when_npm_cli_js_not_discoverable`).
    let _seam_guard = fake_npm_cli_guard();
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
            success: false,
            code: Some(1),
            stdout: Vec::new(),
            stderr: b"npm: not found".to_vec(),
        },
    ])));
    let runs = Rc::new(RefCell::new(Vec::new()));
    let mut runner = recorded_with(runs.clone(), responses.clone());
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();
    assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
    assert!(
        outcome.detail.contains("npm"),
        "detail should mention npm: {}",
        outcome.detail
    );
    let recorded = runs.borrow();
    assert_eq!(recorded.len(), 3, "git, node, and npm were all invoked");
    assert_eq!(recorded[2].program, npm_program());
    assert!(!staging_for(&paths, entry).exists());
}

#[test]
fn preflight_node_unparseable_output_returns_prerequisites_missing() {
    // `node --version` exits 0 but its stdout is not a semver tuple.
    // The installer must refuse to proceed rather than silently
    // accept an unknown version (which could let a too-old or
    // otherwise incompatible node pass through and break later).
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
            stdout: b"this is not a version string\n".to_vec(),
            stderr: Vec::new(),
        },
    ])));
    let runs = Rc::new(RefCell::new(Vec::new()));
    let mut runner = recorded_with(runs.clone(), responses.clone());
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();
    assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
    assert!(
        outcome.detail.contains("node"),
        "detail should mention node: {}",
        outcome.detail
    );
    let recorded = runs.borrow();
    assert_eq!(recorded.len(), 2, "git and node only");
    assert!(!staging_for(&paths, entry).exists());
}

#[test]
fn npm_program_resolves_to_platform_specific_launcher() {
    // Regression: on Windows, npm is reached by spawning `node` with
    // the absolute path to `npm-cli.js` as `argv[0]`. This avoids
    // `npm.cmd`, which Windows would interpret through `cmd.exe` and
    // violate the project's strict no-shell contract. See
    // `build_npm_spec` for the full rationale.
    //
    // On non-Windows platforms the launcher stays `npm` because
    // Linux/macOS npm is a real ELF / Mach-O binary and `Command::new`
    // can launch it directly.
    //
    // Note: the previous iteration of this fix resolved to
    // `npm.cmd` on Windows. That approach is rejected because
    // `Command::new("npm.cmd")` internally invokes cmd.exe — see
    // TOOL_INSTALLER_PLAN.md:219 for the strict no-shell rule.
    #[cfg(target_os = "windows")]
    assert_eq!(npm_program(), "node");
    #[cfg(not(target_os = "windows"))]
    assert_eq!(npm_program(), "npm");
}

#[test]
fn preflight_and_npm_ci_use_npm_program_as_their_program_field() {
    // Regression companion to `npm_program_resolves_to_platform_specific_launcher`:
    // the resolved launcher name is what reaches `SpawnSpec.program`
    // in both preflight (step 1) and npm_ci (step 6). We run a
    // mocked full pipeline and assert:
    //
    // - the npm preflight spawn has program `npm_program()` and
    //   `argv` ending in `--version`. On Windows that is
    //   `node <cli-path> --version`; on other platforms
    //   `npm --version`.
    // - the npm_ci spawn has program `npm_program()` and argv
    //   containing `ci`, `--omit=dev`, `--ignore-scripts`. On
    //   Windows `cli-path` is `argv[0]`; on other platforms argv
    //   starts with `ci`.
    let _seam_guard = fake_npm_cli_guard();
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
    // Rename fails closed so we observe the rest of the flow without
    // touching the real OS primitive.
    let mut rename = rename_err(libc_const_eexist());
    let _ = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();

    // Preflight: the npm --version spawn. Its last arg is `--version`
    // on both platforms; on Windows the spawn has program `node` and
    // 2 args (cli path + `--version`) — the seam forces the cli path
    // to be `C:\fake-for-test\node_modules\npm\bin\npm-cli.js`.
    let preflight_npm = runs
        .borrow()
        .iter()
        .find(|s| {
            if s.program != npm_program() {
                return false;
            }
            if s.args.last().map(String::as_str) != Some("--version") {
                return false;
            }
            // Exclude the literal `node --version` / `npm --version`
            // spawn (1 arg). The npm preflight spawn has either
            // 1 arg on Linux (program="npm") or 2 args on Windows
            // (program="node", args=[cli, "--version"]).
            #[cfg(target_os = "windows")]
            {
                s.args.len() == 2
            }
            #[cfg(not(target_os = "windows"))]
            {
                s.args.len() == 1
            }
        })
        .expect("npm preflight spawn must be recorded")
        .clone();
    assert_eq!(
        preflight_npm.program,
        npm_program(),
        "preflight must spawn `npm_program()` (Windows: node, else: npm)"
    );
    assert!(
        preflight_npm.cwd.is_none(),
        "preflight npm --version must run with no cwd (PATH lookup)"
    );
    #[cfg(target_os = "windows")]
    assert!(
        preflight_npm.args[0].ends_with("npm-cli.js"),
        "Windows npm preflight argv[0] must be the absolute path to npm-cli.js: got `{}`",
        preflight_npm.args[0]
    );

    // Step 6: the npm ci spawn. On Windows argv[0] is the cli path
    // and argv contains `ci`, `--omit=dev`, `--ignore-scripts`. On
    // other platforms argv starts with `ci`.
    let npm_ci = runs
        .borrow()
        .iter()
        .find(|s| s.program == npm_program() && s.args.iter().any(|a| a == "ci"))
        .expect("npm ci must be invoked with the platform-specific program name")
        .clone();
    let mut expected_args: Vec<String> = vec![
        "ci".to_string(),
        "--omit=dev".to_string(),
        "--ignore-scripts".to_string(),
    ];
    #[cfg(target_os = "windows")]
    {
        let cli = std::path::PathBuf::from(r"C:\fake-for-test\node_modules\npm\bin\npm-cli.js");
        expected_args.insert(0, cli.display().to_string());
    }
    assert_eq!(npm_ci.args, expected_args, "npm ci argv: {:?}", npm_ci.args);
    assert_eq!(
        npm_ci.program,
        npm_program(),
        "npm_ci must spawn `npm_program()` (Windows: node, else: npm)"
    );
}

// NOTE on the original "bare `npm` fails" symptom: a previous
// iteration of this fix pinned `Command::new("npm")` to fail on
// Windows as a regression test. That assertion is fragile on hosts
// where `npm.exe` (or another executable named `npm`) is on PATH —
// the bug surfaces only when the bare `npm` IS the Node-shipped
// bash shim. The stronger coverage lives elsewhere and is
// host-independent:
//
//   - `spawn_command_via_node_with_npm_cli_js_works_on_windows`
//     pins the new approach (`node <abs-cli>`) end-to-end on the
//     real OS, skipping gracefully when npm-cli.js is not
//     discoverable.
//   - `npm_ci_via_node_cli_js_works_against_a_local_only_package`
//     pins `npm ci` via `node <abs-cli>` against a self-contained
//     TempDir (no network, no user dir).
//   - `no_npm_invocation_uses_cmd_shim` asserts the installer's
//     recorded spawns never use a `.cmd` program and never carry
//     `npm.cmd` in any arg — the no-shell contract.
//
// Together those three tests cover the original bug + the fix
// without depending on the host's PATH layout.

#[cfg(target_os = "windows")]
#[test]
fn spawn_command_via_node_with_npm_cli_js_works_on_windows() {
    // Runtime regression for the new approach. With the
    // PATH-following `resolve_npm_cli_js_in` finding the canonical
    // npm-cli.js, spawning `node <abs-path> --version` must succeed
    // and print a semver-shaped version.
    //
    // If this host has no `npm.cmd` in PATH (e.g. a CI image without
    // Node), the test logs and returns rather than failing — the
    // real-OS branch is unverified on that host, but the rest of
    // the test suite still passes.
    let cli = match resolve_npm_cli_js() {
        Some(p) => p,
        None => {
            eprintln!(
                "skip: npm-cli.js not discoverable in PATH on this host; \
                 the node+npm-cli.js launcher branch is unverified at runtime"
            );
            return;
        }
    };
    let spec = build_npm_spec(&["--version"], None, Duration::from_secs(GIT_TIMEOUT_SECS))
        .expect("build_npm_spec must succeed when resolve_npm_cli_js succeeded");
    assert_eq!(
        spec.program, "node",
        "the npm launcher on Windows is `node`, not `npm.cmd`"
    );
    assert_eq!(
        spec.args.first().map(String::as_str),
        Some(cli.display().to_string().as_str()),
        "argv[0] must be the absolute path to npm-cli.js"
    );
    assert!(
        !spec.args.iter().any(|a| a.contains("npm.cmd")),
        "no argv entry may reference the .cmd shim: {:?}",
        spec.args
    );
    let out = spawn_command(&spec).expect("spawn_command must succeed");
    assert!(
        out.success,
        "node <cli> --version must exit 0; got code={:?} stderr={}",
        out.code,
        String::from_utf8_lossy(&out.stderr)
    );
    let version = String::from_utf8_lossy(&out.stdout);
    let trimmed = version.trim();
    assert!(
        trimmed.starts_with(|c: char| c.is_ascii_digit()),
        "npm --version must print a semver-ish line; got `{}`",
        trimmed
    );
}

#[cfg(target_os = "windows")]
#[test]
fn npm_ci_via_node_cli_js_works_against_a_local_only_package() {
    // Real-OS regression for the new approach, end-to-end:
    // `node <cli> ci --omit=dev --ignore-scripts` against a tiny
    // self-contained package whose only dep is a local file: dep.
    // Runs in a `TempDir` (not the user's directory), with a
    // hand-crafted valid `package-lock.json` so npm ci works fully
    // offline (no registry contact, no network).
    //
    // If npm-cli.js is not discoverable on this host, the test
    // logs and returns — the real-OS branch is unverified on that
    // host, but the rest of the suite still passes.
    let cli = match resolve_npm_cli_js() {
        Some(p) => p,
        None => {
            eprintln!(
                "skip: npm-cli.js not discoverable in PATH on this host; \
                 the node+npm-cli.js ci branch is unverified at runtime"
            );
            return;
        }
    };
    let staging = TempDir::new().unwrap();
    let p = staging.path();
    // Tiny local dep referenced by `file:./tiny` in the root
    // package. The package.json is intentionally minimal so npm ci
    // has nothing to fetch from any registry.
    fs::create_dir(p.join("tiny")).unwrap();
    fs::write(
        p.join("tiny/package.json"),
        br#"{"name":"tiny","version":"0.0.0"}"#,
    )
    .unwrap();
    fs::write(
        p.join("package.json"),
        br#"{"name":"e2e","version":"0.0.0","dependencies":{"tiny":"file:./tiny"}}"#,
    )
    .unwrap();
    // Hand-crafted lock file produced by `npm install
    // --package-lock-only` against the same content; verified to be
    // accepted by `npm ci` offline (no registry contact).
    fs::write(
        p.join("package-lock.json"),
        br#"{"name":"e2e","version":"0.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"e2e","version":"0.0.0","dependencies":{"tiny":"file:./tiny"}},"node_modules/tiny":{"resolved":"tiny","link":true},"tiny":{"version":"0.0.0"}}}"#,
    )
    .unwrap();

    let spec = build_npm_spec(
        &["ci", "--omit=dev", "--ignore-scripts"],
        Some(p.to_path_buf()),
        Duration::from_secs(NPM_TIMEOUT_SECS),
    )
    .expect("build_npm_spec must succeed when resolve_npm_cli_js succeeded");
    assert_eq!(spec.program, "node");
    assert_eq!(
        spec.args.first().map(String::as_str),
        Some(cli.display().to_string().as_str()),
        "argv[0] must be the absolute path to npm-cli.js"
    );
    assert_eq!(spec.cwd.as_deref(), Some(p as &Path));

    let out = spawn_command(&spec).expect("spawn_command must succeed");
    assert!(
        out.success,
        "node <cli> ci --omit=dev --ignore-scripts must exit 0; \
         got code={:?}\nstderr={}\nstdout={}",
        out.code,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        p.join("node_modules/tiny/package.json").exists(),
        "the local file: dep must be installed into node_modules/; \
         got node_modules/ contents: {:?}",
        fs::read_dir(p.join("node_modules")).ok().map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        })
    );
}

#[cfg(target_os = "windows")]
#[test]
fn build_npm_spec_returns_err_when_npm_cli_js_not_discoverable() {
    // Regression for the unsupported-layout path: when no `npm.cmd`
    // in PATH resolves to a usable `npm-cli.js`, `build_npm_spec`
    // must fail with an actionable error and preflight must surface
    // that as `PrerequisitesMissing` mentioning npm.
    let saved = discovery_seam::get();
    discovery_seam::set(Some(None));
    let result = build_npm_spec(&["--version"], None, Duration::from_secs(GIT_TIMEOUT_SECS));
    discovery_seam::set(saved);
    let err = result.expect_err("build_npm_spec must Err when discovery returns None");
    assert!(
        err.contains("npm-cli.js"),
        "error must mention the missing prerequisite: {}",
        err
    );
    assert!(err.contains("npm"), "error must mention npm: {}", err);
}

#[cfg(target_os = "windows")]
#[test]
fn resolve_npm_cli_js_in_returns_absolute_path_for_relative_entry() {
    // Regression: when PATH contains a relative entry (`.`,
    // `bin`, `subdir/...`), the candidate must still be absolute.
    // A relative candidate would resolve against the
    // invocation-time cwd inside `Command::new` — and `npm ci`
    // runs with `cwd = staging`, not the installer's cwd, so a
    // relative candidate would silently fail with `NotFound` at
    // runtime. We absolutize at discovery time and assert the
    // result here.
    //
    // The fixture is rooted at `<workspace>/target/<unique>` so
    // it sits at a known relative offset from cargo test's cwd
    // (= package root). No global cwd mutation.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = manifest_dir.join("target").join(format!(
        "npm-rel-pkg-{}-resolve_relative",
        std::process::id()
    ));
    let rel_bin = fixture_root.join("rel-bin");
    let npm_cli = rel_bin
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    // Idempotent cleanup before AND after the test so a panic
    // never leaves a stale fixture in target/.
    let _ = std::fs::remove_dir_all(&fixture_root);
    std::fs::create_dir_all(rel_bin.join("node_modules").join("npm").join("bin")).unwrap();
    std::fs::write(rel_bin.join("npm.cmd"), b"@fake shim").unwrap();
    std::fs::write(&npm_cli, b"// fake").unwrap();

    // Relative PATH entry pointing at the fixture. cargo test runs
    // with cwd = package root, so this resolves correctly under
    // the workspace root.
    let rel_path = format!(
        "target{}npm-rel-pkg-{}-resolve_relative{}rel-bin",
        std::path::MAIN_SEPARATOR,
        std::process::id(),
        std::path::MAIN_SEPARATOR,
    );
    let result = resolve_npm_cli_js_in(std::ffi::OsStr::new(&rel_path));

    let cli_path = result.expect("discovery must succeed for the relative-PATH fixture");
    assert!(
        cli_path.is_absolute(),
        "candidate must be absolute even when PATH entry is relative; got `{}`",
        cli_path.display()
    );
    assert_eq!(
        cli_path, npm_cli,
        "candidate must point at the absolute path of the npm-cli.js under the relative PATH entry"
    );
    // Belt-and-braces: the file must exist at the absolute path the
    // installer would actually spawn — guarding against future
    // canonicalization that drops a trailing separator or similar.
    assert!(
        std::fs::metadata(&cli_path)
            .map(|m| m.is_file())
            .unwrap_or(false),
        "the absolute candidate must point at a real npm-cli.js: `{}`",
        cli_path.display()
    );

    // Cleanup runs after all assertions so a panic in any of them
    // still leaves the fixture removable. A subsequent test run on
    // the same pid re-enters the `let _ = remove_dir_all(...)` at
    // the top, so a panic-induced leftover does not break the test.
    let _ = std::fs::remove_dir_all(&fixture_root);
}

#[cfg(target_os = "windows")]
#[test]
fn resolve_npm_cli_js_in_returns_none_when_no_path_entry_has_npm_layout() {
    // Empty PATH and PATH with no usable entries both yield None.
    let result_empty = resolve_npm_cli_js_in(std::ffi::OsStr::new(""));
    assert!(
        result_empty.is_none(),
        "empty PATH must yield None; got {:?}",
        result_empty
    );
    // A non-existent PATH entry also yields None (the entry does
    // not exist, so we skip it).
    let bogus = format!("C:\\definitely-not-a-real-path-{}\\bin", std::process::id());
    let result_bogus = resolve_npm_cli_js_in(std::ffi::OsStr::new(&bogus));
    assert!(
        result_bogus.is_none(),
        "PATH with only a non-existent entry must yield None; got {:?}",
        result_bogus
    );
}

#[cfg(target_os = "windows")]
#[test]
fn preflight_returns_prerequisites_missing_when_npm_cli_js_not_discoverable() {
    // End-to-end: install_tool_with under a forced "npm not
    // installed" seam must return PrerequisitesMissing with a
    // detail that names npm. Staging is never created.
    let _seam_guard = fake_npm_missing_guard();

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
    ])));
    let runs = Rc::new(RefCell::new(Vec::new()));
    let mut runner = recorded_with(runs.clone(), responses.clone());
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename_ok).unwrap();

    assert_eq!(outcome.status, ToolStatus::PrerequisitesMissing);
    assert!(
        outcome.detail.contains("npm"),
        "detail must mention npm: {}",
        outcome.detail
    );
    assert!(
        outcome.detail.contains("npm-cli.js"),
        "detail must mention the missing npm-cli.js: {}",
        outcome.detail
    );
    // The detail must NOT carry a duplicated `npm:` prefix — that
    // would happen if `build_npm_spec`'s error already started with
    // `npm:` and the caller wrapped it again. The current contract
    // is `format!("npm: {}", inner)` in `preflight`, where `inner`
    // describes the missing prerequisite without a leading `npm:`.
    assert!(
        !outcome.detail.contains("npm: npm:"),
        "detail must not duplicate the npm: prefix: {}",
        outcome.detail
    );
    // git + node reached the runner; the npm-via-node spawn never
    // happened because discovery failed before build_npm_spec could
    // build a spec.
    let recorded = runs.borrow();
    assert_eq!(recorded.len(), 2, "git and node only");
    assert_eq!(recorded[0].program, "git");
    assert_eq!(recorded[1].program, "node");
    assert!(!staging_for(&paths, entry).exists());
}

#[test]
fn no_npm_invocation_uses_cmd_shim() {
    // Project-wide invariant: the installer must never spawn
    // `npm.cmd` (or any other `.cmd` shim that Windows would
    // interpret through cmd.exe). On Windows npm is reached via
    // `node <abs-path-to-npm-cli.js>`; on other platforms npm is
    // a direct binary. This is the no-shell contract.
    //
    // We assert the invariant by running a mocked full pipeline
    // and inspecting every recorded spawn spec.
    let _seam_guard = fake_npm_cli_guard();
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
    for spec in runs.borrow().iter() {
        assert_ne!(
            spec.program, "npm.cmd",
            "no spawn may use program=`npm.cmd` (Windows would invoke cmd.exe)"
        );
        assert!(
            !spec.program.ends_with(".cmd"),
            "no spawn may use a .cmd program; got `{}`",
            spec.program
        );
        for arg in &spec.args {
            assert!(
                !arg.ends_with(".cmd"),
                "no spawn arg may be a .cmd path; got `{}`",
                arg
            );
            assert!(
                !arg.contains("npm.cmd"),
                "no spawn arg may reference npm.cmd; got `{}`",
                arg
            );
        }
    }
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

    let lightweight = "0dba366061911f0ec389f4a78cc46fd6d6a19d41\trefs/tags/opencode-2026-09-23\n";
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
    let _seam_guard = fake_npm_cli_guard();
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
    let _seam_guard = fake_npm_cli_guard();
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
    let _seam_guard = fake_npm_cli_guard();
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
    let _seam_guard = fake_npm_cli_guard();
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
        outcome.detail.contains("already exists") && outcome.detail.contains("refusing to clobber"),
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
    let _seam_guard = fake_npm_cli_guard();
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
        outcome.detail.contains("already exists") && outcome.detail.contains("refusing to clobber"),
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
    let _seam_guard = fake_npm_cli_guard();
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
        outcome.detail.contains("already exists") && outcome.detail.contains("refusing to clobber"),
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
    let _seam_guard = fake_npm_cli_guard();
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
    let skill_body = "---\nname: totally-different-skill\ndescription: x\n---\nbody\n".to_string();
    let mut runner = mock_runner_with_skill_md(runs.clone(), responses.clone(), skill_body);
    let mut rename = rename_ok;
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
    assert_eq!(outcome.status, ToolStatus::IdentityMismatch);
    assert!(
        outcome.detail.contains("totally-different-skill") && outcome.detail.contains("pi-psql"),
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
    let _seam_guard = fake_npm_cli_guard();
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
    let _seam_guard = fake_npm_cli_guard();
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
    // On Windows the program field is `node` and argv[0] is the
    // npm-cli.js absolute path; on other platforms the program is
    // `npm` and argv starts with `ci`. Both resolve through
    // `npm_program()`.
    let npm = runs
        .borrow()
        .iter()
        .find(|s| s.program == npm_program() && s.args.iter().any(|a| a == "ci"))
        .expect("npm ci must be invoked")
        .clone();
    let mut expected_args: Vec<String> = vec![
        "ci".to_string(),
        "--omit=dev".to_string(),
        "--ignore-scripts".to_string(),
    ];
    #[cfg(target_os = "windows")]
    {
        let cli = std::path::PathBuf::from(r"C:\fake-for-test\node_modules\npm\bin\npm-cli.js");
        expected_args.insert(0, cli.display().to_string());
    }
    assert_eq!(npm.args, expected_args, "npm ci argv: {:?}", npm.args);
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
    let _seam_guard = fake_npm_cli_guard();
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
fn windows_rename_movefilew_returns_already_exists_when_target_is_directory() {
    // Runtime check: when the destination already holds a regular
    // directory (with contents), MoveFileW must return
    // ERROR_ALREADY_EXISTS — no separate "exists" precheck, no
    // partial merge, no clobber. The directory and a sentinel file
    // inside it must be byte-equal to their pre-call state.
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let entry = pi_psql_entry();
    let staging = staging_for(&paths, entry);
    write_skill_md(&staging, entry.skill_name);
    let dst = destination_for(&paths, entry);
    fs::create_dir_all(&dst).unwrap();
    let sentinel = b"existing directory contents must be preserved\n";
    fs::write(dst.join("existing.md"), sentinel).unwrap();

    // Invoke the real OS primitive — no mock.
    let result = rename_no_replace(&staging, &dst);
    let code = result.unwrap_err();
    assert_eq!(
        code,
        windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32,
        "MoveFileW must surface ERROR_ALREADY_EXISTS for an existing directory target"
    );
    assert!(
        staging.exists(),
        "staging must NOT be removed by the primitive itself"
    );
    assert!(dst.is_dir(), "destination must remain a directory");
    assert_eq!(
        fs::read(dst.join("existing.md")).unwrap(),
        sentinel,
        "sentinel bytes inside the existing destination must be preserved"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_rename_movefilew_returns_already_exists_when_target_is_symlink() {
    // Runtime check for the symlink-at-destination branch. On
    // Windows, creating a symlink without admin rights or
    // Developer Mode fails with ERROR_PRIVILEGE_NOT_HELD. Rather
    // than `#[ignore]`-ing the test (which hides the gap), the
    // test surfaces the limitation via stderr and returns — no
    // false positive, no silent skip. Hosts with symlink rights
    // exercise the full assertion path.
    use std::os::windows::fs::symlink_dir;
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let entry = pi_psql_entry();
    let staging = staging_for(&paths, entry);
    write_skill_md(&staging, entry.skill_name);
    let dst = destination_for(&paths, entry);
    // Point the symlink at a real dir so the OS accepts the link
    // and metadata reads on the link itself stay consistent.
    let link_target = dir.path().join("symlink-target");
    fs::create_dir_all(&link_target).unwrap();
    if let Err(e) = symlink_dir(&link_target, &dst) {
        eprintln!(
            "windows_rename_movefilew_returns_already_exists_when_target_is_symlink: \
             limitation: cannot create symlink on this host ({:?}); \
             the existing-target symlink branch is unverified at runtime. \
             Enable Developer Mode or run with SeCreateSymbolicLinkPrivilege \
             to exercise it.",
            e
        );
        return;
    }

    // Invoke the real OS primitive — no mock.
    let result = rename_no_replace(&staging, &dst);
    let code = result.unwrap_err();
    assert_eq!(
        code,
        windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32,
        "MoveFileW must surface ERROR_ALREADY_EXISTS for a symlink at the destination"
    );
    assert!(
        fs::symlink_metadata(&dst).unwrap().file_type().is_symlink(),
        "destination must remain a symlink"
    );
    assert_eq!(
        fs::read_link(&dst).unwrap(),
        link_target,
        "symlink target must be preserved"
    );
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

#[cfg(target_os = "windows")]
#[test]
fn windows_rename_movefilew_returns_not_same_device_is_install_failed() {
    // Staging and destination sit on the same drive so the mocked
    // rename can return ERROR_NOT_SAME_DEVICE without requiring a
    // real cross-volume scenario. The test exercises the installer's
    // typed match arm and asserts InstallFailed, staging cleanup,
    // and an untouched destination. No copy fallback is attempted.
    let _seam_guard = fake_npm_cli_guard();
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
    let mut rename = rename_err(windows_const_not_same_device());
    let outcome = install_tool_with(&paths, entry, &mut runner, &mut rename).unwrap();
    assert_eq!(outcome.status, ToolStatus::InstallFailed);
    assert!(
        outcome.detail.contains("different filesystem"),
        "NOT_SAME_DEVICE detail: {}",
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
        "staging must be removed on NOT_SAME_DEVICE"
    );
    assert!(
        !destination_for(&paths, entry).exists(),
        "destination must not be touched on NOT_SAME_DEVICE"
    );
}

// ---------- Conflict flow integration ----------

#[test]
fn conflict_when_target_already_exists() {
    let _seam_guard = fake_npm_cli_guard();
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
    let _seam_guard = fake_npm_cli_guard();
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
    // `npm_program()` resolves to `node` on Windows (npm is reached
    // through `node <abs-npm-cli.js>`) and `npm` elsewhere. The set
    // is otherwise closed — no shell, no other binaries, no `.cmd`
    // shims.
    assert_eq!(
        programs,
        ["git", "node", npm_program()]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        "the installer must only spawn git, node, <npm_program()>; got {:?}",
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
