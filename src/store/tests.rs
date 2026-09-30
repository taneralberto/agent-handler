//! Store unit tests. The `pub(super)` helper visibility matches the
//! existing convention; consumers reach into the store via the
//! `crate::store` re-exports.

use super::canonical::source_hash;
use super::sha256_hex;
use super::*;
use crate::agent::{starter_agent, Agent, Mode, PermissionAction, STARTERS};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

fn setup_paths(dir: &TempDir) -> Paths {
    let paths = Paths {
        agenthd_root: dir.path().join(".agenthd"),
        canonical_dir: dir.path().join(".agenthd").join("agents"),
        state_file: dir.path().join(".agenthd").join("state.json"),
        target_dir: dir.path().join(".config").join("opencode").join("agents"),
        pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
        skills_dir: dir.path().join(".config").join("opencode").join("skills"),
        settings_file: dir.path().join(".agenthd").join("settings.json"),
    };
    paths.ensure_dirs().unwrap();
    paths
}

/// Build a `Paths` whose `canonical_dir` points at a real
/// `<checkout>/agents` directory inside the tempdir, and seed the
/// bundled starters into it. Mirrors what the production runtime
/// does after `with_settings` is called: the canonical directory is
/// the checkout's `agents/`, the agenthd root stays empty of
/// agent files.
fn setup_paths_with_checkout(dir: &TempDir) -> (Paths, std::path::PathBuf) {
    let paths = setup_paths(dir);
    let checkout = dir.path().join("checkout");
    let agents = checkout.join("agents");
    fs::create_dir_all(&agents).unwrap();
    for starter in STARTERS {
        fs::write(
            agents.join(format!("{}.md", starter.name)),
            starter_agent(starter).render(),
        )
        .unwrap();
    }
    let paths = Paths {
        canonical_dir: agents,
        ..paths
    };
    (paths, checkout)
}

fn read_target(paths: &Paths, name: &str) -> Option<String> {
    let path = paths.target_dir.join(format!("{}.md", name));
    fs::read_to_string(&path).ok()
}

fn read_pi_target(paths: &Paths, name: &str) -> Option<String> {
    let path = paths.pi_target_dir.join(format!("{}.md", name));
    fs::read_to_string(&path).ok()
}

#[test]
fn paths_resolve_agenthd_root_follows_home_only() {
    let p = Paths::resolve(Some("/tmp/abc"), Some("/tmp/home")).unwrap();
    assert_eq!(p.agenthd_root, PathBuf::from("/tmp/home/.agenthd"));
    assert_eq!(p.canonical_dir, PathBuf::from("/tmp/home/.agenthd/agents"));
    assert_eq!(p.state_file, PathBuf::from("/tmp/home/.agenthd/state.json"));
    assert_eq!(p.target_dir, PathBuf::from("/tmp/abc/opencode/agents"));
    assert_eq!(p.pi_target_dir, PathBuf::from("/tmp/home/.pi/agent/agents"));
    assert_eq!(
        p.settings_file,
        PathBuf::from("/tmp/home/.agenthd/settings.json")
    );
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
    assert!(Paths::resolve(Some("/xdg"), None).is_err());
    assert!(Paths::resolve(None, None).is_err());
}

#[test]
fn paths_reject_empty_home() {
    assert!(Paths::resolve(Some("/xdg"), Some("")).is_err());
    assert!(Paths::resolve(None, Some("")).is_err());
}

#[test]
fn settings_default_canonical_dir_is_local_default() {
    // Without a `with_settings` call the canonical dir is the
    // historical local default. The runtime always re-points it
    // through `with_settings` on startup; tests rely on this so
    // they can construct a `Paths` and immediately mutate it.
    let p = Paths::resolve(None, Some("/tmp/home")).unwrap();
    assert_eq!(p.canonical_dir, PathBuf::from("/tmp/home/.agenthd/agents"));
}

#[test]
fn with_settings_repoints_canonical_dir() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let settings = Settings::new(checkout.to_string_lossy().into_owned());
    let after = paths.with_settings(&settings).unwrap();
    assert_eq!(after.canonical_dir, checkout.join("agents"));
}

#[test]
fn with_settings_rejects_missing_checkout() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    // Use a non-existent subpath of an existing tempdir so the
    // path is absolute on the host platform. The Unix-style
    // literal `/this/path/...` is NOT absolute on Windows and
    // would trip the absolute-path gate instead of the
    // missing-checkout gate this test is asserting.
    let missing = dir.path().join("does-not-exist");
    let settings = Settings::new(missing.to_string_lossy());
    let err = paths.with_settings(&settings).unwrap_err().to_string();
    assert!(
        err.contains("does not exist"),
        "expected missing-checkout error, got: {err}"
    );
}

#[test]
fn compute_plan_is_read_only() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().to_str().unwrap();
    // Compute-plan now validates the configured canonical source
    // up front, so the "read-only" contract is exercised against
    // a real (but empty) checkout instead of an absent one. A
    // missing checkout is an explicit error covered by the
    // `plan_for_rejects_missing_source` test below.
    let mut paths = Paths::resolve(None, Some(home)).unwrap();
    let checkout = dir.path().join("checkout");
    std::fs::create_dir_all(checkout.join("agents")).unwrap();
    paths.canonical_dir = checkout.join("agents");
    assert!(compute_plan(&paths, &State::default()).unwrap().is_empty());
    // The planner must not create target directories on a fresh
    // checkout that has nothing to install.
    assert!(!paths.target_dir.exists());
    assert!(!paths.pi_target_dir.exists());
    // And it must not silently recreate the agenthd-root agents
    // shortcut that the single-source design retired.
    assert!(!paths.agenthd_root.join("agents").exists());
}

#[test]
fn apply_safe_fresh_install() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let first = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, first).unwrap();
    let second = compute_plan(&paths, &state).unwrap();
    let (state, outcomes) = apply_safe(&paths, state, second).unwrap();
    assert!(outcomes.iter().all(|o| o.action == "kept"));
    assert_eq!(state.installed.len(), STARTERS.len());
}

#[test]
fn apply_safe_update_when_target_matches_last_installed() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let first = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, first).unwrap();

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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    delete_canonical(&paths, "scout").unwrap();
    let plan = compute_plan(&paths, &state).unwrap();
    let scout = plan.iter().find(|i| i.filename == "scout.md").unwrap();
    assert_eq!(scout.status, SyncStatus::Remove);
    let (state, _) = apply_safe(&paths, state, plan).unwrap();
    assert!(!paths.target_dir.join("scout.md").exists());

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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (_, _) = apply_safe(&paths, state, plan).unwrap();

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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);

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

    let err = rename_canonical(&paths, "assistant", "scout");
    assert!(err.is_err());

    delete_canonical(&paths, "assistant").unwrap();
    assert!(!paths.canonical_dir.join("assistant.md").exists());
}

#[test]
fn rejects_non_regular_target() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let first = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, first).unwrap();
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);

    let mut agent = Agent::read(&paths.canonical_dir.join("scout.md")).unwrap();
    agent.name = "scout".to_string();
    let err = save_canonical(&paths, &agent, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("refusing to overwrite"));
    assert!(fs::read_to_string(paths.canonical_dir.join("scout.md"))
        .unwrap()
        .contains("read-only codebase scout"));
}

#[test]
fn save_canonical_rejects_stale_write() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);

    let scout_path = paths.canonical_dir.join("scout.md");
    let stale_hash = hash_file(&scout_path).unwrap();
    let original = fs::read_to_string(&scout_path).unwrap();
    let external = original.replace("read-only codebase scout", "externally rewritten");
    fs::write(&scout_path, &external).unwrap();

    let mut agent = Agent::read(&scout_path).unwrap();
    agent.prompt = "Editor edit".to_string();
    let err = save_canonical(&paths, &agent, stale_hash.as_deref())
        .unwrap_err()
        .to_string();
    assert!(err.contains("changed on disk"));
    assert!(fs::read_to_string(&scout_path)
        .unwrap()
        .contains("externally rewritten"));
}

#[test]
fn ghost_manifest_entry_has_distinct_reason() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    // The planner now validates the canonical source up front, so
    // seed an empty checkout-shaped agents directory for the
    // manifest-only scenario this test exercises.
    let checkout = dir.path().join("ghost-checkout");
    std::fs::create_dir_all(checkout.join("agents")).unwrap();
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
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
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    let target = paths.target_dir.join("scout.md");
    let original = fs::read_to_string(&target).unwrap();
    fs::write(&target, original.replace("read-only", "tampered")).unwrap();
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
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    // Seed an empty checkout-shaped agents directory so the
    // canonical-source validation gate accepts the path. The test
    // itself only inspects a manifest-only ghost entry, so the
    // checkout stays empty.
    let checkout = dir.path().join("manifest-only-checkout");
    std::fs::create_dir_all(checkout.join("agents")).unwrap();
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
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
    // Seed an empty checkout-shaped agents directory so the
    // canonical-source validation gate accepts the path. The test
    // focuses on a target-side orphan, not on canonical contents.
    let checkout = dir.path().join("target-orphan-checkout");
    std::fs::create_dir_all(checkout.join("agents")).unwrap();
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
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
    // `ensure_dirs` no longer creates `canonical_dir`: create it
    // here because the loader needs an actual directory to read.
    fs::create_dir_all(&paths.canonical_dir).unwrap();
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
fn load_canonical_accepts_empty_agents_directory() {
    // An empty `agents/` directory is intentional. A freshly cloned
    // checkout with no `.md` files produces an empty map, not an
    // error — the user may have cleared the directory themselves.
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let agents = dir.path().join("checkout-empty").join("agents");
    fs::create_dir_all(&agents).unwrap();
    let paths = Paths {
        canonical_dir: agents,
        ..paths
    };
    let agents = load_canonical(&paths).unwrap();
    assert!(agents.is_empty());
}

#[test]
fn ensure_dirs_does_not_create_canonical() {
    // `Paths::ensure_dirs` creates the agenthd root (settings +
    // state parents) and the output target trees. It must NOT
    // create `canonical_dir`: before a checkout is configured
    // there is no local canonical directory, and creating one
    // would silently turn `<agenthd_root>/agents` into a real
    // source that the runtime never intends to ship.
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    // Reset to a fresh resolve so canonical_dir is the default and
    // ensure_dirs has not yet been called.
    let paths = Paths::resolve(None, Some(dir.path().to_str().unwrap())).unwrap();
    paths.ensure_dirs().unwrap();
    let agenthd_agents = paths.agenthd_root.join("agents");
    assert!(
        !agenthd_agents.exists(),
        "ensure_dirs must not create the local canonical directory"
    );
    // The output targets are present so the install / sync layers
    // have somewhere to write.
    assert!(paths.target_dir.is_dir());
    assert!(paths.pi_target_dir.is_dir());
    assert!(paths.skills_dir.is_dir());
    assert!(paths.agenthd_root.is_dir());
}

#[test]
fn ensure_dirs_does_not_create_checkout_agents() {
    // The same contract applies after `with_settings` re-points
    // canonical_dir at a configured checkout: ensure_dirs still
    // must not create `<checkout>/agents/`. A configured checkout
    // must already contain an `agents/` directory (validated at
    // settings load); the runtime never has to make it.
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let settings = Settings::new(checkout.to_string_lossy().into_owned());
    let paths = paths.with_settings(&settings).unwrap();
    paths.ensure_dirs().unwrap();
    let agenthd_agents = paths.agenthd_root.join("agents");
    assert!(
        !agenthd_agents.exists(),
        "ensure_dirs must not create <agenthd_root>/agents even after with_settings"
    );
    // The configured checkout's agents directory is untouched by
    // ensure_dirs.
    assert!(checkout.join("agents").is_dir());
}

#[test]
fn startup_does_not_create_agenthd_agents() {
    // The runtime never writes into `<agenthd_root>/agents/`: the
    // canonical directory is always the configured checkout's
    // `agents/`. After `Paths::resolve` + `with_settings` +
    // `ensure_dirs` the agenthd-root agents directory must NOT
    // exist: there is no legacy sibling directory the runtime
    // creates for backwards compatibility — the binary never had
    // a Local mode in the single-source design. This test pins the
    // contract: a fresh machine with a configured checkout never
    // has its agenthd-root agents touched by a CRUD round-trip.
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let settings = Settings::new(checkout.to_string_lossy().into_owned());
    let paths = paths.with_settings(&settings).unwrap();
    let agenthd_agents = paths.agenthd_root.join("agents");
    assert!(
        !agenthd_agents.exists(),
        "agenthd_root/agents must never exist"
    );
    assert!(!agenthd_agents.join("scout.md").exists());
    // CRUD round-trip on the checkout does not touch the absent
    // legacy directory.
    let mut agent = Agent::new_default("helper".to_string()).unwrap();
    agent.description = "Test helper".to_string();
    agent.prompt = "help".into();
    save_canonical(&paths, &agent, None).unwrap();
    assert!(checkout.join("agents").join("helper.md").exists());
    assert!(!agenthd_agents.join("helper.md").exists());
}

#[test]
fn settings_round_trip() {
    let dir = TempDir::new().unwrap();
    let path = settings_file_path(&dir.path().join(".agenthd"));
    let settings = Settings::new("/some/abs/path");
    save_settings(&path, &settings).unwrap();
    let loaded = load_settings(&path).unwrap().unwrap();
    assert_eq!(loaded, settings);
}

#[test]
fn canonical_dir_from_rejects_missing_checkout() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join(".agenthd");
    // Non-existent subpath of an existing tempdir so the path is
    // absolute on the host platform (the Unix-style literal
    // `/this/path/...` is NOT absolute on Windows and would trip
    // the absolute-path gate instead of the missing-checkout
    // gate this test is asserting).
    let missing = dir.path().join("does-not-exist");
    let settings = Settings::new(missing.to_string_lossy());
    let err = canonical_dir_from(&root, &settings)
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not exist"), "got: {err}");
}

#[test]
fn canonical_dir_from_rejects_empty_checkout_path() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join(".agenthd");
    let settings = Settings::new("");
    let err = canonical_dir_from(&root, &settings)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no checkout_path"), "got: {err}");
}

#[test]
fn validate_checkout_accepts_empty_agents_dir() {
    let dir = TempDir::new().unwrap();
    let agents = dir.path().join("agents");
    fs::create_dir_all(&agents).unwrap();
    validate_checkout_path(dir.path()).unwrap();
}

#[test]
fn validate_checkout_rejects_missing_agents_subdir() {
    let dir = TempDir::new().unwrap();
    let err = validate_checkout_path(dir.path()).unwrap_err().to_string();
    assert!(err.contains("does not exist"), "got: {err}");
}

#[test]
fn _ensure_mode_action_in_scope() {
    let mut map: BTreeMap<String, Mode> = BTreeMap::new();
    map.insert("a".into(), Mode::subagent);
    let _action = PermissionAction::Allow;
    assert!(map.contains_key("a"));
}

// ---------- Safety tests: canonical-source validation ----------
//
// The four tests below pin the missing-checkout / missing-agents /
// symlinked-source fail-closed contract that the runtime now
// enforces at every store entry point. The contract is the same
// one `with_settings` / `validate_checkout_path` already enforce at
// startup; the new tests cover what happens when the configured
// source disappears or is replaced with a symlink AFTER startup —
// the case the original silent-empty `load_canonical` and
// `read_md_filenames` paths got wrong.

/// `load_canonical` must surface an explicit error when the
/// configured checkout has been removed between launches — not
/// return an empty map the UI would then render as "no agents".
/// The runtime cannot tell the difference between a checkout the
/// user genuinely cleared and a checkout that was deleted out of
/// band, so it must fail closed and surface the failure rather
/// than silently treat the missing source as an empty set.
#[test]
fn load_canonical_rejects_missing_canonical_dir() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    // `setup_paths` leaves `canonical_dir` pointing at the
    // never-created `<agenthd_root>/agents`. The validation gate
    // must refuse to load from a missing source.
    let err = load_canonical(&paths).unwrap_err().to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "expected missing-source error, got: {err}"
    );
}

/// `load_canonical` on a checkout root that is a symlink must be
/// refused even if the symlink target is a valid directory. The
/// canonical source is the only indirection the runtime permits —
/// anything else (especially a symlink pointing at an unrelated
/// directory the user did not pick) must surface as an error so
/// the user can repoint the configuration through Settings.
#[test]
fn load_canonical_rejects_symlinked_source() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let target = dir.path().join("real-source");
    fs::create_dir_all(target.join("agents")).unwrap();
    let link = dir.path().join("checkout-link");
    #[cfg(unix)]
    let made_link = std::os::unix::fs::symlink(&target, &link).is_ok();
    #[cfg(windows)]
    let made_link = std::os::windows::fs::symlink_dir(&target, &link).is_ok();
    if !made_link {
        // Symlink creation can fail in sandboxed CI environments
        // or on Windows without SeCreateSymbolicLinkPrivilege.
        return;
    }
    let paths = Paths {
        canonical_dir: link.join("agents"),
        ..paths
    };
    let err = load_canonical(&paths).unwrap_err().to_string();
    assert!(
        err.contains("symlink") || err.contains("not a directory"),
        "expected symlink rejection, got: {err}"
    );
}

/// A valid empty checkout must still produce an empty map and MUST
/// NOT create `<agenthd_root>/agents/`. The safety gate must not
/// reintroduce the historical local-default canonical shortcut the
/// single-source design retired.
#[test]
fn load_canonical_on_valid_empty_dir_does_not_create_agenthd_agents() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("empty-checkout");
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
    let agents = load_canonical(&paths).unwrap();
    assert!(agents.is_empty());
    assert!(
        !paths.agenthd_root.join("agents").exists(),
        "loading an empty checkout must not recreate <agenthd_root>/agents"
    );
}

/// `plan_for` must refuse to plan when the configured canonical
/// source is missing. Without this check, a removed checkout would
/// produce an empty plan that downstream UI would happily render
/// — and a precomputed `Remove` entry for an installed target
/// would silently delete that target on the next `apply_safe`.
#[test]
fn plan_for_rejects_missing_source() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let err = plan_for(&paths, &State::default(), SyncTarget::OpenCode)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "expected missing-source error, got: {err}"
    );
}

/// Simulates the directory-removal-after-launch scenario the
/// safety gate is built for: the user installs an agent, then the
/// checkout is removed out of band. A subsequent `plan_for` must
/// error (rather than produce a `Remove` plan), and `apply_safe`
/// invoked with the stale precomputed `Remove` plan must NOT
/// delete the installed target — the validation gate must run
/// before any item in the plan is honored, including the
/// precomputed `Remove` entry the previous `plan_for` produced
/// before the checkout vanished.
///
/// This is the regression test for the bug where a precomputed
/// `Remove` could remove an installed target after the configured
/// checkout disappears: the runtime must keep the installed
/// target on disk until the user resolves the missing-source error.
#[test]
fn apply_safe_with_precomputed_remove_does_not_delete_target_after_source_removal() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    // Install a starter so the target is owned by agenthd.
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();
    let scout_target = paths.target_dir.join("scout.md");
    assert!(
        scout_target.exists(),
        "scout.md must be installed on the OpenCode target"
    );
    // Drop the canonical file AND the entire checkout out of band
    // — this is the state the runtime would observe if the user
    // `rm -rf`'d their checkout between two Install/Update ticks.
    fs::remove_file(paths.canonical_dir.join("scout.md")).unwrap();
    fs::remove_dir_all(&checkout).unwrap();
    // A fresh planner call MUST error rather than produce an empty
    // or `Remove`-laden plan.
    let plan_err = plan_for(&paths, &state, SyncTarget::OpenCode).unwrap_err();
    assert!(
        plan_err.to_string().contains("does not exist"),
        "fresh plan after source removal must fail closed, got: {plan_err}"
    );
    // And a stale precomputed `Remove` plan (which we hand-craft
    // here to model the bug) MUST be refused by `apply_safe`
    // before any file is removed. The installed target stays put.
    let stale_remove = vec![SyncItem {
        target: SyncTarget::OpenCode,
        filename: "scout.md".to_string(),
        status: SyncStatus::Remove,
        canonical_hash: None,
        target_hash: Some(sha256_hex(b"placeholder")),
        last_installed_hash: Some(sha256_hex(b"placeholder")),
        canonical_path: paths.canonical_dir.join("scout.md"),
        target_path: scout_target.clone(),
    }];
    let apply_err = apply_safe(&paths, state, stale_remove).unwrap_err();
    assert!(
        apply_err.to_string().contains("does not exist"),
        "stale precomputed Remove plan must fail closed, got: {apply_err}"
    );
    assert!(
        scout_target.exists(),
        "installed target must survive a precomputed Remove plan after source removal"
    );
}

/// `apply_safe` must also refuse to install or update when the
/// configured checkout has been removed. Without this check, a
/// precomputed `NotInstalled` or `UpdateAvailable` plan could
/// re-create an installed target from a now-missing source (which
/// would silently recreate the missing checkout via
/// `write_target`'s parent-creation behavior).
#[test]
fn apply_safe_rejects_install_or_update_after_source_removal() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    // Hand-craft an `UpdateAvailable` item so the apply path
    // would otherwise write to the target.
    let canonical_path = paths.canonical_dir.join("scout.md");
    let target_path = paths.target_dir.join("scout.md");
    fs::remove_dir_all(&checkout).unwrap();
    let item = SyncItem {
        target: SyncTarget::OpenCode,
        filename: "scout.md".to_string(),
        status: SyncStatus::UpdateAvailable,
        canonical_hash: Some(sha256_hex(b"placeholder-canonical")),
        target_hash: Some(sha256_hex(b"placeholder-target")),
        last_installed_hash: Some(sha256_hex(b"placeholder-target")),
        canonical_path: canonical_path.clone(),
        target_path: target_path.clone(),
    };
    let apply_err = apply_safe(&paths, state, vec![item]).unwrap_err();
    assert!(
        apply_err.to_string().contains("does not exist"),
        "apply_safe with an UpdateAvailable item must fail closed after source removal, got: {apply_err}"
    );
}

/// `save_canonical` must refuse to write a new agent file when the
/// configured checkout has been removed. This is the regression
/// test for the silent checkout-recreation bug: previously,
/// `write_target`'s parent-directory creation would silently
/// recreate the missing checkout (or worse, fall back to
/// `<agenthd_root>/agents`) the moment the user clicked Save on
/// a brand-new agent. The validation gate now blocks the write
/// before any directory is created.
#[test]
fn save_canonical_does_not_recreate_missing_checkout() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    // User removes the entire checkout.
    fs::remove_dir_all(&checkout).unwrap();
    let mut agent = Agent::new_default("helper".to_string()).unwrap();
    agent.description = "Should never be written".to_string();
    agent.prompt = "Should never be written".to_string();
    let err = save_canonical(&paths, &agent, None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "save_canonical must refuse to recreate a missing checkout, got: {err}"
    );
    // Neither the configured checkout's `agents/` nor the historical
    // `<agenthd_root>/agents/` shortcut must exist.
    assert!(!paths.canonical_dir.exists());
    assert!(
        !paths.agenthd_root.join("agents").exists(),
        "<agenthd_root>/agents must never be recreated by save_canonical"
    );
}

/// `delete_canonical` must surface a missing source as an
/// explicit error rather than silently returning Ok. The
/// historical NotFound-on-file path was about race conditions on
/// the agent file itself; the validation gate handles the
/// distinct case where the configured checkout has vanished.
#[test]
fn delete_canonical_rejects_missing_source() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    fs::remove_dir_all(&checkout).unwrap();
    let err = delete_canonical(&paths, "scout").unwrap_err().to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "delete_canonical must fail closed on missing source, got: {err}"
    );
}

/// `rename_canonical` must surface a missing source as an
/// explicit error rather than silently returning Ok or
/// accidentally creating a new agent file in a freshly-created
/// checkout.
#[test]
fn rename_canonical_rejects_missing_source() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    fs::remove_dir_all(&checkout).unwrap();
    let err = rename_canonical(&paths, "scout", "renamed")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "rename_canonical must fail closed on missing source, got: {err}"
    );
}

/// `force_install` must surface a missing source as an explicit
/// error. The function reads from canonical and writes into the
/// target — without the validation gate, a missing source would
/// return NotFound on the canonical read and the call would
/// error confusingly from inside `target_bytes` rather than from
/// the source check.
#[test]
fn force_install_rejects_missing_source() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_checkout(&dir);
    fs::remove_dir_all(&checkout).unwrap();
    let err = force_install(&paths, State::default(), SyncTarget::OpenCode, "scout.md")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist") || err.contains("`agents/`"),
        "force_install must fail closed on missing source, got: {err}"
    );
}

/// A user can still create a new agent inside an existing empty
/// checkout. The validation gate accepts a present-and-empty
/// `agents/` directory; the historical local-default shortcut is
/// not recreated by this save.
#[test]
fn save_canonical_into_empty_but_existing_agents_dir_succeeds() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("empty-checkout");
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
    let mut agent = Agent::new_default("helper".to_string()).unwrap();
    agent.description = "A new helper agent".to_string();
    agent.prompt = "Do helpful things.".to_string();
    save_canonical(&paths, &agent, None).unwrap();
    assert!(paths.canonical_dir.join("helper.md").exists());
    assert!(
        !paths.agenthd_root.join("agents").join("helper.md").exists(),
        "save_canonical into the configured checkout must not touch <agenthd_root>/agents"
    );
}

// ---------- Safety tests: apply_safe per-item revalidation ----------
//
// The tests below pin the fail-closed per-item revalidation
// contract `apply_safe` enforces against a stale `SyncItem`
// snapshot. Each test hand-crafts a `SyncItem` whose plan-time
// observations no longer match the filesystem (source/target
// changed, vanished, reappeared, or replaced with a symlink) and
// asserts that `apply_safe` honors the live filesystem rather than
// the snapshot.
//
// Helper: build a SyncItem with all snapshot fields explicit so a
// test can stage any combination of plan-vs-truth.

fn make_sync_item(
    paths: &Paths,
    target: SyncTarget,
    filename: &str,
    status: SyncStatus,
    canonical_hash: Option<String>,
    target_hash: Option<String>,
    last_installed_hash: Option<String>,
) -> SyncItem {
    let target_path = match target {
        SyncTarget::OpenCode => paths.target_dir.join(filename),
        SyncTarget::Pi => paths.pi_target_dir.join(filename),
    };
    SyncItem {
        target,
        filename: filename.to_string(),
        status,
        canonical_hash,
        target_hash,
        last_installed_hash,
        canonical_path: paths.canonical_dir.join(filename),
        target_path,
    }
}

/// Stale UpdateAvailable snapshot: the target was externally
/// modified between plan and apply (its byte hash no longer matches
/// `last_installed_hash`). `apply_safe` MUST skip the update and
/// preserve both the existing target bytes and the manifest entry.
#[test]
fn apply_safe_skips_update_when_target_changed_externally() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    // Mutate the target behind apply_safe's back.
    let target_path = paths.target_dir.join("scout.md");
    let original = fs::read_to_string(&target_path).unwrap();
    let tampered = original.replace("read-only", "TAMPERED");
    fs::write(&target_path, &tampered).unwrap();
    let tampered_hash = sha256_hex(tampered.as_bytes());

    // Hand-craft an UpdateAvailable plan that still records the
    // *pre-mutation* hashes — modeling the bug where apply_safe
    // trusts the snapshot without re-hashing the target.
    let canonical_path = paths.canonical_dir.join("scout.md");
    let canonical_bytes = fs::read(&canonical_path).unwrap();
    let canonical_hash = sha256_hex(&canonical_bytes);
    let owned_hash = sha256_hex(original.as_bytes());
    let item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::UpdateAvailable,
        Some(canonical_hash.clone()),
        Some(owned_hash.clone()),
        Some(owned_hash.clone()),
    );
    let (state, outcomes) = apply_safe(&paths, state, vec![item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    // Target bytes are untouched.
    assert_eq!(fs::read_to_string(&target_path).unwrap(), tampered);
    // Manifest is preserved.
    assert_eq!(state.installed.get("scout.md"), Some(&owned_hash));
    let _ = tampered_hash;
}

/// Stale Remove snapshot: the canonical reappears between plan and
/// apply. The Remove MUST NOT delete the target and MUST NOT drop
/// the ownership entry — both will be re-evaluated by the next
/// plan.
#[test]
fn apply_safe_skips_remove_when_canonical_reappears() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    // Delete the canonical file so a fresh plan would classify
    // scout.md as Remove.
    let canonical_path = paths.canonical_dir.join("scout.md");
    fs::remove_file(&canonical_path).unwrap();
    let plan = compute_plan(&paths, &state).unwrap();
    let remove_item = plan
        .iter()
        .find(|i| i.filename == "scout.md")
        .unwrap()
        .clone();
    assert_eq!(remove_item.status, SyncStatus::Remove);

    // Canonical comes back before apply_safe runs.
    let new_canonical = starter_agent(&STARTERS[0]).render();
    fs::write(&canonical_path, &new_canonical).unwrap();
    let owned_hash = state.installed.get("scout.md").cloned().unwrap();
    let target_path = paths.target_dir.join("scout.md");
    let target_bytes = fs::read(&target_path).unwrap();
    let _target_hash = sha256_hex(&target_bytes);

    // The plan's Remove snapshot still claims canonical_hash = None;
    // live filesystem disagrees.
    let (state, outcomes) = apply_safe(&paths, state, vec![remove_item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    assert!(outcome.detail.contains("reappeared"));
    // Target is still on disk.
    assert!(target_path.exists());
    assert_eq!(fs::read(&target_path).unwrap(), target_bytes);
    // Ownership entry is preserved.
    assert_eq!(state.installed.get("scout.md"), Some(&owned_hash));
}

/// Stale NotInstalled snapshot: the target appeared after plan
/// (e.g. the user dropped a file there). `apply_safe` MUST NOT
/// overwrite it. The manifest entry stays empty (NotInstalled
/// recorded no owner) so the next plan will surface the situation
/// as either UpToDate, Conflict, or Unowned.
#[test]
fn apply_safe_skips_install_when_target_appeared_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let scout_item = plan
        .iter()
        .find(|i| i.filename == "scout.md" && i.target == SyncTarget::OpenCode)
        .cloned()
        .unwrap();
    assert_eq!(scout_item.status, SyncStatus::NotInstalled);

    // The user drops a hand-written file into the OpenCode target
    // before apply runs.
    let target_path = scout_item.target_path.clone();
    let user_bytes = b"user-owned-content\n";
    fs::write(&target_path, user_bytes).unwrap();
    let user_hash = sha256_hex(user_bytes);

    let (state, outcomes) = apply_safe(&paths, state, vec![scout_item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    assert!(outcome.detail.contains("appeared after plan"));
    // User file is untouched.
    assert_eq!(fs::read(&target_path).unwrap(), user_bytes);
    // Manifest stays empty for this row.
    assert!(!state.installed.contains_key("scout.md"));
    let _ = user_hash;
}

/// Source disappears after plan: `apply_safe` must surface a
/// non-panic error for the affected item and CONTINUE processing
/// other items. Before this revalidation, a `NotInstalled` /
/// `UpdateAvailable` plan whose canonical source vanished between
/// plan and apply would `.expect()` and panic, killing the whole
/// sync batch.
#[test]
fn apply_safe_continues_after_source_disappears_no_panic() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let _plan = compute_plan(&paths, &state).unwrap();

    // Remove two canonicals after plan but before apply.
    fs::remove_file(paths.canonical_dir.join("scout.md")).unwrap();
    fs::remove_file(paths.canonical_dir.join("reviewer.md")).unwrap();
    // Rebuild a plan-like SyncItem list using the pre-removal
    // hashes by re-hashing the now-missing files would fail, so we
    // use the canonical contents we already wrote to disk during
    // `setup_paths_with_checkout` — they are still available via
    // the starter fixtures.
    let scout_bytes = starter_agent(&STARTERS[0]).render().into_bytes();
    let reviewer_bytes = starter_agent(&STARTERS[1]).render().into_bytes();
    let scout_hash = sha256_hex(&scout_bytes);
    let reviewer_hash = sha256_hex(&reviewer_bytes);
    let scout_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::NotInstalled,
        Some(scout_hash),
        None,
        None,
    );
    let reviewer_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "reviewer.md",
        SyncStatus::NotInstalled,
        Some(reviewer_hash),
        None,
        None,
    );
    // A third item whose canonical is still present — must succeed
    // even though its siblings failed.
    let worker_bytes = starter_agent(&STARTERS[2]).render().into_bytes();
    let worker_hash = sha256_hex(&worker_bytes);
    let worker_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "worker.md",
        SyncStatus::NotInstalled,
        Some(worker_hash),
        None,
        None,
    );

    let (state, outcomes) =
        apply_safe(&paths, state, vec![scout_item, reviewer_item, worker_item]).unwrap();

    let scout_outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(scout_outcome.action, "error");
    assert!(!scout_outcome.ok);
    assert!(scout_outcome.detail.contains("disappeared after plan"));
    let reviewer_outcome = outcomes
        .iter()
        .find(|o| o.filename == "reviewer.md")
        .unwrap();
    assert_eq!(reviewer_outcome.action, "error");
    let worker_outcome = outcomes.iter().find(|o| o.filename == "worker.md").unwrap();
    assert_eq!(worker_outcome.action, "installed");
    assert!(worker_outcome.ok);
    // Worker is actually installed.
    assert!(paths.target_dir.join("worker.md").exists());
    // Scout and reviewer targets stay absent.
    assert!(!paths.target_dir.join("scout.md").exists());
    assert!(!paths.target_dir.join("reviewer.md").exists());
    // Only the successful row is recorded in the manifest.
    assert!(state.installed.contains_key("worker.md"));
    assert!(!state.installed.contains_key("scout.md"));
    assert!(!state.installed.contains_key("reviewer.md"));
}

/// Source changes after plan: `apply_safe` must surface a
/// non-panic error and refuse to write the now-stale snapshot
/// bytes. The existing manifest entry (if any) is preserved.
#[test]
fn apply_safe_skips_install_when_source_changed_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    let scout_canonical = paths.canonical_dir.join("scout.md");
    let prior = hash_file(&scout_canonical).unwrap();
    let mut agent = Agent::read(&scout_canonical).unwrap();
    agent.prompt.push_str("\nUpdated.");
    save_canonical(&paths, &agent, prior.as_deref()).unwrap();

    // Build a NotInstalled-ish plan item that records the OLD
    // canonical hash from before the save — modeling the bug
    // where the planner snapshot is no longer accurate.
    let old_hash = prior.unwrap();
    let target_path = paths.target_dir.join("scout.md");
    let target_hash_before = hash_file(&target_path).unwrap();
    let item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::UpdateAvailable,
        Some(old_hash.clone()),
        target_hash_before.clone(),
        target_hash_before,
    );

    let (state, outcomes) = apply_safe(&paths, state, vec![item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    assert!(outcome.detail.contains("changed since plan"));
    // Target is untouched (still has the original install bytes).
    let on_disk = fs::read_to_string(&target_path).unwrap();
    assert!(on_disk.contains("read-only codebase scout"));
    assert!(!on_disk.contains("Updated."));
    // Manifest preserves the original install hash.
    assert_eq!(state.installed.get("scout.md"), Some(&old_hash));
}

/// Per-target state isolation: a failed Pi row must not pollute the
/// OpenCode ownership map and vice versa. The cleanup pass at the
/// end of `apply_safe` must only touch the targets the caller
/// passed in.
#[test]
fn apply_safe_isolates_failed_rows_per_target() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();
    let opencode_owned: BTreeMap<String, String> = state.installed.clone();
    let pi_owned: BTreeMap<String, String> = state.pi_installed.clone();

    // Force a NotInstalled snapshot for Pi that targets a
    // canonical we then delete. apply_safe must record an error
    // outcome for the Pi row, leave Pi's ownership map alone, and
    // leave OpenCode's ownership map completely untouched.
    let pi_canonical = paths.canonical_dir.join("scout.md");
    let pi_bytes = starter_agent(&STARTERS[0]).render().into_bytes();
    let pi_canonical_hash = sha256_hex(&pi_bytes);
    fs::remove_file(&pi_canonical).unwrap();
    let pi_item = make_sync_item(
        &paths,
        SyncTarget::Pi,
        "scout.md",
        SyncStatus::NotInstalled,
        Some(pi_canonical_hash),
        None,
        None,
    );
    let (state, outcomes) = apply_safe(&paths, state, vec![pi_item]).unwrap();
    let pi_outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(pi_outcome.action, "error");
    // OpenCode ownership map is byte-for-byte unchanged.
    assert_eq!(state.installed, opencode_owned);
    // Pi ownership map is unchanged (entry was never recorded).
    assert_eq!(state.pi_installed, pi_owned);
}

/// Successes continue after a failed row: a single failed
/// `NotInstalled` row must not prevent subsequent successful rows
/// from being applied and recorded in the manifest.
#[test]
fn apply_safe_continues_successes_after_failed_row() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let _plan = compute_plan(&paths, &state).unwrap();

    // Build a batch: scout (will be poisoned by removing its
    // canonical after planning), reviewer (will succeed), worker
    // (will succeed).
    fs::remove_file(paths.canonical_dir.join("scout.md")).unwrap();
    let scout_bytes = starter_agent(&STARTERS[0]).render().into_bytes();
    let reviewer_bytes = starter_agent(&STARTERS[1]).render().into_bytes();
    let worker_bytes = starter_agent(&STARTERS[2]).render().into_bytes();
    let scout_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::NotInstalled,
        Some(sha256_hex(&scout_bytes)),
        None,
        None,
    );
    let reviewer_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "reviewer.md",
        SyncStatus::NotInstalled,
        Some(sha256_hex(&reviewer_bytes)),
        None,
        None,
    );
    let worker_item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "worker.md",
        SyncStatus::NotInstalled,
        Some(sha256_hex(&worker_bytes)),
        None,
        None,
    );
    let (state, outcomes) =
        apply_safe(&paths, state, vec![scout_item, reviewer_item, worker_item]).unwrap();

    let scout_outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(scout_outcome.action, "error");
    let reviewer_outcome = outcomes
        .iter()
        .find(|o| o.filename == "reviewer.md")
        .unwrap();
    assert_eq!(reviewer_outcome.action, "installed");
    assert!(reviewer_outcome.ok);
    let worker_outcome = outcomes.iter().find(|o| o.filename == "worker.md").unwrap();
    assert_eq!(worker_outcome.action, "installed");
    assert!(worker_outcome.ok);
    // Reviewer and worker are actually installed.
    assert!(paths.target_dir.join("reviewer.md").exists());
    assert!(paths.target_dir.join("worker.md").exists());
    assert!(!paths.target_dir.join("scout.md").exists());
    // Manifest records only the successful rows.
    assert!(state.installed.contains_key("reviewer.md"));
    assert!(state.installed.contains_key("worker.md"));
    assert!(!state.installed.contains_key("scout.md"));
}

/// Stale UpdateAvailable regression: the plan claimed an update was
/// available, but between plan and apply the target was externally
/// edited AND the manifest still records the old owned hash.
/// `apply_safe` must NOT silently become an install (the user's
/// ownership hash no longer matches the bytes on disk — this is
/// what `UpdateAvailable` was supposed to catch as `Conflict`).
/// Adding the regression locks the documented behavior so a future
/// "lenient overwrite" change has to update the test on purpose.
#[test]
fn apply_safe_rejects_stale_update_available_on_changed_target() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    let target_path = paths.target_dir.join("scout.md");
    let original = fs::read_to_string(&target_path).unwrap();
    let owned_hash = state.installed.get("scout.md").cloned().unwrap();

    // Hand-craft an UpdateAvailable row that still records the
    // pre-edit hashes — the plan-time classification was correct
    // when it ran, but the user has since edited the target.
    let canonical_path = paths.canonical_dir.join("scout.md");
    let canonical_bytes = fs::read(&canonical_path).unwrap();
    let canonical_hash = sha256_hex(&canonical_bytes);
    let mut item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::UpdateAvailable,
        Some(canonical_hash.clone()),
        Some(owned_hash.clone()),
        Some(owned_hash.clone()),
    );

    // Now do the actual external edit between plan and apply.
    let tampered = original.replace("read-only", "TAMPERED");
    fs::write(&target_path, &tampered).unwrap();

    // Confirm the live target hash no longer matches the plan's
    // target_hash/last_installed_hash. Either mismatch trips the
    // skip; the spec wants errors here, not silent overwrites.
    item.target_hash = Some(sha256_hex(owned_hash.as_bytes())); // pre-edit hash
    item.last_installed_hash = Some(owned_hash.clone());

    let (state, outcomes) = apply_safe(&paths, state, vec![item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    // Target bytes are untouched (the "no silent overwrite"
    // contract).
    assert_eq!(fs::read_to_string(&target_path).unwrap(), tampered);
    // Manifest preserves the prior install hash — we did not
    // re-insert anything for a row we did not actually update.
    assert_eq!(state.installed.get("scout.md"), Some(&owned_hash));
}

/// Symlinked source between plan and apply: `apply_safe` must fail
/// the per-item revalidation on the symlink check (canonical must
/// be a regular file), keep the existing ownership entry intact,
/// and continue with the rest of the batch.
#[test]
fn apply_safe_skips_when_canonical_is_symlink_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (mut state, _) = apply_safe(&paths, state, plan).unwrap();

    // Replace the canonical scout.md with a symlink pointing at
    // some unrelated content. Plan said `UpToDate`; live is now a
    // symlink (not a regular file). The revalidation must detect
    // this and refuse to adopt.
    let canonical_path = paths.canonical_dir.join("scout.md");
    let original_canonical = fs::read(&canonical_path).unwrap();
    fs::remove_file(&canonical_path).unwrap();
    let real_target = dir.path().join("real-canonical.md");
    fs::write(&real_target, b"symlink-target\n").unwrap();
    #[cfg(unix)]
    let made_link = std::os::unix::fs::symlink(&real_target, &canonical_path).is_ok();
    #[cfg(windows)]
    let made_link = std::os::windows::fs::symlink_file(&real_target, &canonical_path).is_ok();
    if !made_link {
        // Symlink creation can fail in sandboxed CI environments
        // or on Windows without SeCreateSymbolicLinkPrivilege.
        return;
    }

    let canonical_hash = sha256_hex(&original_canonical);
    let item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::UpToDate,
        Some(canonical_hash.clone()),
        Some(canonical_hash.clone()),
        Some(canonical_hash.clone()),
    );
    let owned_hash_before = state.installed.get("scout.md").cloned();

    let (state, outcomes) = apply_safe(&paths, state, vec![item]).unwrap();
    let outcome = &outcomes[0];
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    // Truthful message: the source is still on disk as a symlink,
    // so it did not "disappear"; surface the real reason.
    assert!(outcome.detail.contains("not a regular file"));
    assert!(!outcome.detail.contains("disappeared"));
    // Manifest is preserved.
    assert_eq!(state.installed.get("scout.md"), owned_hash_before.as_ref());
    // Restore the file so cleanup doesn't see a stale symlink.
    fs::remove_file(&canonical_path).unwrap();
    fs::write(&canonical_path, &original_canonical).unwrap();
}

/// Symlinked target between plan and apply: a symlink at the
/// target path is not a regular file and must never be
/// overwritten or deleted by `apply_safe`.
#[test]
fn apply_safe_skips_when_target_is_symlink_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_checkout(&dir);
    let state = State::default();
    let plan = compute_plan(&paths, &state).unwrap();
    let (state, _) = apply_safe(&paths, state, plan).unwrap();

    let target_path = paths.target_dir.join("scout.md");
    let real_target = dir.path().join("real-target.md");
    fs::write(&real_target, b"target-backing-content\n").unwrap();
    fs::remove_file(&target_path).unwrap();
    #[cfg(unix)]
    let made_link = std::os::unix::fs::symlink(&real_target, &target_path).is_ok();
    #[cfg(windows)]
    let made_link = std::os::windows::fs::symlink_file(&real_target, &target_path).is_ok();
    if !made_link {
        return;
    }

    let canonical_path = paths.canonical_dir.join("scout.md");
    let canonical_bytes = fs::read(&canonical_path).unwrap();
    let canonical_hash = sha256_hex(&canonical_bytes);
    // Force an UpdateAvailable by changing the canonical bytes
    // (we don't actually re-plan here — the stale plan still
    // thinks the file is UpToDate at the manifest).
    let item = make_sync_item(
        &paths,
        SyncTarget::OpenCode,
        "scout.md",
        SyncStatus::UpToDate,
        Some(canonical_hash.clone()),
        Some(canonical_hash.clone()),
        Some(canonical_hash.clone()),
    );

    let owned_hash_before = state.installed.get("scout.md").cloned();
    let (state, outcomes) = apply_safe(&paths, state, vec![item]).unwrap();
    let outcome = outcomes.iter().find(|o| o.filename == "scout.md").unwrap();
    assert_eq!(outcome.action, "error");
    assert!(!outcome.ok);
    assert!(outcome.detail.contains("not a regular file"));
    // Symlink is left in place; the backing file is untouched.
    assert!(fs::symlink_metadata(&target_path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&real_target).unwrap(), b"target-backing-content\n");
    // Manifest is preserved.
    assert_eq!(state.installed.get("scout.md"), owned_hash_before.as_ref());
}
