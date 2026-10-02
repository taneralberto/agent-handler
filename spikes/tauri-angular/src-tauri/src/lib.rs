//! D1 spike: Tauri v2 + Angular prototype backend.
//!
//! Three `#[tauri::command]`s. Two are strictly read-only, both
//! wrapping the same composition helpers the unit tests
//! call directly:
//!
//! - `settings_status` → `compose_settings_status(&Paths)`
//! - `list_agents`     → `compose_agents_list(&Paths)`
//!
//! The third, `apply_checkout`, is the smallest mutating surface
//! Fase 5 adds: the GUI Settings screen needs a way to validate
//! and persist a user-typed checkout path. It delegates to the
//! lib's `workflows::apply_checkout` (the same one the TUI
//! Settings screen calls) via a `compose_apply_checkout` helper,
//! so the visible failure modes and the persisted JSON are
//! bit-for-bit identical to the TUI. After a successful save the
//! frontend re-invokes the read-only commands to re-render
//! Settings + Agents; if the post-write revalidate fails the
//! lib still persists but returns the error so the GUI can
//! surface it without rolling back (the TUI does the same).
//!
//! The helpers take `&Paths` so tests can build them from a
//! `tempfile::TempDir` without mutating `HOME` /
//! `XDG_CONFIG_HOME`. The commands are thin: they resolve
//! `Paths::from_env()` and call the helper. Every test of the
//! helper is a test of the command's data path; runtime
//! command dispatch itself is exercised by `tauri::Builder`
//! at integration time, not by these unit tests.
//!
//! Design rules (the spike does NOT relax any of these):
//!
//! - **Single source of truth.** The lib's
//!   `workflows::read_checkout`,
//!   `workflows::list_canonical_agents`, and
//!   `workflows::apply_checkout` are reused verbatim. The
//!   `ApplyError::message()` text is forwarded to the frontend
//!   unchanged so the TUI's error strings stay canonical.
//! - **No projection of prompt or permissions.** `AgentSummary`
//!   has exactly four fields; a unit test asserts the JSON
//!   serialization carries only those.
//! - **No `ensure_dirs`, no canonical/target/skills writes.**
//!   `apply_checkout` writes `settings.json` only (via
//!   `save_settings` → `write_target`); the agenthd root and
//!   target trees are the boot path's responsibility.
//! - **No CLI coupling.** `agenthd gui` locates the companion
//!   binary (see root `src/main.rs`); the spike itself never
//!   reaches into CLI args.

use agenthd::agent::Mode as AgentMode;
use agenthd::store::Paths;
use agenthd::workflows::{
    apply_checkout as apply_checkout_workflow, list_canonical_agents, read_checkout, CheckoutStatus,
};
use serde::Serialize;

/// Wire-friendly status returned to the frontend for the
/// configured checkout. Mirrors the `CheckoutStatus` cases the
/// TUI's `read_checkout` produces, plus an explicit `Error`
/// arm so the frontend never sees a thrown exception.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsStatus {
    Empty,
    Ready { checkout_path: String },
    Stale { banner: String, raw_path: String },
    Error { message: String },
}

/// Wire-friendly projection of one agent. Intentionally
/// narrower than `agenthd::agent::Agent`: we do NOT expose
/// `prompt` or `permissions`, only metadata the frontend
/// needs for a read-only list view.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentSummary {
    pub name: String,
    pub mode: String,
    pub model: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentsList {
    pub agents: Vec<AgentSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SettingsResponse {
    pub settings_file: String,
    pub status: SettingsStatus,
}

/// Composition helper: classify the configured checkout
/// using the lib's `read_checkout`. No I/O outside the
/// `paths.settings_file` the caller already holds. Returns a
/// wire-friendly `SettingsResponse` directly — the command
/// just forwards it.
pub fn compose_settings_status(paths: &Paths) -> SettingsResponse {
    let settings_file = paths.settings_file.to_string_lossy().into_owned();
    let status = match read_checkout(&paths.settings_file) {
        Ok(CheckoutStatus::Empty) => SettingsStatus::Empty,
        Ok(CheckoutStatus::Ready(path)) => SettingsStatus::Ready {
            checkout_path: path.to_string_lossy().into_owned(),
        },
        Ok(CheckoutStatus::Stale { raw, banner }) => SettingsStatus::Stale {
            banner,
            raw_path: raw.to_string_lossy().into_owned(),
        },
        Err(e) => SettingsStatus::Error {
            message: format!("read settings: {e}"),
        },
    };
    SettingsResponse {
        settings_file,
        status,
    }
}

/// Composition helper: classify the configured checkout
/// and, if `Ready`, enumerate the agents directory and project
/// each `Agent` to `AgentSummary`. No I/O outside `paths`. The
/// caller passes a `Paths` whose `canonical_dir` may be either
/// the agenthd default (pre-`with_settings`) or already
/// re-pointed at the configured checkout.
pub fn compose_agents_list(paths: &Paths) -> AgentsList {
    let checkout_path = match read_checkout(&paths.settings_file) {
        Ok(CheckoutStatus::Ready(path)) => path,
        Ok(CheckoutStatus::Empty) => {
            return AgentsList {
                agents: vec![],
                error: Some(
                    "no checkout configured yet (settings.json is empty); \
                     configure one in the agenthd TUI to see agents here"
                        .to_string(),
                ),
            };
        }
        Ok(CheckoutStatus::Stale { banner, .. }) => {
            return AgentsList {
                agents: vec![],
                error: Some(format!(
                    "configured checkout is unusable: {banner}; \
                     repair it in the agenthd TUI"
                )),
            };
        }
        Err(e) => {
            return AgentsList {
                agents: vec![],
                error: Some(format!("read settings: {e}")),
            };
        }
    };

    let settings = agenthd::store::Settings::new(checkout_path.to_string_lossy().into_owned());
    let paths = match paths.clone().with_settings(&settings) {
        Ok(p) => p,
        Err(e) => {
            return AgentsList {
                agents: vec![],
                error: Some(format!("apply settings: {e}")),
            };
        }
    };

    match list_canonical_agents(&paths) {
        Ok(agents) => AgentsList {
            agents: agents.into_iter().map(project_agent).collect(),
            error: None,
        },
        Err(e) => AgentsList {
            agents: vec![],
            error: Some(e.to_string()),
        },
    }
}

/// Composition helper for the mutating path: delegates to the
/// lib's `workflows::apply_checkout` (which writes
/// `settings.json` and re-points `paths.canonical_dir` on full
/// success). Clones `paths` so a failure leaves the caller's
/// `Paths` untouched — the lib's `apply_checkout` already
/// preserves the original `paths` on failure, but cloning here
/// matches the read-only helpers' signature and lets the
/// command stay a one-liner. On success returns the trimmed
/// absolute path the workflow just persisted; on failure
/// returns the exact `ApplyError::message()` text the TUI
/// already renders (no prefix munging, no copy of the
/// validation cases — the lib is the single source of truth).
pub fn compose_apply_checkout(paths: &Paths, input: &str) -> Result<String, String> {
    let mut scratch = paths.clone();
    match apply_checkout_workflow(&mut scratch, input) {
        Ok(path) => Ok(path.to_string_lossy().into_owned()),
        Err(err) => Err(err.message()),
    }
}

/// `Agent` → `AgentSummary`. Pulled out so the projection is
/// testable in isolation (see `projection_drops_prompt_and_
/// permissions`) and so the helper stays readable.
fn project_agent(a: agenthd::agent::Agent) -> AgentSummary {
    AgentSummary {
        name: a.name,
        mode: match a.mode {
            AgentMode::subagent => "subagent",
            AgentMode::primary => "primary",
            AgentMode::all => "all",
        }
        .to_string(),
        model: a.model,
        description: a.description,
    }
}

#[tauri::command]
fn settings_status() -> SettingsResponse {
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            return SettingsResponse {
                settings_file: String::new(),
                status: SettingsStatus::Error {
                    message: format!("resolve config paths: {e}"),
                },
            };
        }
    };
    compose_settings_status(&paths)
}

#[tauri::command]
fn list_agents() -> AgentsList {
    match Paths::from_env() {
        Ok(paths) => compose_agents_list(&paths),
        Err(e) => AgentsList {
            agents: vec![],
            error: Some(format!("resolve config paths: {e}")),
        },
    }
}

/// Validate, persist, and re-point the configured checkout at
/// the path the frontend typed. The frontend passes the raw
/// buffer verbatim — trim, empty-rejection, absolute check,
/// `agents/` presence, symlink rejection, and the post-write
/// revalidation all live in `workflows::apply_checkout` so the
/// TUI and the GUI share the same visible contract. Returns
/// `Result<String, String>`: on `Ok` the `String` is the trimmed
/// absolute path the helper just persisted; on `Err` the
/// `String` is `ApplyError::message()` verbatim. Tauri serializes
/// the `Err` arm straight into the JS rejection payload, so the
/// frontend `catch` receives the literal text.
#[tauri::command]
fn apply_checkout(checkout_path: String) -> Result<String, String> {
    let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
    compose_apply_checkout(&paths, &checkout_path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            settings_status,
            list_agents,
            apply_checkout
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    //! Tests call the composition helpers directly with a
    //! `Paths` built from a `TempDir` — the same pattern the
    //! lib `agenthd` tests use. No `HOME` / `XDG_CONFIG_HOME`
    //! mutation, no env-side effects, no writes outside the
    //! tempdir. The fixtures written under the tempdir are
    //! test data, not code.
    //!
    //! Assertions cover both the typed DTOs (Rust shape) and
    //! the serialized JSON (wire shape) so the
    //! `no prompt / no permissions` contract is pinned on both
    //! sides.

    use super::*;
    use agenthd::agent::Agent;
    use agenthd::store::{save_settings, Settings};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    fn paths_in(dir: &TempDir) -> Paths {
        let home = dir.path().to_str().unwrap();
        let xdg = dir.path().join("xdg").to_str().unwrap().to_string();
        Paths::resolve(Some(&xdg), Some(home)).unwrap()
    }

    fn write_md(agents_dir: &std::path::Path, name: &str, body: &str) {
        fs::write(agents_dir.join(format!("{name}.md")), body).unwrap();
    }

    fn make_agent(name: &str, mode: AgentMode) -> Agent {
        Agent {
            name: name.to_string(),
            description: format!("{name} description"),
            mode,
            model: Some(format!("prov/{name}")),
            prompt: format!("SECRET PROMPT FOR {name} — must never leak"),
            permissions: {
                let mut m = BTreeMap::new();
                m.insert("bash".to_string(), agenthd::agent::PermissionAction::Allow);
                m.insert("read".to_string(), agenthd::agent::PermissionAction::Deny);
                m
            },
        }
    }

    /// `Empty`: no `settings.json` → the helper surfaces
    /// `Empty` and list returns the friendly first-run
    /// message.
    #[test]
    fn compose_settings_status_empty() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);

        let resp = compose_settings_status(&paths);
        assert_eq!(
            resp,
            SettingsResponse {
                settings_file: paths.settings_file.to_string_lossy().into_owned(),
                status: SettingsStatus::Empty,
            }
        );
    }

    /// `Ready`: a valid checkout → the helper reports the
    /// configured path verbatim.
    #[test]
    fn compose_settings_status_ready() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();

        let resp = compose_settings_status(&paths);
        match resp.status {
            SettingsStatus::Ready { checkout_path } => {
                assert_eq!(checkout_path, checkout.to_string_lossy());
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        assert!(resp.settings_file.ends_with("settings.json"));
    }

    /// `Stale`: settings point at a missing path → helper
    /// surfaces `Stale` with banner + raw path.
    #[test]
    fn compose_settings_status_stale() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let bogus = dir.path().join("does-not-exist");
        save_settings(
            &paths.settings_file,
            &Settings::new(bogus.to_string_lossy().into_owned()),
        )
        .unwrap();

        let resp = compose_settings_status(&paths);
        match resp.status {
            SettingsStatus::Stale { banner, raw_path } => {
                assert_eq!(raw_path, bogus.to_string_lossy());
                assert!(banner.contains("does not exist"));
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    /// `list_agents` on a `Ready` checkout with one agent:
    /// the helper returns exactly one `AgentSummary` whose
    /// fields match the projection. No writes happen — the
    /// only file written is the markdown fixture.
    #[test]
    fn compose_agents_list_ready_projects_one_summary() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(
            &agents_dir,
            "helper",
            "---\ndescription: Test helper agent\nmode: subagent\nmodel: test/test-model\n---\nYou are a test helper.\n",
        );
        save_settings(
            &paths.settings_file,
            &Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();

        let resp = compose_agents_list(&paths);
        assert_eq!(resp.error, None);
        assert_eq!(resp.agents.len(), 1);
        let s = &resp.agents[0];
        assert_eq!(s.name, "helper");
        assert_eq!(s.mode, "subagent");
        assert_eq!(s.model.as_deref(), Some("test/test-model"));
        assert_eq!(s.description, "Test helper agent");
    }

    /// `list_agents` on `Empty` returns no agents and the
    /// friendly "configure one in the agenthd TUI" message.
    #[test]
    fn compose_agents_list_empty_surfaces_friendly_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);

        let resp = compose_agents_list(&paths);
        assert!(resp.agents.is_empty());
        let err = resp.error.expect("error must be set on Empty");
        assert!(
            err.contains("no checkout configured yet"),
            "expected first-run hint, got: {err}"
        );
    }

    /// `list_agents` on `Stale` returns no agents and the
    /// "configured checkout is unusable" message.
    #[test]
    fn compose_agents_list_stale_blocks_listing() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let bogus = dir.path().join("does-not-exist");
        save_settings(
            &paths.settings_file,
            &Settings::new(bogus.to_string_lossy().into_owned()),
        )
        .unwrap();

        let resp = compose_agents_list(&paths);
        assert!(resp.agents.is_empty());
        let err = resp.error.expect("error must be set on Stale");
        assert!(
            err.contains("configured checkout is unusable"),
            "expected Stale hint, got: {err}"
        );
    }

    /// Contract pin: the `Agent → AgentSummary` projection
    /// drops `prompt` and `permissions`. The agent built here
    /// has both populated with values that would be impossible
    /// to confuse with metadata; if the projection ever leaks
    /// them, the JSON assertion below fails first.
    #[test]
    fn projection_drops_prompt_and_permissions() {
        let agent = make_agent("helper", AgentMode::subagent);

        let summary = project_agent(agent);
        assert_eq!(summary.name, "helper");
        assert_eq!(summary.mode, "subagent");
        assert_eq!(summary.model.as_deref(), Some("prov/helper"));
        assert_eq!(summary.description, "helper description");

        // Wire shape: JSON must not contain the prompt text or
        // any permission key. This is the strongest assertion
        // we can make without a TS schema check.
        let json = serde_json::to_value(&summary).unwrap();
        let obj = json.as_object().expect("summary must serialize to object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["description", "mode", "model", "name"]
                .into_iter()
                .collect()
        );
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(
            !serialized.contains("SECRET PROMPT"),
            "prompt must not leak into the wire payload, got: {serialized}"
        );
        assert!(
            !serialized.contains("bash") && !serialized.contains("read"),
            "permission keys must not leak into the wire payload, got: {serialized}"
        );
    }

    /// End-to-end JSON pin: a `Ready` checkout with one agent
    /// produces a `SettingsResponse` and `AgentsList` whose
    /// serialized JSON carries no leak surface — no prompt,
    /// no permission keys, no extra fields.
    #[test]
    fn ready_checkout_json_has_no_prompt_or_permission_leak() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        let md = "---\ndescription: holds secrets\nmode: subagent\nmodel: test/test-model\npermission:\n  bash: allow\n  edit: deny\n---\nTOP SECRET PROMPT body\n";
        write_md(&agents_dir, "secret-keeper", md);
        save_settings(
            &paths.settings_file,
            &Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();

        let settings = compose_settings_status(&paths);
        // Schema pin on settings payload: top-level keys are
        // exactly `settings_file` + `status`; the Ready arm
        // carries only `checkout_path`; nothing else can leak
        // prompt or permission data.
        let value = serde_json::to_value(&settings).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(
            obj.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["settings_file", "status"].into_iter().collect()
        );

        let list = compose_agents_list(&paths);
        // Sanity: the fixture must parse; otherwise this
        // test is checking the schema of an error envelope
        // and not the Ready path. Fail loudly with the
        // exact error message if the fixture is malformed.
        assert!(
            list.error.is_none(),
            "fixture must parse cleanly, got error: {:?}",
            list.error
        );
        let value = serde_json::to_value(&list).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(
            obj.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["agents", "error"].into_iter().collect()
        );
        let arr = obj["agents"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let summary_keys: std::collections::BTreeSet<&str> = arr[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            summary_keys,
            ["description", "mode", "model", "name"]
                .into_iter()
                .collect()
        );

        // Negative pin: `prompt` and `permissions` must not
        // appear as keys anywhere in the agents payload, even
        // nested (they should not be present at all in
        // `AgentSummary`, but pinning the keys is the
        // strongest static guarantee we can make from Rust).
        let payload = serde_json::to_string(&list).unwrap();
        assert!(
            !payload.contains("\"prompt\"") && !payload.contains("\"permissions\""),
            "wire payload must not reference `prompt` or `permissions` keys, got: {payload}"
        );
    }

    // ---------- apply_checkout (GUI Settings mutation) ----------

    /// Write a tiny starter `.md` so `list_canonical_agents`
    /// produces a non-empty `AgentSummary` after the apply.
    fn write_minimal_agent(agents_dir: &std::path::Path, name: &str) {
        let body = format!(
            "---\ndescription: {name} helper\nmode: subagent\nmodel: test/{name}\n---\nYou are a test {name}.\n"
        );
        fs::write(agents_dir.join(format!("{name}.md")), body).unwrap();
    }

    /// End-to-end happy path: a valid checkout path is
    /// persisted, the Settings helper now reports `Ready` for
    /// that path, and the Agents helper enumerates the
    /// `.md` files inside it. Mirrors what the GUI does on a
    /// successful Save → refresh both: same composition
    /// helpers, same data path.
    #[test]
    fn compose_apply_checkout_valid_persists_and_recomposes_settings_and_agents() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout-a");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_minimal_agent(&agents_dir, "helper");

        let resolved = compose_apply_checkout(&paths, &checkout.to_string_lossy()).unwrap();
        assert_eq!(resolved, checkout.to_string_lossy());

        // Settings must report Ready with the new checkout.
        let settings = compose_settings_status(&paths);
        match settings.status {
            SettingsStatus::Ready { checkout_path } => {
                assert_eq!(checkout_path, checkout.to_string_lossy());
            }
            other => panic!("expected Ready after apply, got {other:?}"),
        }

        // Agents must list the single fixture we wrote.
        let list = compose_agents_list(&paths);
        assert_eq!(list.error, None);
        assert_eq!(list.agents.len(), 1);
        assert_eq!(list.agents[0].name, "helper");
    }

    /// Switching between two checkouts (different agents in
    /// each) recomposes the Agents list to match the new
    /// checkout, not the old one. This is the precise behavior
    /// the GUI depends on after Save.
    #[test]
    fn compose_apply_checkout_switching_checkouts_recomposes_agents() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);

        let checkout_a = dir.path().join("checkout-a");
        let agents_a = checkout_a.join("agents");
        fs::create_dir_all(&agents_a).unwrap();
        write_minimal_agent(&agents_a, "alpha");

        compose_apply_checkout(&paths, &checkout_a.to_string_lossy()).unwrap();
        let list_a = compose_agents_list(&paths);
        assert_eq!(list_a.agents.len(), 1);
        assert_eq!(list_a.agents[0].name, "alpha");

        let checkout_b = dir.path().join("checkout-b");
        let agents_b = checkout_b.join("agents");
        fs::create_dir_all(&agents_b).unwrap();
        write_minimal_agent(&agents_b, "beta");
        write_minimal_agent(&agents_b, "gamma");

        compose_apply_checkout(&paths, &checkout_b.to_string_lossy()).unwrap();
        let list_b = compose_agents_list(&paths);
        assert_eq!(list_b.error, None);
        assert_eq!(list_b.agents.len(), 2);
        let names: Vec<&str> = list_b.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "gamma"]);
    }

    /// Empty input → exact TUI error text, no settings.json
    /// written, original canonical_dir preserved. The GUI must
    /// show the same message the TUI shows so the user gets
    /// identical feedback in both modes.
    #[test]
    fn compose_apply_checkout_empty_input_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Seed a valid prior settings file so the failing
        // apply can be checked against a known good baseline.
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_settings_bytes = fs::read(&paths.settings_file).unwrap();
        let before_paths = paths.clone();

        let err = compose_apply_checkout(&paths, "   ").unwrap_err();
        assert_eq!(err, "checkout path is empty; type the absolute path");

        // settings.json on disk is byte-for-byte the prior file.
        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(
            after_bytes, prior_settings_bytes,
            "empty input must not rewrite settings.json"
        );
        assert_eq!(
            paths, before_paths,
            "empty input must not mutate the caller's Paths"
        );
    }

    /// Relative path → exact TUI error text, no settings.json
    /// written, original canonical_dir preserved.
    #[test]
    fn compose_apply_checkout_relative_input_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_bytes = fs::read(&paths.settings_file).unwrap();
        let before = paths.clone();

        let input = "relative/path";
        let err = compose_apply_checkout(&paths, input).unwrap_err();
        assert_eq!(err, "`relative/path` is not an absolute path");

        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(after_bytes, prior_bytes);
        assert_eq!(paths, before);
    }

    /// Non-existent absolute path → validator's "does not
    /// exist" message, settings.json untouched, original
    /// canonical_dir preserved.
    #[test]
    fn compose_apply_checkout_nonexistent_path_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_bytes = fs::read(&paths.settings_file).unwrap();
        let before = paths.clone();

        let missing = dir.path().join("does-not-exist");
        let err = compose_apply_checkout(&paths, &missing.to_string_lossy()).unwrap_err();
        assert!(
            err.contains("does not exist"),
            "validator message must surface verbatim, got: {err}"
        );

        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(after_bytes, prior_bytes);
        assert_eq!(paths, before);
    }

    /// Path exists but lacks an `agents/` subdir → validator's
    /// "does not exist" / "`agents/` does not exist" message,
    /// settings.json untouched, original canonical_dir
    /// preserved.
    #[test]
    fn compose_apply_checkout_missing_agents_subdir_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_bytes = fs::read(&paths.settings_file).unwrap();
        let before = paths.clone();

        // Directory exists, but has no `agents/` child.
        let bogus = dir.path().join("no-agents-child");
        fs::create_dir_all(&bogus).unwrap();
        let err = compose_apply_checkout(&paths, &bogus.to_string_lossy()).unwrap_err();
        assert!(
            err.contains("agents"),
            "validator must surface the missing agents/ message, got: {err}"
        );

        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(after_bytes, prior_bytes);
        assert_eq!(paths, before);
    }

    /// Write failure → `write settings: …` prefix surfaces
    /// verbatim, settings.json is byte-for-byte the prior file,
    /// and the caller's `Paths` is unchanged. We force the
    /// failure by making `paths.settings_file`'s parent a
    /// regular file: `save_settings` → `write_target` tries to
    /// `mkdir -p` that parent and fails on the non-directory.
    #[test]
    fn compose_apply_checkout_write_failure_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_bytes = fs::read(&paths.settings_file).unwrap();
        let before = paths.clone();

        // Replace `paths.settings_file` with a path whose
        // parent is a regular file, so `write_target`'s
        // `create_dir_all(parent)` cannot land the temp file.
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"not a directory").unwrap();
        let blocked_paths = Paths {
            settings_file: blocker.join("settings.json"),
            ..paths.clone()
        };

        let valid = dir.path().join("valid-checkout");
        fs::create_dir_all(valid.join("agents")).unwrap();
        let err = compose_apply_checkout(&blocked_paths, &valid.to_string_lossy()).unwrap_err();
        assert!(
            err.starts_with("write settings: "),
            "save error must carry the write settings: prefix, got: {err}"
        );

        // The original `paths.settings_file` on disk is still
        // the seeded prior file; the blocked file does not
        // exist either (mkdir failed before any byte hit disk).
        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(after_bytes, prior_bytes);
        assert!(!blocker.join("settings.json").exists());
        assert_eq!(paths, before);
    }

    /// Symlink at the root → validator refuses with a
    /// symlink-aware message, settings.json untouched, original
    /// canonical_dir preserved. Gated to Unix because the
    /// symlink primitives differ by platform.
    #[cfg(unix)]
    #[test]
    fn compose_apply_checkout_symlinked_root_is_rejected_byte_exact() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let prior = dir.path().join("prior-checkout");
        fs::create_dir_all(prior.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(prior.to_string_lossy().into_owned()),
        )
        .unwrap();
        let prior_bytes = fs::read(&paths.settings_file).unwrap();
        let before = paths.clone();

        let target = dir.path().join("target");
        fs::create_dir_all(target.join("agents")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = compose_apply_checkout(&paths, &link.to_string_lossy()).unwrap_err();
        assert!(
            err.contains("symlink"),
            "validator must surface the symlink refusal, got: {err}"
        );

        let after_bytes = fs::read(&paths.settings_file).unwrap();
        assert_eq!(after_bytes, prior_bytes);
        assert_eq!(paths, before);
    }
}
