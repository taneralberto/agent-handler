//! D1 spike: Tauri v2 + Angular prototype backend.
//!
//! Two `#[tauri::command]`s, both strictly read-only, both
//! wrapping the same pure composition helpers the unit tests
//! call directly:
//!
//! - `settings_status` → `compose_settings_status(&Paths)`
//! - `list_agents`     → `compose_agents_list(&Paths)`
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
//! - **No frontend input.** Commands take no arguments.
//! - **No `ensure_dirs`, no `save_*`, no `delete_*`.** The
//!   prototype never writes to the host filesystem from this
//!   surface.
//! - **No projection of prompt or permissions.** `AgentSummary`
//!   has exactly four fields; a unit test asserts the JSON
//!   serialization carries only those.
//! - **Single source of truth.** The lib's
//!   `workflows::read_checkout` and
//!   `workflows::list_canonical_agents` are reused verbatim.
//! - **No CLI coupling.** `agenthd gui` stays rejected.

use agenthd::agent::Mode as AgentMode;
use agenthd::store::Paths;
use agenthd::workflows::{list_canonical_agents, read_checkout, CheckoutStatus};
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

/// Pure composition helper: classify the configured checkout
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

/// Pure composition helper: classify the configured checkout
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![settings_status, list_agents])
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
        let keys: std::collections::BTreeSet<&str> =
            obj.keys().map(String::as_str).collect();
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
            obj.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
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
            obj.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
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
}
