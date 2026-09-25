//! Verbatim port of `src/store.rs`'s `mod tests` block. Re-homed to a
//! dedicated file so the production siblings stay focused; the tests
//! themselves are unchanged, modulo the explicit imports that replace
//! the old `use super::*;` shortcut.
//!
//! The store module's mod-relative items (`super::*` from here) cover
//! everything `pub` or `pub(crate)` at the module level (`Paths`,
//! `State`, `migrate_legacy_agenthd`, `hash_file`, `write_target`,
//! `sha256_hex`, `PLUGIN_FILENAME`). Items introduced by the split
//! that only tests need (`source_hash`) are pulled from their owning
//! submodule under `pub(in crate::store)` so the helper visibility
//! stays narrower than crate-wide.

use super::canonical::source_hash;
use super::sha256_hex;
use super::PLUGIN_FILENAME;
use super::*;
use crate::agent::{starter_agent, Agent, Mode, PermissionAction, STARTERS};
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

/// Lint helper: collect a summary of parsed agents without bailing on bad
/// files. Lives next to the tests because it is only used by them.
#[allow(dead_code)]
fn lint(paths: &Paths) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(e) = load_canonical(paths) {
        errors.push(e.to_string());
    }
    errors
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
        super::plugin::PLUGIN_SOURCE
    );
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&paths.plugin_config).unwrap()).unwrap();
    assert_eq!(config["plugin"][0], super::plugin::PLUGIN_SPEC);

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
    let (_wrote2, state) = seed_starters(&paths, State::load(&paths.state_file).unwrap()).unwrap();
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
        let agent = Agent::read(&path)
            .unwrap_or_else(|e| panic!("seeded `{}` does not parse as a valid agent: {}", name, e));
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

    let (state, outcome) = force_install(&paths, state, SyncTarget::OpenCode, "scout.md").unwrap();
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

    let (state, outcome) = force_install(&paths, state, SyncTarget::OpenCode, "scout.md").unwrap();
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
