//! Live smoke test for the Tools installer.
//!
//! Exercises `tools::install_tool` against the real pi-psql remote and a
//! real temp filesystem, with `git`, `node`, and `npm` actually running.
//!
//! Ignored by default because it touches the network and writes to a temp
//! directory. Run with:
//!
//! ```text
//! cargo test --test tools_install_smoke -- --ignored --nocapture
//! ```
//!
//! The test re-includes the relevant modules with `#[path]` so it can
//! invoke them from a separate test binary without a library target.

// The smoke test only uses a small surface (`store::Paths` and
// `tools::{install_tool, tool_status, DEFAULT_CATALOG}`); most of the
// re-included modules' public items end up "unused" from clippy's
// perspective. Silence that for this binary only.
#![allow(dead_code)]

#[path = "../src/agent.rs"]
mod agent;
#[path = "../src/store.rs"]
mod store;
#[path = "../src/tools/mod.rs"]
mod tools;

#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn real_install_pi_psql_against_remote() {
    use crate::store::Paths;
    use crate::tools;
    let dir = tempfile::tempdir().expect("tempdir");
    let xdg = dir.path().join("xdg");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let paths = Paths::resolve(
        Some(xdg.to_str().expect("xdg utf-8")),
        Some(home.to_str().expect("home utf-8")),
    )
    .expect("paths");
    paths.ensure_dirs().expect("ensure_dirs");

    let entry = &tools::DEFAULT_CATALOG[0];
    let destination = tools::destination_for(&paths, entry);
    assert!(
        !destination.exists(),
        "destination must be absent before install"
    );

    let outcome = tools::install_tool(&paths, entry).expect("install_tool must not itself error");
    assert_eq!(
        outcome.status,
        tools::ToolStatus::Installed,
        "install should succeed: {}",
        outcome.detail
    );
    let item = tools::tool_status(&paths, entry).expect("tool_status");
    assert_eq!(item.status, tools::ToolStatus::Installed);
    assert!(destination.is_dir(), "destination must be a directory");
    assert!(
        destination.join("SKILL.md").is_file(),
        "SKILL.md must be byte-equal to the staged copy"
    );
    let skill_text = std::fs::read_to_string(destination.join("SKILL.md")).expect("read SKILL.md");
    assert!(
        skill_text.lines().any(|l| l == "name: pi-psql"),
        "frontmatter name must match the catalog skill, got: {:?}",
        skill_text.lines().next()
    );
    assert!(
        destination.join("node_modules").is_dir(),
        "npm ci must have produced node_modules"
    );

    // Second run with the destination already present must report Conflict,
    // not overwrite.
    let outcome = tools::install_tool(&paths, entry).expect("install_tool on existing");
    assert_eq!(
        outcome.status,
        tools::ToolStatus::Conflict,
        "second install must refuse to overwrite: {}",
        outcome.detail
    );
}
