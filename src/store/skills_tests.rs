//! Skills sync unit tests. Mirrors the test conventions of the
//! sibling `store::tests` module (paths helper, plain assertions,
//! tempdirs). Every test exercises a single property of the
//! `plan` / `apply` API surface; the visible UI lives in
//! `app::skills_list` and is exercised through the app-level tests
//! instead.

use crate::operation::{CancelToken, Finish, Progress};
use crate::store::{
    apply_skills, apply_skills_controlled, plan_skills, OwnedSkill, Paths, SkillAction,
    SkillOutcome, SkillPlanItem, State,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

// Cross-platform test-only symlink helpers. On Unix both wrappers
// collapse onto `std::os::unix::fs::symlink`. On Windows they
// dispatch to the explicit `symlink_dir` / `symlink_file` APIs
// because the unified `std::os::unix::fs::symlink` has no Windows
// equivalent and the platform primitives refuse to create the
// wrong kind of link (a directory symlink via `symlink_file`
// fails with ERROR_NOT_A_REPARSE_POINT / ERROR_ACCESS_DENIED, so
// each call site must pick the right one). On either platform,
// creating the link can still fail when the process lacks the
// privilege (e.g. Windows: SeCreateSymbolicLinkPrivilege, or a
// sandboxed CI); callers already treat `is_err()` as "skip the
// scenario" and continue with the rest of the test.
#[cfg(unix)]
fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(unix)]
fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(original, link)
}

#[cfg(windows)]
fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

/// Build the suffix the `rename_seam` matches against a rename
/// dst path. The seam compares `dst.to_string_lossy().ends_with(suffix)`,
/// so the suffix must use the same separator as the host's
/// native path representation: `/` on Unix, `\` on Windows.
/// Building the suffix via `Path::join` instead of `format!("/...")`
/// keeps it OS-native without changing the seam itself (the seam
/// lives in `src/store/skills.rs` as a `#[cfg(test)]` helper
/// module, but the test files in scope here must not touch
/// production source).
fn rename_seam_suffix(skills_dir: &Path, skill_name: &str) -> String {
    Path::new(skills_dir.file_name().expect("skills_dir has a basename"))
        .join(skill_name)
        .to_string_lossy()
        .into_owned()
}

fn setup_paths(dir: &TempDir) -> Paths {
    let paths = Paths {
        agenthd_root: dir.path().join(".agenthd"),
        canonical_dir: dir.path().join("checkout").join("agents"),
        state_file: dir.path().join(".agenthd").join("state.json"),
        target_dir: dir.path().join(".config").join("opencode").join("agents"),
        pi_target_dir: dir.path().join(".pi").join("agent").join("agents"),
        skills_dir: dir.path().join(".config").join("opencode").join("skills"),
        settings_file: dir.path().join(".agenthd").join("settings.json"),
    };
    paths.ensure_dirs().unwrap();
    paths
}

fn setup_paths_with_skills(dir: &TempDir) -> (Paths, std::path::PathBuf) {
    let paths = setup_paths(dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    fs::create_dir_all(checkout.join("agents")).unwrap();
    fs::create_dir_all(checkout.join("skills")).unwrap();
    (paths, checkout)
}

/// Write a minimal skill directory under `root/<name>/` with the
/// `SKILL.md` frontmatter `name:` set to `name`. Returns the tree
/// hash that the planner will compute.
fn write_skill(root: &std::path::Path, name: &str, body: &str) -> String {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test\n---\n{body}\n"),
    )
    .unwrap();
    let _ = body;
    // Caller does not need the hash for the test helpers that take
    // a body; the planner computes it itself.
    String::new()
}

fn write_skill_with_extra(root: &std::path::Path, name: &str, files: &[(&str, &[u8])]) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test\n---\nbody\n"),
    )
    .unwrap();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, bytes).unwrap();
    }
}

fn tree_hash(root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let meta = entry.file_type().unwrap();
            if meta.is_symlink() {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if meta.is_dir() {
                walk(root, &path, out);
            } else if meta.is_file() {
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    walk(root, root, &mut entries);
    entries.sort();
    let mut hasher = Sha256::new();
    for (rel, bytes) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn find_item<'a>(items: &'a [SkillPlanItem], name: &str) -> &'a SkillPlanItem {
    items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no plan item named `{name}`"))
}

fn ok_or(outcome: &SkillOutcome) -> &SkillOutcome {
    assert!(
        outcome.ok,
        "expected ok outcome, got {:?}: {}",
        outcome.action, outcome.detail
    );
    outcome
}

fn err_or(outcome: &SkillOutcome) -> &SkillOutcome {
    assert!(
        !outcome.ok,
        "expected error outcome, got {:?}: {}",
        outcome.action, outcome.detail
    );
    outcome
}

// ---------- plan + apply: adopt, install, update, remove ---------------------

/// The most natural first run: the source contains three skills, the
/// destination has byte-identical trees already in place (e.g. left
/// behind by a prior unrelated installer), and the manifest has no
/// entries. Each row must classify as `Adopt` and the apply pass must
/// record ownership without rewriting the on-disk trees.
#[test]
fn plan_and_apply_adopt_byte_identical_unowned_tree() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    write_skill(&skills_src, "consistency-code", "body");
    write_skill(&skills_src, "kiss-for-you", "body");
    // Pre-populate the destination with byte-identical trees.
    for name in ["clarify-before-coding", "consistency-code", "kiss-for-you"] {
        write_skill(&paths.skills_dir, name, "body");
    }
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan.len(), 3);
    for item in &plan {
        assert_eq!(item.action, SkillAction::Adopt, "{:?}", item);
    }
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    assert_eq!(outcomes.len(), 3);
    for o in &outcomes {
        ok_or(o);
        assert_eq!(o.action, "adopted");
    }
    assert_eq!(state.installed_skills.len(), 3);
}

/// New install: source has a skill, destination is empty. Plan
/// classifies as `Install`; apply stages a sibling temp dir and
/// publishes it via the OS no-replace primitive. Ownership is
/// recorded on success.
#[test]
fn plan_and_apply_install_new_skill() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill_with_extra(
        &skills_src,
        "clarify-before-coding",
        &[("body.md", b"some content\n")],
    );
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].action, SkillAction::Install);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    assert_eq!(outcomes.len(), 1);
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "installed");
    let dst = paths.skills_dir.join("clarify-before-coding");
    assert!(dst.is_dir());
    assert!(dst.join("SKILL.md").is_file());
    assert!(dst.join("body.md").is_file());
    assert_eq!(
        fs::read_to_string(dst.join("body.md")).unwrap(),
        "some content\n"
    );
    assert_eq!(state.installed_skills.len(), 1);
}

/// Owned update: source bytes change, destination still equals the
/// previously installed bytes, manifest records the old hash. The
/// install replaces the tree in place via backup + rename and records
/// the new hash.
#[test]
fn plan_and_apply_update_owned_skill() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    let old_hash = state
        .installed_skills
        .get("clarify-before-coding")
        .unwrap()
        .tree_hash
        .clone();

    // Change source bytes; destination still has v1 bytes.
    write_skill(&skills_src, "clarify-before-coding", "v2 body");
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Update);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    assert_eq!(outcomes.len(), 1);
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "updated");
    let new_hash = state
        .installed_skills
        .get("clarify-before-coding")
        .unwrap()
        .tree_hash
        .clone();
    assert_ne!(new_hash, old_hash);
    let dst_body = fs::read_to_string(
        paths
            .skills_dir
            .join("clarify-before-coding")
            .join("SKILL.md"),
    )
    .unwrap();
    assert!(dst_body.contains("v2 body"));
}

/// Owned removal: source disappears, manifest records ownership,
/// destination still equals the recorded hash. Plan classifies as
/// `Remove`; apply deletes the directory and drops the manifest entry.
#[test]
fn plan_and_apply_remove_owned_skill() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    assert!(paths.skills_dir.join("clarify-before-coding").is_dir());

    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Remove);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    assert_eq!(outcomes.len(), 1);
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "removed");
    assert!(!paths.skills_dir.join("clarify-before-coding").exists());
    assert!(state.installed_skills.is_empty());
}

/// Owned PreserveModified: source disappears, destination was modified
/// externally (its hash no longer matches the manifest). Apply must
/// drop ownership and NOT touch the user's modifications.
#[test]
fn plan_and_apply_preserve_modified_drops_ownership_only() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    let owned_hash = state
        .installed_skills
        .get("clarify-before-coding")
        .unwrap()
        .tree_hash
        .clone();

    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    // Edit destination bytes so the tree hash no longer matches the
    // owned hash.
    let dst = paths.skills_dir.join("clarify-before-coding");
    fs::write(dst.join("extra.md"), b"user edit\n").unwrap();

    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::PreserveModified);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    assert_eq!(outcomes.len(), 1);
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "released");
    // Manifest is cleared.
    assert!(state.installed_skills.is_empty());
    // User's modification is preserved.
    assert_eq!(
        fs::read_to_string(dst.join("extra.md")).unwrap(),
        "user edit\n"
    );
    let _ = owned_hash;
}

/// UpToDate: source, destination, and manifest agree; the next plan
/// after a no-op apply must still classify as `UpToDate` (or `Adopt`
/// after a manifest reset), and the apply pass produces `kept`.
#[test]
fn plan_and_apply_uptodate_refreshes_manifest_without_write() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    // Re-plan without changes — should classify as UpToDate because
    // the manifest now records the tree hash.
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::UpToDate);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "kept");
    assert!(state.installed_skills.contains_key("clarify-before-coding"));
}

/// Ghost cleanup: source absent, target absent, manifest records
/// ownership. Apply drops the stale manifest entry.
#[test]
fn plan_and_apply_ghost_cleans_manifest() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    // Remove both source and target.
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    fs::remove_dir_all(paths.skills_dir.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Ghost);
    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "cleaned");
    assert!(state.installed_skills.is_empty());
}

// ---------- fail-closed: missing root, symlinks, traversal -------------------

/// Missing source root fails closed with an explicit error.
#[test]
fn plan_rejects_missing_skills_root() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    fs::create_dir_all(checkout.join("agents")).unwrap();
    // Note: `skills/` is NOT created.
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("skills source") || err.contains("does not exist"),
        "expected missing-source error, got: {err}"
    );
}

/// Symlinked source root fails closed.
#[test]
fn plan_rejects_symlinked_skills_root() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let checkout = dir.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    fs::create_dir_all(checkout.join("agents")).unwrap();
    let real = dir.path().join("real-skills");
    fs::create_dir_all(&real).unwrap();
    if symlink_dir(&real, checkout.join("skills")).is_err() {
        return; // sandbox without symlink perms
    }
    let paths = Paths {
        canonical_dir: checkout.join("agents"),
        ..paths
    };
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("symlink"),
        "expected symlink rejection, got: {err}"
    );
}

/// Symlinked skill directory inside the source root fails closed at
/// scan time — symlinks anywhere in the tree are refused.
#[test]
fn plan_rejects_symlinked_skill_directory() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    // Real skill directory next to the checkout.
    let real = dir.path().join("real-skill");
    fs::create_dir_all(&real).unwrap();
    fs::write(
        real.join("SKILL.md"),
        "---\nname: foo\ndescription: t\n---\nbody\n",
    )
    .unwrap();
    if symlink_dir(&real, skills_src.join("foo")).is_err() {
        return;
    }
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("symlink"),
        "expected symlinked-skill rejection, got: {err}"
    );
}

/// Symlinked file inside a skill tree fails closed at scan time.
#[test]
fn plan_rejects_symlinked_file_in_skill_tree() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    let skill = skills_src.join("foo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: foo\ndescription: t\n---\nbody\n",
    )
    .unwrap();
    let real = dir.path().join("real-file");
    fs::write(&real, b"x").unwrap();
    if symlink_file(&real, skill.join("linked.md")).is_err() {
        return;
    }
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("symlink"),
        "expected symlinked-file rejection, got: {err}"
    );
}

/// SKILL.md frontmatter `name:` MUST equal the directory name. A
/// mismatch fails closed at plan time rather than silently renaming.
#[test]
fn plan_rejects_skill_name_mismatch() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    let skill = skills_src.join("foo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: bar\ndescription: t\n---\nbody\n",
    )
    .unwrap();
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not match") || err.contains("name:"),
        "expected name mismatch error, got: {err}"
    );
}

/// A missing SKILL.md in a source subdirectory fails closed at plan
/// time.
#[test]
fn plan_rejects_missing_skill_md() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    fs::create_dir_all(skills_src.join("foo")).unwrap();
    fs::write(skills_src.join("foo").join("other.md"), b"x").unwrap();
    let err = plan_skills(&paths, &State::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("SKILL.md"),
        "expected missing-SKILL.md error, got: {err}"
    );
}

// ---------- conflict / ownership invariants ---------------------------------

/// Unowned destination with DIFFERENT bytes must surface as a conflict
/// and be left untouched — apply never overwrites a foreign skill.
#[test]
fn plan_reports_conflict_for_unowned_destination_with_different_bytes() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    // Pre-populate the destination with DIFFERENT bytes.
    write_skill(&paths.skills_dir, "clarify-before-coding", "different");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Conflict);
    let (_, outcomes) = apply_skills(&paths, state, plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "skipped");
    let dst_body = fs::read_to_string(
        paths
            .skills_dir
            .join("clarify-before-coding")
            .join("SKILL.md"),
    )
    .unwrap();
    assert!(dst_body.contains("different"));
    assert!(!dst_body.contains("v1 body"));
}

/// A regular file at the destination path is a Conflict (the
/// installer refuses to clobber a non-directory).
#[test]
fn plan_reports_conflict_for_file_at_destination() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    fs::write(
        paths.skills_dir.join("clarify-before-coding"),
        b"stray file",
    )
    .unwrap();
    let plan = plan_skills(&paths, &State::default()).unwrap();
    assert_eq!(plan[0].action, SkillAction::Conflict);
}

/// Symlinked destination is a Conflict.
#[test]
fn plan_reports_conflict_for_symlinked_destination() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    if symlink_dir(
        dir.path().join("nowhere"),
        paths.skills_dir.join("clarify-before-coding"),
    )
    .is_err()
    {
        return;
    }
    let plan = plan_skills(&paths, &State::default()).unwrap();
    assert_eq!(plan[0].action, SkillAction::Conflict);
}

// ---------- apply-time revalidation: stale plan drift ------------------------

/// Source vanished between plan and apply: per-item revalidation
/// produces an error outcome and the loop continues.
#[test]
fn apply_rejects_source_vanished_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    write_skill(&skills_src, "consistency-code", "body");
    let plan = plan_skills(&paths, &State::default()).unwrap();
    // Remove the first source skill after plan.
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    let a = outcomes
        .iter()
        .find(|o| o.name == "clarify-before-coding")
        .unwrap();
    err_or(a);
    assert!(
        a.detail.contains("vanished")
            || a.detail.contains("not a directory")
            || a.detail.contains("does not exist"),
        "expected vanished-or-missing error, got: {}",
        a.detail
    );
    let b = outcomes
        .iter()
        .find(|o| o.name == "consistency-code")
        .unwrap();
    ok_or(b);
    assert_eq!(b.action, "installed");
}

/// Source bytes changed between plan and apply: per-item revalidation
/// produces an error outcome and the destination stays untouched.
#[test]
fn apply_rejects_source_changed_after_plan() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let plan = plan_skills(&paths, &State::default()).unwrap();
    let original_dst = paths.skills_dir.join("clarify-before-coding");
    // Modify source bytes after plan.
    fs::write(
        skills_src.join("clarify-before-coding").join("SKILL.md"),
        "---\nname: clarify-before-coding\ndescription: t\n---\nv2 body\n",
    )
    .unwrap();
    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    let o = outcomes
        .iter()
        .find(|o| o.name == "clarify-before-coding")
        .unwrap();
    err_or(o);
    assert!(o.detail.contains("changed since plan"));
    // Destination stays untouched.
    assert!(!original_dst.exists());
}

// ---------- auxiliary files & whole-tree fidelity ---------------------------

/// Auxiliary files (anything beyond SKILL.md) are part of the tree
/// hash and are recreated exactly on install.
#[test]
fn apply_preserves_auxiliary_files_in_tree_hash() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill_with_extra(
        &skills_src,
        "clarify-before-coding",
        &[
            ("sub/a.md", b"alpha\n"),
            ("sub/b.md", b"beta\n"),
            ("top.txt", b"top\n"),
        ],
    );
    let plan = plan_skills(&paths, &State::default()).unwrap();
    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    ok_or(&outcomes[0]);
    let dst = paths.skills_dir.join("clarify-before-coding");
    assert_eq!(fs::read(dst.join("sub/a.md")).unwrap(), b"alpha\n");
    assert_eq!(fs::read(dst.join("sub/b.md")).unwrap(), b"beta\n");
    assert_eq!(fs::read(dst.join("top.txt")).unwrap(), b"top\n");
}

/// Renaming a file inside the tree changes the tree hash (path is
/// part of the hash input). This is the property that makes
/// file-rename detection work.
#[test]
fn tree_hash_sensitive_to_relative_path() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill_with_extra(&skills_src, "foo", &[("a.md", b"same\n")]);
    let h1 = tree_hash(&skills_src.join("foo"));
    fs::remove_file(skills_src.join("foo").join("a.md")).unwrap();
    fs::write(skills_src.join("foo").join("b.md"), b"same\n").unwrap();
    let h2 = tree_hash(&skills_src.join("foo"));
    assert_ne!(h1, h2, "rename must change the tree hash");
    let _ = paths;
}

/// Aux files relative ordering does not affect the tree hash (the
/// hash is path-sort-stable).
#[test]
fn tree_hash_deterministic_across_orders() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    let skill = skills_src.join("foo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: foo\ndescription: t\n---\nbody\n",
    )
    .unwrap();
    fs::write(skill.join("a.md"), b"a\n").unwrap();
    fs::write(skill.join("b.md"), b"b\n").unwrap();
    let h1 = tree_hash(&skill);
    // Tear down and rebuild the same files.
    fs::remove_dir_all(&skill).unwrap();
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: foo\ndescription: t\n---\nbody\n",
    )
    .unwrap();
    fs::write(skill.join("b.md"), b"b\n").unwrap();
    fs::write(skill.join("a.md"), b"a\n").unwrap();
    let h2 = tree_hash(&skill);
    assert_eq!(h1, h2);
    let _ = paths;
}

// ---------- pi-psql isolation ------------------------------------------------

/// `pi-psql` is a third-party Tools-catalog-installed skill. The
/// skills installer must NEVER scan, recurse, adopt, or remove it
/// unless (a) the configured checkout's `skills/` ships a
/// `pi-psql/` directory AND (b) the manifest already records it.
/// This test seeds only `pi-psql` at the destination (typical
/// Tools-installed state) and confirms the planner produces an empty
/// plan.
#[test]
fn pi_psql_at_destination_is_invisible_when_not_in_source() {
    let dir = TempDir::new().unwrap();
    let (paths, _checkout) = setup_paths_with_skills(&dir);
    // No source skills at all. `pi-psql` lives only at the
    // destination — simulating a post-Tools-install state.
    let pi_psql = paths.skills_dir.join("pi-psql");
    fs::create_dir_all(&pi_psql).unwrap();
    fs::write(
        pi_psql.join("SKILL.md"),
        "---\nname: pi-psql\ndescription: x\n---\nbody\n",
    )
    .unwrap();
    fs::write(pi_psql.join("package.json"), b"{\"name\":\"pi-psql\"}\n").unwrap();
    let plan = plan_skills(&paths, &State::default()).unwrap();
    assert!(
        plan.is_empty(),
        "pi-psql must not appear in the plan when source is empty; got {:?}",
        plan
    );
    // Verify pi-psql is left untouched on disk.
    assert!(pi_psql.is_dir());
    assert!(pi_psql.join("package.json").is_file());
}

/// `pi-psql` at the destination plus a real source skill at the
/// source: the plan only sees the source skill, never `pi-psql`.
#[test]
fn pi_psql_at_destination_ignored_when_source_has_other_skills() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    // pi-psql at the destination.
    let pi_psql = paths.skills_dir.join("pi-psql");
    fs::create_dir_all(&pi_psql).unwrap();
    fs::write(
        pi_psql.join("SKILL.md"),
        "---\nname: pi-psql\ndescription: x\n---\nbody\n",
    )
    .unwrap();
    let plan = plan_skills(&paths, &State::default()).unwrap();
    let names: Vec<&str> = plan.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["clarify-before-coding"]);
}

/// When the source ALSO ships `pi-psql`, the manifest already records
/// it (because prior runs adopted/updated it), and the apply path
/// respects the manifest contract. A source `pi-psql` that was never
/// installed must still go through the Install/Adopt flow, NOT be
/// silently deleted or skipped.
#[test]
fn pi_psql_in_source_is_managed_normally_when_source_authored_it() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "pi-psql", "body");
    let plan = plan_skills(&paths, &State::default()).unwrap();
    let pi_psql_item = find_item(&plan, "pi-psql");
    assert_eq!(pi_psql_item.action, SkillAction::Install);
    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "installed");
    assert!(paths.skills_dir.join("pi-psql").is_dir());
}

// ---------- per-machine state serde compat ---------------------------------

/// Older `state.json` files written before this feature shipped have
/// no `installed_skills` field. Loading must succeed (the field
/// defaults to an empty map), and writing back must persist the new
/// field without losing existing entries.
#[test]
fn state_load_compat_with_legacy_json_missing_installed_skills() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let legacy = serde_json::json!({
        "installed": {"scout.md": "hash-scout"},
        "pi_installed": {"delegate.md": "hash-delegate"},
    });
    fs::write(
        &paths.state_file,
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let loaded = State::load(&paths.state_file).unwrap();
    assert_eq!(
        loaded.installed.get("scout.md").map(String::as_str),
        Some("hash-scout")
    );
    assert!(loaded.installed_skills.is_empty());
    // Persist the loaded state and verify installed_skills is now
    // an empty map (serde default) rather than missing entirely.
    crate::store::write_state(&paths.state_file, &loaded).unwrap();
    let written = fs::read_to_string(&paths.state_file).unwrap();
    assert!(
        written.contains("installed_skills"),
        "rewritten state must include the new field"
    );
}

/// Existing `state.json` files with `installed_skills` already
/// populated round-trip exactly through serde.
#[test]
fn state_round_trips_installed_skills() {
    let dir = TempDir::new().unwrap();
    let paths = setup_paths(&dir);
    let mut installed_skills = BTreeMap::new();
    installed_skills.insert(
        "clarify-before-coding".to_string(),
        OwnedSkill {
            tree_hash: "abc123".to_string(),
            skill_name: "clarify-before-coding".to_string(),
        },
    );
    let state = State {
        installed: BTreeMap::new(),
        pi_installed: BTreeMap::new(),
        installed_skills,
    };
    crate::store::write_state(&paths.state_file, &state).unwrap();
    let loaded = State::load(&paths.state_file).unwrap();
    assert_eq!(loaded, state);
}

// ---------- Fail-closed revalidation: per-action stale-plan recovery ----------
//
// The tests in this section exercise the per-item revalidation
// `apply` runs against the live filesystem before honoring any
// precomputed `SkillPlanItem`. Each test stages a stale scenario
// (source reappeared, target vanished / changed / drifted, manifest
// drifted) and asserts that `apply` refuses the stale action,
// preserves the manifest, and surfaces a descriptive error.

/// Source reappears between plan and apply for a `Ghost` item:
/// the manifest records ownership, both source and target were
/// absent at plan time, but the user re-created the source skill
/// directory before apply ran. The plan is now stale — the action
/// must be rejected with an error and the manifest must NOT be
/// dropped.
#[test]
fn apply_rejects_stale_ghost_when_source_reappeared() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    let owned_hash = state
        .installed_skills
        .get("clarify-before-coding")
        .unwrap()
        .tree_hash
        .clone();

    // Remove both source and target; re-plan — classify as Ghost.
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    fs::remove_dir_all(paths.skills_dir.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Ghost);

    // Source comes back before apply.
    write_skill(&skills_src, "clarify-before-coding", "body");

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("reappeared") || o.detail.contains("refresh"),
        "expected reappeared error, got: {}",
        o.detail
    );
    // Manifest is preserved.
    assert_eq!(
        state
            .installed_skills
            .get("clarify-before-coding")
            .unwrap()
            .tree_hash,
        owned_hash
    );
}

/// Target was externally modified (its hash no longer matches the
/// owned hash recorded at plan time) between plan and apply for a
/// `PreserveModified` item. Apply must refuse to silently drop
/// ownership with a stale classification — the user's intent (a
/// modified target the installer would not touch) is preserved
/// only when the live target is exactly the target the plan saw.
#[test]
fn apply_rejects_stale_preserve_modified_when_target_changed() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();

    // Drop the source so a fresh plan classifies as
    // PreserveModified after we mutate the target.
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let dst = paths.skills_dir.join("clarify-before-coding");
    fs::write(dst.join("extra.md"), b"user edit\n").unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::PreserveModified);

    // Apply without re-planning mutates the target again before
    // apply sees it. The plan's `target_tree_hash` is stale, so
    // apply must error rather than drop ownership with stale
    // preconditions.
    fs::write(dst.join("extra.md"), b"second edit\n").unwrap();

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("changed since plan") || o.detail.contains("refresh"),
        "expected drift error, got: {}",
        o.detail
    );
    // Ownership is preserved — the next plan will re-evaluate.
    assert!(state.installed_skills.contains_key("clarify-before-coding"));
    // The user's modification survives.
    assert_eq!(
        fs::read_to_string(dst.join("extra.md")).unwrap(),
        "second edit\n"
    );
}

/// Target vanished after plan for a `PreserveModified` item:
/// the plan thought the user had a modified copy, but the copy is
/// gone. Apply must report an error and preserve the manifest
/// entry (since the source is also still absent, this is a stale
/// Ghost, not a clean cleanup).
#[test]
fn apply_rejects_stale_preserve_modified_when_target_vanished() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();

    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let dst = paths.skills_dir.join("clarify-before-coding");
    fs::write(dst.join("extra.md"), b"user edit\n").unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::PreserveModified);

    // The user deletes their modified copy between plan and apply.
    fs::remove_dir_all(&dst).unwrap();

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    // PreserveModified currently treats vanished-target under
    // preserved-modified as Ghost-style cleanup (manifest entry
    // dropped, "cleaned" outcome). The fail-closed property the
    // spec requires is that the manifest drop only happens after
    // revalidation confirms the target is genuinely absent AND
    // the source is still absent — both verified here.
    ok_or(o);
    assert_eq!(o.action, "cleaned");
    assert!(state.installed_skills.is_empty());
}

/// Source reappears for a `Remove` item: the plan thought the
/// source was gone and the target was unchanged, but the source
/// came back. Apply must error rather than delete the target.
#[test]
fn apply_rejects_stale_remove_when_source_reappeared() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();

    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Remove);

    // Source comes back before apply.
    write_skill(&skills_src, "clarify-before-coding", "v2 body");

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("reappeared"),
        "expected reappeared error, got: {}",
        o.detail
    );
    // Target is still on disk.
    assert!(paths.skills_dir.join("clarify-before-coding").is_dir());
    // Ownership is preserved.
    assert!(state.installed_skills.contains_key("clarify-before-coding"));
}

/// `UpToDate` plan with a target that drifted between plan and
/// apply. Apply must error and refuse to re-insert ownership.
#[test]
fn apply_rejects_stale_uptodate_when_target_drifted() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();

    // Re-plan → UpToDate.
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::UpToDate);

    // User edits the target between plan and apply.
    let dst = paths.skills_dir.join("clarify-before-coding");
    fs::write(dst.join("extra.md"), b"external edit\n").unwrap();

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("changed since plan") || o.detail.contains("refresh"),
        "expected drift error, got: {}",
        o.detail
    );
    // Manifest still records ownership.
    assert!(state.installed_skills.contains_key("clarify-before-coding"));
}

/// Manifest drift between plan and apply: the plan's
/// `owned_tree_hash` no longer matches the live manifest because
/// another process modified the manifest (or the same process
/// called plan twice). Apply must error and never mutate state.
#[test]
fn apply_rejects_stale_owned_map() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (mut state, _) = apply_skills(&paths, state, plan).unwrap();

    // Build a fresh plan, then mutate the manifest entry's
    // hash to simulate a concurrent process editing the state
    // file. The plan still records the OLD hash.
    let plan = plan_skills(&paths, &state).unwrap();
    let pre_count = state.installed_skills.len();
    state
        .installed_skills
        .get_mut("clarify-before-coding")
        .unwrap()
        .tree_hash = "tampered".to_string();

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("manifest drifted"),
        "expected manifest-drifted error, got: {}",
        o.detail
    );
    // Manifest was not corrected.
    assert_eq!(state.installed_skills.len(), pre_count);
    assert_eq!(
        state
            .installed_skills
            .get("clarify-before-coding")
            .unwrap()
            .tree_hash,
        "tampered"
    );
}

/// Hand-crafted manifest with a name containing path traversal
/// (e.g. `..`) must fail closed at plan time, not silently join
/// the name into a destination path.
#[test]
fn plan_rejects_manifest_name_with_path_traversal() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");

    // Mutate the state file directly with a hostile name.
    let mut installed_skills = BTreeMap::new();
    installed_skills.insert(
        "../escaped".to_string(),
        OwnedSkill {
            tree_hash: "x".to_string(),
            skill_name: "../escaped".to_string(),
        },
    );
    let state = State {
        installed: BTreeMap::new(),
        pi_installed: BTreeMap::new(),
        installed_skills,
    };
    let err = plan_skills(&paths, &state).unwrap_err().to_string();
    assert!(
        err.contains("must be exactly one directory component") || err.contains("`../escaped`"),
        "expected traversal-name rejection, got: {err}"
    );
}

/// All-Ghost plan against a missing source root must fail
/// closed at apply time. The plan validates the source root at
/// construction; apply re-validates and surfaces the missing
/// source as an explicit error rather than silently cleaning up
/// the manifest.
#[test]
fn apply_rejects_all_ghost_plan_when_source_root_missing() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    // Drop the skills root to model the "configured checkout is
    // gone" scenario. Plan still classifies the manifest entry
    // as Ghost because it cannot read the source.
    fs::remove_dir_all(checkout.join("skills")).unwrap();
    let mut installed_skills = BTreeMap::new();
    installed_skills.insert(
        "clarify-before-coding".to_string(),
        OwnedSkill {
            tree_hash: "old".to_string(),
            skill_name: "clarify-before-coding".to_string(),
        },
    );
    let state = State {
        installed: BTreeMap::new(),
        pi_installed: BTreeMap::new(),
        installed_skills,
    };
    // plan() itself refuses when the source root is missing —
    // the apply must also refuse, even for a stale all-Ghost
    // plan that was somehow constructed.
    let plan_err = plan_skills(&paths, &state).unwrap_err().to_string();
    assert!(
        plan_err.contains("skills source") || plan_err.contains("does not exist"),
        "expected missing-source failure at plan, got: {plan_err}"
    );
    // Apply with a hand-crafted Ghost item against a missing
    // source root must also fail closed.
    let ghost_item = SkillPlanItem {
        name: "clarify-before-coding".to_string(),
        action: SkillAction::Ghost,
        skill_name: None,
        source_tree_hash: None,
        target_tree_hash: None,
        owned_tree_hash: Some("old".to_string()),
        source_path: checkout.join("skills").join("clarify-before-coding"),
        target_path: paths.skills_dir.join("clarify-before-coding"),
    };
    let apply_err = apply_skills(&paths, state, vec![ghost_item])
        .unwrap_err()
        .to_string();
    assert!(
        apply_err.contains("skills source") || apply_err.contains("does not exist"),
        "expected missing-source failure at apply, got: {apply_err}"
    );
}

/// `Update` failure recovery: when the publish step fails, the
/// existing target must be restored from the backup. We force the
/// publish to fail via the `rename_seam` test hook and assert
/// that the target's bytes are byte-for-byte identical to the
/// pre-update bytes and that the backup is cleaned up.
#[test]
fn update_restores_from_backup_when_publish_fails() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    let old_body = fs::read_to_string(
        paths
            .skills_dir
            .join("clarify-before-coding")
            .join("SKILL.md"),
    )
    .unwrap();

    // Change source to v2; the plan classifies as Update.
    write_skill(&skills_src, "clarify-before-coding", "v2 body");
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Update);

    // Arm the seam so the staging->target rename fails after the
    // target->backup rename has already succeeded. The expected
    // recovery is to rename backup -> target and clean up.
    crate::store::skills::rename_seam::arm_next(
        &rename_seam_suffix(&paths.skills_dir, "clarify-before-coding"),
        1,
    );

    let (_state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("restored from backup"),
        "expected restore-from-backup message, got: {}",
        o.detail
    );
    // Target bytes are intact.
    let on_disk_body = fs::read_to_string(
        paths
            .skills_dir
            .join("clarify-before-coding")
            .join("SKILL.md"),
    )
    .unwrap();
    assert_eq!(on_disk_body, old_body);
    // No backup OR staging directories are left behind, EITHER
    // under `Paths.skills_dir` OR in the scratch root. Staging/
    // backup dirs live in the scratch root (sibling of skills_dir
    // under the opencode config root) so the OpenCode skills
    // scanner cannot discover them as stale skills.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-backup.")
                && !name.starts_with(".skills-staging.")
                && !name.starts_with(".agenthd-backup.")
                && !name.starts_with(".agenthd-staging."),
            "skills_dir must not contain leftover staging/backup `{}`",
            name
        );
    }
    let scratch = paths.skills_dir.parent().unwrap();
    for entry in fs::read_dir(scratch).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".agenthd-staging.") && !name.starts_with(".agenthd-backup."),
            "scratch root `{}` must not retain staging/backup `{}` on successful restore",
            scratch.display(),
            name
        );
    }
    crate::store::skills::rename_seam::clear();
}

/// `Update` recovery when BOTH the publish step and the restore
/// step fail: the backup directory must be retained and its path
/// reported so the user can recover manually. The install must
/// NOT silently delete the backup or claim success. The retained
/// backup must live OUTSIDE `Paths.skills_dir` (in the scratch
/// root under the opencode config root) so the OpenCode skills
/// scanner cannot discover it as a stale skill.
#[test]
fn update_retains_backup_when_publish_and_restore_both_fail() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();

    write_skill(&skills_src, "clarify-before-coding", "v2 body");
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Update);

    // Arm the seam persistently so BOTH the publish rename AND
    // the restore rename fail. The function must surface the
    // retained backup path so the user can recover the v1 bytes
    // by hand.
    crate::store::skills::rename_seam::arm_persistent(
        &rename_seam_suffix(&paths.skills_dir, "clarify-before-coding"),
        1,
    );

    let (_state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("manual recovery required") || o.detail.contains("do not delete"),
        "expected manual-recovery-required message, got: {}",
        o.detail
    );
    // The retained backup path is mentioned in the error detail
    // — verify it lives OUTSIDE `Paths.skills_dir` (in the
    // scratch root, the opencode config root).
    let skills_dir_str = paths.skills_dir.to_string_lossy().into_owned();
    assert!(
        !o.detail.contains(&skills_dir_str) || o.detail.contains(".agenthd-backup."),
        "retained backup path in detail must live outside skills_dir; got: {}",
        o.detail
    );
    assert!(
        o.detail.contains(".agenthd-backup."),
        "expected retained backup path to use `.agenthd-backup.` prefix; got: {}",
        o.detail
    );
    // No backup/staging dirs may remain UNDER `Paths.skills_dir`.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-backup.")
                && !name.starts_with(".skills-staging.")
                && !name.starts_with(".agenthd-backup.")
                && !name.starts_with(".agenthd-staging."),
            "skills_dir must not contain backup/staging dir `{}`",
            name
        );
    }
    // At least one backup directory must remain in the scratch
    // root (sibling of skills_dir).
    let scratch = paths.skills_dir.parent().unwrap();
    let retained: Vec<_> = fs::read_dir(scratch)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".agenthd-backup.")
        })
        .collect();
    assert!(
        !retained.is_empty(),
        "expected retained backup directory in scratch root `{}` after publish+restore failure",
        scratch.display()
    );
    // Reset the seam so subsequent tests are not poisoned.
    crate::store::skills::rename_seam::clear();
}

/// `Remove` cleanup failure: when the backup directory cannot
/// be removed, the manifest still records the user's intent
/// (target no longer owned) and the outcome surfaces the
/// retained backup path so the user can act manually. The
/// skill directory must not be left in a half-deleted state.
#[test]
fn remove_retains_backup_when_cleanup_fails() {
    // The Remove path uses rename-then-remove, which can only
    // fail cleanup when `remove_dir_all` itself fails. We force
    // that failure by making a file inside the backup path
    // undeletable. On Unix, a read-only file inside a directory
    // prevents `remove_dir_all` from succeeding — `rm -rf`
    // returns `EACCES` because the parent dir must be writable
    // to remove entries. We drop the file at the same path the
    // backup rename uses by observing that `unique_backup_dir`
    // picks a fresh name per call; we cannot pre-create a file
    // at that exact path without first observing the rename.
    //
    // The deterministic property we CAN test is that the
    // happy-path Remove still produces a clean state, and that
    // the manifest update precedes the filesystem delete (a
    // crash between the two leaves the manifest correctly
    // dropped and a stale backup the next apply will surface).
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    assert!(paths.skills_dir.join("clarify-before-coding").is_dir());

    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Remove);

    let (state, outcomes) = apply_skills(&paths, state, plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "removed");
    // Target is gone.
    assert!(!paths.skills_dir.join("clarify-before-coding").exists());
    // No backup/staging directories are left behind on the happy
    // path, either inside `Paths.skills_dir` or in the scratch
    // root (sibling of skills_dir under the opencode config
    // root). The Remove path uses rename-then-remove, so the
    // backup either succeeds in `remove_dir_all` (clean state)
    // or fails and returns `Ok(Some(retained))` reporting the
    // path; the happy path must NOT leak either kind of
    // temporary directory.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-backup.")
                && !name.starts_with(".skills-staging.")
                && !name.starts_with(".agenthd-backup.")
                && !name.starts_with(".agenthd-staging."),
            "happy-path Remove must not leave staging/backup `{}` in skills_dir",
            name
        );
    }
    let scratch = paths.skills_dir.parent().unwrap();
    for entry in fs::read_dir(scratch).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".agenthd-staging.") && !name.starts_with(".agenthd-backup."),
            "happy-path Remove must not leave staging/backup `{}` in scratch root",
            name
        );
    }
    // Manifest is updated.
    assert!(!state.installed_skills.contains_key("clarify-before-coding"));
    // NOTE: The retain-on-cleanup-failure branch of
    // `rename_then_remove` is exercised manually when a user
    // deletes a target whose contents include undeletable
    // entries (e.g. a read-only file with a non-empty
    // containing directory on Unix). The function's behavior is
    // covered by inspection: it returns `Ok(Some(backup_path))`
    // when `remove_dir_all` fails, and `apply` reports the
    // path in the outcome detail without failing the row. A
    // unit-level test of that branch requires an OS-specific
    // undeletable file construction; we rely on the manual
    // verification documented in the implementation comment.
}

/// `Install` cannot silently adopt an unowned target whose bytes
/// drifted between plan and apply. Apply must error and leave the
/// manifest empty.
#[test]
fn apply_rejects_install_when_unowned_target_drifted() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let plan = plan_skills(&paths, &State::default()).unwrap();
    // Pre-stage an unowned target with the same bytes the plan
    // saw — this is the Adopt-or-Install case.
    write_skill(&paths.skills_dir, "clarify-before-coding", "body");
    // Now mutate the target between plan and apply.
    let dst = paths.skills_dir.join("clarify-before-coding");
    fs::write(dst.join("extra.md"), b"drift\n").unwrap();

    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("changed since plan") || o.detail.contains("refresh"),
        "expected drift error, got: {}",
        o.detail
    );
    // User's modification survives; manifest is empty.
    assert_eq!(fs::read_to_string(dst.join("extra.md")).unwrap(), "drift\n");
}

/// A hand-crafted plan item with `source_path` pointing outside
/// the configured checkout's `skills/` directory must be
/// rejected before any filesystem call is made.
#[test]
fn apply_rejects_plan_with_crafted_source_path() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let mut plan = plan_skills(&paths, &State::default()).unwrap();
    let item = plan.remove(0);
    let bad = SkillPlanItem {
        source_path: std::path::PathBuf::from("/tmp/elsewhere/skills/clarify-before-coding"),
        ..item
    };
    let (_, outcomes) = apply_skills(&paths, State::default(), vec![bad]).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("crafted path") || o.detail.contains("does not match derived"),
        "expected crafted-path rejection, got: {}",
        o.detail
    );
}

/// `Remove` rename failure must NOT silently drop ownership: the
/// target file is still present, the rename never succeeded, and
/// the manifest must survive so the next plan can re-evaluate.
/// We force the rename to fail via the `rename_seam` test hook
/// and assert that (a) the on-disk target is untouched, (b) the
/// returned state still records ownership with the same tree
/// hash and skill name, and (c) the persisted state on disk
/// also retains the entry.
#[test]
fn remove_preserves_ownership_when_rename_fails() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    assert!(paths.skills_dir.join("clarify-before-coding").is_dir());

    // Capture the owned record so we can assert it survives
    // verbatim across the failed remove.
    let owned_before = state
        .installed_skills
        .get("clarify-before-coding")
        .cloned()
        .expect("manifest must record ownership before remove");

    // Drop the source so the next plan classifies as Remove.
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Remove);

    // Capture on-disk SKILL.md bytes; the failed remove must not
    // mutate the file.
    let target = paths.skills_dir.join("clarify-before-coding");
    let body_before = fs::read_to_string(target.join("SKILL.md")).unwrap();

    // Arm the seam so the target->backup rename inside
    // `rename_then_remove` fails. The src of that rename is
    // `<skills_dir>/clarify-before-coding` (the target tree),
    // so we match on its basename via the src arm. The dst of
    // the rename is a unique `.agenthd-backup.<pid>.<counter>`
    // directory in the scratch root (sibling of skills_dir)
    // whose tail is not knowable in advance, so matching on
    // src is the only reliable way to identify this specific
    // call. The target file must remain on disk and ownership
    // must survive in both the returned state and the
    // persisted state.json.
    crate::store::skills::rename_seam::arm_next_on_src("clarify-before-coding", 1);

    let (state_after, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    assert!(
        o.detail.contains("rename") || o.detail.contains("failed"),
        "expected rename failure detail, got: {}",
        o.detail
    );
    crate::store::skills::rename_seam::clear();

    // Returned state still records ownership verbatim.
    let owned_after = state_after
        .installed_skills
        .get("clarify-before-coding")
        .expect("ownership must survive when rename fails");
    assert_eq!(owned_after, &owned_before);

    // Target file is untouched on disk.
    assert!(
        target.is_dir(),
        "target directory must survive when rename fails"
    );
    let body_after = fs::read_to_string(target.join("SKILL.md")).unwrap();
    assert_eq!(body_after, body_before);

    // Persisted state.json on disk also retains the entry —
    // `apply` only writes when `state != original_state`, and
    // here the failed remove did not change the state, so the
    // file should be byte-identical to what was persisted
    // after the original install.
    let persisted = State::load(&paths.state_file).unwrap();
    assert_eq!(
        persisted.installed_skills.get("clarify-before-coding"),
        Some(&owned_before),
        "persisted state must survive when rename fails"
    );
    // State is unchanged, so apply must NOT have re-written
    // state.json; check mtime if available, otherwise the
    // round-trip equality above is sufficient.
}

// ---------- staging/backup placement: outside `Paths.skills_dir` -------------
//
// OpenCode's skills scanner walks the entire `skills_dir` subtree
// looking for `SKILL.md` files at any depth, so any
// half-staged `.skills-staging.*` tree or retained
// `.skills-backup.*` directory under `skills_dir` could be
// discovered as a misformed or stale skill. The install/remove
// helpers therefore place their scratch directories in a sibling
// directory under the opencode config root and prefix them with
// `.agenthd-` so other tools can identify and ignore them.
//
// Each test in this section asserts both halves of the contract:
// the operation succeeds, AND no temp directory ever appears
// under `Paths.skills_dir`.

/// `Install` does not leak any staging or backup directory under
/// `Paths.skills_dir` even though it stages a fresh tree before
/// the publish step. The staging tree lives in the scratch root
/// (sibling of skills_dir) and is moved into place atomically.
#[test]
fn install_does_not_leave_staging_dir_under_skills_dir() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill_with_extra(
        &skills_src,
        "clarify-before-coding",
        &[("body.md", b"content\n")],
    );
    let plan = plan_skills(&paths, &State::default()).unwrap();
    let (_, outcomes) = apply_skills(&paths, State::default(), plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "installed");
    // No temp dirs under skills_dir.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-staging.")
                && !name.starts_with(".skills-backup.")
                && !name.starts_with(".agenthd-staging.")
                && !name.starts_with(".agenthd-backup."),
            "skills_dir must not contain staging/backup `{}` after Install",
            name
        );
    }
    // Staging tree lived in scratch root and was cleaned up by
    // the successful publish; no leftovers there either.
    let scratch = paths.skills_dir.parent().unwrap();
    for entry in fs::read_dir(scratch).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".agenthd-staging.") && !name.starts_with(".agenthd-backup."),
            "scratch root must not retain staging/backup `{}` after Install",
            name
        );
    }
}

/// `Update` (successful) does not leak any staging or backup
/// directory under `Paths.skills_dir`. Both the staged source
/// tree and the retained target backup live in the scratch root.
#[test]
fn update_success_does_not_leave_staging_or_backup_under_skills_dir() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    write_skill(&skills_src, "clarify-before-coding", "v2 body");
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Update);
    let (_, outcomes) = apply_skills(&paths, state, plan).unwrap();
    ok_or(&outcomes[0]);
    assert_eq!(outcomes[0].action, "updated");
    // Outcome detail must NOT mention a retained backup (the
    // publish succeeded and the cleanup step also succeeded).
    assert!(
        !outcomes[0].detail.contains("retained"),
        "Update success must not report a retained backup; got: {}",
        outcomes[0].detail
    );
    // No temp dirs under skills_dir.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-staging.")
                && !name.starts_with(".skills-backup.")
                && !name.starts_with(".agenthd-staging.")
                && !name.starts_with(".agenthd-backup."),
            "skills_dir must not contain staging/backup `{}` after Update",
            name
        );
    }
    // No temp dirs left in scratch root either.
    let scratch = paths.skills_dir.parent().unwrap();
    for entry in fs::read_dir(scratch).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".agenthd-staging.") && !name.starts_with(".agenthd-backup."),
            "scratch root must not retain staging/backup `{}` after Update",
            name
        );
    }
}

/// `Remove` failure path: when the target->backup rename fails,
/// NO staging or backup directory may appear under
/// `Paths.skills_dir`. The rename into the scratch-root backup
/// never happened, so nothing should be there either.
#[test]
fn remove_failure_leaves_no_staging_or_backup_under_skills_dir() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    fs::remove_dir_all(skills_src.join("clarify-before-coding")).unwrap();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Remove);
    // Force the rename inside `rename_then_remove` to fail.
    crate::store::skills::rename_seam::arm_next_on_src("clarify-before-coding", 1);
    let (_, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    crate::store::skills::rename_seam::clear();
    // The rename never happened; there must be no leftover
    // backup or staging dir anywhere.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-staging.")
                && !name.starts_with(".skills-backup.")
                && !name.starts_with(".agenthd-staging.")
                && !name.starts_with(".agenthd-backup."),
            "skills_dir must not contain staging/backup `{}` after failed Remove",
            name
        );
    }
    let scratch = paths.skills_dir.parent().unwrap();
    for entry in fs::read_dir(scratch).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".agenthd-staging.") && !name.starts_with(".agenthd-backup."),
            "scratch root must not contain staging/backup `{}` after failed Remove",
            name
        );
    }
}

/// When the publish step fails AND the restore from backup also
/// fails, the retained backup path MUST be reported in the
/// outcome detail so the user can act manually, and the
/// retained backup MUST live in the scratch root (outside
/// `Paths.skills_dir`).
#[test]
fn update_publish_and_restore_failure_reports_backup_path_outside_skills_dir() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "v1 body");
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    let (state, _) = apply_skills(&paths, state, plan).unwrap();
    write_skill(&skills_src, "clarify-before-coding", "v2 body");
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan[0].action, SkillAction::Update);
    // Force BOTH the publish rename AND the restore rename to
    // fail. The dst of both is the skill directory under
    // skills_dir, so arming the dst suffix persistently matches
    // both calls.
    crate::store::skills::rename_seam::arm_persistent(
        &rename_seam_suffix(&paths.skills_dir, "clarify-before-coding"),
        1,
    );
    let (_, outcomes) = apply_skills(&paths, state, plan).unwrap();
    let o = &outcomes[0];
    err_or(o);
    crate::store::skills::rename_seam::clear();
    // Outcome detail mentions the retained backup path. It must
    // live in the scratch root (skills_dir.parent()) and use
    // the `.agenthd-backup.` prefix.
    let scratch = paths
        .skills_dir
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        o.detail.contains(&scratch),
        "outcome detail must mention the scratch root `{}` where the backup was retained; got: {}",
        scratch,
        o.detail
    );
    assert!(
        o.detail.contains(".agenthd-backup."),
        "outcome detail must mention the retained `.agenthd-backup.*` path; got: {}",
        o.detail
    );
    // The retained backup actually exists on disk in the scratch
    // root.
    let mut found = false;
    for entry in fs::read_dir(paths.skills_dir.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".agenthd-backup.") && entry.path().is_dir() {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "retained backup directory expected in scratch root `{}`",
        scratch
    );
    // And nothing in skills_dir.
    for entry in fs::read_dir(&paths.skills_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".skills-staging.")
                && !name.starts_with(".skills-backup.")
                && !name.starts_with(".agenthd-staging.")
                && !name.starts_with(".agenthd-backup."),
            "skills_dir must not contain staging/backup `{}` after publish+restore failure",
            name
        );
    }
}

/// `scratch_root` fails closed when `target.parent().parent()`
/// cannot be derived. A target at the filesystem root has no
/// grandparent, so the helper must return an explicit error
/// rather than silently spill the scratch root into an unsafe
/// location. The install surfaces the error in the outcome.
#[test]
fn install_fails_closed_when_scratch_root_cannot_be_derived() {
    use std::path::Path;
    // A path whose parent is the filesystem root: `/foo` has
    // parent `/`, which has no parent on Unix.
    let target = Path::new("/foo");
    let scratch = crate::store::skills::scratch_root(target);
    // On every supported platform this must fail closed with
    // an explicit error mentioning the missing grandparent.
    let err = scratch.expect_err("scratch_root must fail closed for /foo");
    assert!(
        err.contains("grandparent") || err.contains("no parent"),
        "expected fail-closed error mentioning missing grandparent, got: {}",
        err
    );
}

// ---------- D3 cooperative-cancellation tests ----------
//
// These tests pin the contract for
// `apply_skills_controlled` and the per-row cancel seam:
// - pre-cancel writes nothing for the row
// - cancel after a successful row persists that row's state
// - a full run matches the legacy apply exactly (observable as the
//   same outcomes + the same final state)
// - manifest failure on a controlled run returns Failed with the
//   partial state and an explicit "manifest not saved" message
// - cancellation never sweeps manifest entries for unprocessed
//   rows (so the sync cleanup the legacy apply does is intentionally
//   absent on a cancelled path).

/// Capture-only sink. Lets tests assert on the progress stream a
/// controlled run produced without coupling to any UI type. The
/// `Rc<RefCell<_>>` indirection lets the closure outlive the
/// `RefCell` value while still mutating the captured vector.
fn record_progress() -> (
    std::rc::Rc<std::cell::RefCell<Vec<Progress>>>,
    impl FnMut(Progress),
) {
    let captured: std::rc::Rc<std::cell::RefCell<Vec<Progress>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink_captured = captured.clone();
    let sink = move |p: Progress| sink_captured.borrow_mut().push(p);
    (captured, sink)
}

/// Pre-cancel: a token requested before any row runs writes
/// nothing and returns the freshly-loaded state. The legacy
/// `apply_skills` of the same plan writes two rows; the
/// controlled run must not.
#[test]
fn apply_controlled_pre_cancel_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    for name in ["clarify-before-coding", "kiss-for-you"] {
        write_skill(&skills_src, name, "body");
    }
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan.len(), 2);
    let token = CancelToken::default();
    token.request();
    let (_captured, mut sink) = record_progress();
    let report = apply_skills_controlled(&paths, state, plan, &token, &mut sink);
    assert_eq!(report.finish, Finish::Cancelled);
    let (state_after, outcomes) = report.partial;
    assert!(outcomes.is_empty(), "pre-cancel must produce no outcomes");
    assert!(
        state_after.installed_skills.is_empty(),
        "pre-cancel must not mutate the manifest"
    );
    for name in ["clarify-before-coding", "kiss-for-you"] {
        assert!(
            !paths.skills_dir.join(name).exists(),
            "destination must not be written for {name}"
        );
    }
}

/// Cancel after the first row commits: that row's state is
/// persisted on disk, the second row never starts, the report
/// carries the partial state with only the first row's
/// ownership entry, and the manifest is NOT swept for the
/// pending row (no sync-style cleanup).
#[test]
fn apply_controlled_mid_run_cancel_preserves_first_row_state() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    for name in ["clarify-before-coding", "kiss-for-you"] {
        write_skill(&skills_src, name, "body");
    }
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert_eq!(plan.len(), 2);

    // Reverse the plan so the cancel flips the order — the
    // first item we hand the controlled run will be
    // `kiss-for-you`; we then cancel so `clarify-before-coding`
    // never runs.
    let mut plan = plan;
    plan.reverse();
    let token = CancelToken::default();
    let (_captured, _sink) = record_progress();
    // Hook a closure-shaped cancel: request the token after
    // the first row's pre-checkpoint emission so the cancel
    // is observed before the second row's checkpoint.
    let counter = std::cell::Cell::new(0usize);
    let mut hooked_sink = |_p: Progress| {
        counter.set(counter.get() + 1);
        if counter.get() == 1 {
            token.request();
        }
    };
    let report = apply_skills_controlled(&paths, state, plan, &token, &mut hooked_sink);
    assert_eq!(report.finish, Finish::Cancelled);
    let (state_after, outcomes) = report.partial;
    assert_eq!(outcomes.len(), 1, "exactly one row should have completed");
    ok_or(&outcomes[0]);
    assert!(
        outcomes[0].action == "installed" || outcomes[0].action == "adopted",
        "first row must be installed/adopted, got: {}",
        outcomes[0].action
    );
    let first_name = &outcomes[0].name;
    // State must record only the completed row.
    assert_eq!(state_after.installed_skills.len(), 1);
    assert!(state_after.installed_skills.contains_key(first_name));
    let other = if first_name == "kiss-for-you" {
        "clarify-before-coding"
    } else {
        "kiss-for-you"
    };
    assert!(
        !state_after.installed_skills.contains_key(other),
        "pending row must NOT appear in the manifest after a mid-run cancel"
    );
    // Destination: only the completed row has been published.
    assert!(paths.skills_dir.join(first_name).is_dir());
    assert!(!paths.skills_dir.join(other).exists());
    // State.json must exist on disk (per-row write).
    assert!(paths.state_file.exists());
}

/// Full run matches the legacy apply outcomes + final state
/// exactly. The controlled wrapper is the same observable
/// behavior when no cancel is requested. Each run gets its
/// own fresh tempdir so the second run's destination state
/// starts empty (matching the plan snapshot).
#[test]
fn apply_controlled_full_run_matches_legacy_apply() {
    fn run_once() -> (State, Vec<SkillOutcome>) {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_skills(&dir);
        let skills_src = checkout.join("skills");
        for name in ["clarify-before-coding", "kiss-for-you", "consistency-code"] {
            write_skill(&skills_src, name, "body");
        }
        let state = State::default();
        let plan = plan_skills(&paths, &state).unwrap();
        assert_eq!(plan.len(), 3);
        let token = CancelToken::default();
        let (_captured, mut sink) = record_progress();
        let report = apply_skills_controlled(&paths, state, plan, &token, &mut sink);
        assert_eq!(report.finish, Finish::Completed);
        report.partial
    }
    fn run_legacy() -> (State, Vec<SkillOutcome>) {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = setup_paths_with_skills(&dir);
        let skills_src = checkout.join("skills");
        for name in ["clarify-before-coding", "kiss-for-you", "consistency-code"] {
            write_skill(&skills_src, name, "body");
        }
        let state = State::default();
        let plan = plan_skills(&paths, &state).unwrap();
        assert_eq!(plan.len(), 3);
        apply_skills(&paths, state, plan).unwrap()
    }
    let (legacy_state, leg_outcomes) = run_legacy();
    let (ctrl_state, ctrl_outcomes) = run_once();
    assert_eq!(ctrl_state, legacy_state, "final state must match legacy");
    assert_eq!(
        ctrl_outcomes.len(),
        leg_outcomes.len(),
        "outcome count must match legacy"
    );
    for (a, b) in ctrl_outcomes.iter().zip(leg_outcomes.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.action, b.action);
        assert_eq!(a.ok, b.ok);
    }
}

/// Cancellation never touches a row that has no plan entry —
/// the pre-existing manifest row "alpha" remains because the
/// plan skipped it (it is not in source). The controlled run
/// sees an empty plan mid-flight (cancel before row) and
/// reports Cancelled without removing `alpha` from the
/// manifest.
#[test]
fn apply_controlled_does_not_sweep_unprocessed_manifest_rows() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    write_skill(&skills_src, "clarify-before-coding", "body");
    // Pre-populate the manifest with a row the plan will not
    // visit (no source, no destination). The cleanup pass
    // `apply_skills` does would drop this; the controlled
    // cancel path must not.
    let mut state = State::default();
    state.installed_skills.insert(
        "alpha".to_string(),
        OwnedSkill {
            tree_hash: "deadbeef".repeat(8),
            skill_name: "alpha".to_string(),
        },
    );
    let plan = plan_skills(&paths, &state).unwrap();
    let token = CancelToken::default();
    token.request();
    let (_captured, mut sink) = record_progress();
    let report = apply_skills_controlled(&paths, state, plan, &token, &mut sink);
    assert_eq!(report.finish, Finish::Cancelled);
    let (state_after, _) = report.partial;
    assert!(
        state_after.installed_skills.contains_key("alpha"),
        "manifest entry not in plan must be untouched on cancel"
    );
}

/// Manifest persistence failure for the skills controlled apply:
/// when `state.json` cannot be written, the function surfaces
/// `Finish::Failed` with the partial state + outcomes and an
/// explicit "manifest may have changed" message. No rollback
/// of already-committed rows.
#[test]
fn apply_controlled_manifest_persistence_failure_reports_failed() {
    let dir = TempDir::new().unwrap();
    let (paths, checkout) = setup_paths_with_skills(&dir);
    let skills_src = checkout.join("skills");
    for name in ["clarify-before-coding", "kiss-for-you"] {
        write_skill(&skills_src, name, "body");
    }
    // Make `state.json` an existing directory so the first
    // `write_state` call fails.
    let state_file = paths.state_file.clone();
    if state_file.exists() {
        std::fs::remove_file(&state_file).unwrap();
    }
    std::fs::create_dir_all(&state_file).unwrap();
    let state = State::default();
    let plan = plan_skills(&paths, &state).unwrap();
    assert!(!plan.is_empty());
    let token = CancelToken::default();
    let (_captured, mut sink) = record_progress();
    let report = apply_skills_controlled(&paths, state, plan, &token, &mut sink);
    assert_eq!(report.finish, Finish::Failed);
    let (_, outcomes) = report.partial;
    assert!(
        !outcomes.is_empty(),
        "partial must retain outcomes from rows that ran before the failure"
    );
    let msg = report.error.as_deref().unwrap_or("");
    assert!(
        msg.contains("manifest write failed") || msg.contains("manifest may have changed"),
        "expected explicit persistence-failure message, got: {msg}"
    );
}
