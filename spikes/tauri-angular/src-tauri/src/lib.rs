//! D1 spike: Tauri v2 + Angular prototype backend.
//!
//! Commands for the Settings + Agents surfaces,
//! composed over the lib `agenthd`:
//!
//! - `settings_status` → `compose_settings_status(&Paths)`
//!   (read-only; classify the configured checkout).
//! - `list_agents`     → `compose_agents_list(&Paths)`
//!   (read-only; enumerate `agents/` and project to
//!   `AgentSummary`).
//! - `apply_checkout`  → `compose_apply_checkout(&Paths, &str)`
//!   (mutating small-synchronous slice; validate + persist +
//!   revalidate the configured checkout path).
//! - `load_agent_for_edit` →
//!   `compose_load_agent_for_edit(&Paths, &str)` (read-only;
//!   open an existing canonical agent for the GUI editor).
//! - `save_agent_edit` →
//!   `compose_save_agent_edit(&Paths, AgentEditContext,
//!   AgentEditDto)` (mutating small-synchronous slice; rewrite
//!   new or existing agent bytes through the same workflow the
//!   TUI's editor uses).
//! - `load_new_agent_for_edit` → root default material + new context (read-only).
//! - `delete_agent_edit` → guarded canonical-only deletion with captured hash.
//!
//! The editor supports canonical-only create, edit, rename and delete.
//! Rename preserves the root workflow's non-atomic partial failures,
//! without compensating rollback. `load_agent_for_edit` captures the
//! on-disk SHA-256 of the bytes the parser consumed and hands
//! it back to the frontend as `prior_hash`; `save_agent_edit`
//! re-validates the persisted checkout, the file's existence,
//! and the captured context before delegating to
//! `workflows::save_agent` so an external writer that slipped
//! in between open and save still surfaces the standard
//! "changed on disk" error.
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
//!   `workflows::list_canonical_agents`,
//!   `workflows::apply_checkout`, and
//!   `workflows::save_agent` are reused verbatim. The
//!   `ApplyError::message()` and `save_agent` errors are
//!   forwarded to the frontend unchanged so the TUI's error
//!   strings stay canonical.
//! - **Read-only list projection stays narrow.** `AgentSummary`
//!   has exactly four fields; the existing test pins the JSON
//!   payload carries only those. The editor DTO `AgentEditDto`
//!   is a separate type that carries every editable field
//!   (description, mode, model, prompt, permissions); the list
//!   summary never widens.
//! - **No `ensure_dirs`, no canonical/target/skills writes
//!   beyond what the workflow owns.** `apply_checkout` writes
//!   `settings.json` only (via `save_settings` →
//!   `write_target`); `save_agent_edit` writes the
//!   `<name>.md` it edited (via `save_agent` → `save_canonical`
//!   → `write_target`); no target-tree writes, no `state.json`
//!   mutation, no `skills_dir` write. The agenthd root and
//!   target trees are the boot path's responsibility.
//! - **No CLI coupling.** `agenthd gui` locates the companion
//!   binary (see root `src/main.rs`); the spike itself never
//!   reaches into CLI args.

use agenthd::agent::Mode as AgentMode;
use agenthd::store::Paths;
use agenthd::workflows::{
    apply_checkout as apply_checkout_workflow, list_canonical_agents, read_checkout, save_agent,
    CheckoutStatus,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// D3 GUI job registry. See `jobs.rs` for the
/// reservation model, the `CurrentView` snapshot, and the
/// `OperationRequest` wire shape. The command layer in
/// this file is a thin wrapper over the registry's
/// helpers; the registry owns all of the bookkeeping.
pub mod jobs;

use jobs::{
    cancel_operation as registry_cancel, current_operation as registry_current,
    guarded_short as registry_guarded_short, operation_status as registry_status,
    permission_keys as registry_permission_keys, production_emit_fn as registry_emit_fn,
    start_operation as registry_start, tool_catalog_status as registry_tool_status, CurrentView,
    LongOutcome, OperationError, OperationRegistry, OperationRequest,
};

/// Advisory read-only snapshot. `op_start` always replans; these rows are
/// never accepted as an executable plan or an overwrite authorization.
#[derive(Debug, Clone, Serialize)]
pub struct OperationPlan {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<jobs::SyncAgentTarget>,
    pub checkout_path: String,
    pub rows: Vec<PlanRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanRow {
    pub name: String,
    pub status: String,
    pub source_path: String,
    pub target_path: String,
    pub source_hash: Option<String>,
    pub target_hash: Option<String>,
    pub owned_hash: Option<String>,
    pub reason: Option<String>,
}

fn require_plan_request(request: &OperationRequest) -> Result<(), String> {
    match request {
        OperationRequest::SyncAgents { .. } | OperationRequest::InstallSkills => Ok(()),
        _ => Err("read-only plans support only sync_agents and install_skills".into()),
    }
}

pub fn compose_operation_plan(
    paths: &Paths,
    request: OperationRequest,
) -> Result<OperationPlan, String> {
    require_plan_request(&request)?;
    let ready =
        match read_checkout(&paths.settings_file).map_err(|e| format!("read settings: {e}"))? {
            CheckoutStatus::Ready(path) => path,
            other => {
                return Err(format!(
                    "cannot plan: configured checkout is {} (configure a checkout first)",
                    checkout_status_label(&other)
                ))
            }
        };
    let checkout_path = ready.to_string_lossy().into_owned();
    let scoped = paths
        .clone()
        .with_settings(&agenthd::store::Settings::new(checkout_path.clone()))
        .map_err(|e| format!("resolve configured checkout: {e}"))?;
    let state = agenthd::store::State::load(&scoped.state_file).map_err(|e| e.to_string())?;
    let (kind, target, rows) = match request {
        OperationRequest::SyncAgents { target } => {
            let rows = agenthd::store::plan_for(&scoped, &state, target.as_lib_target())
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|item| PlanRow {
                    reason: Some(item.reason()),
                    name: item.filename,
                    status: item.status.label().into(),
                    source_path: item.canonical_path.to_string_lossy().into_owned(),
                    target_path: item.target_path.to_string_lossy().into_owned(),
                    source_hash: item.canonical_hash,
                    target_hash: item.target_hash,
                    owned_hash: item.last_installed_hash,
                })
                .collect();
            ("sync_agents", Some(target), rows)
        }
        OperationRequest::InstallSkills => {
            let rows = agenthd::store::plan_skills(&scoped, &state)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|item| PlanRow {
                    name: item.name,
                    status: item.action.label().into(),
                    source_path: item.source_path.to_string_lossy().into_owned(),
                    target_path: item.target_path.to_string_lossy().into_owned(),
                    source_hash: item.source_tree_hash,
                    target_hash: item.target_tree_hash,
                    owned_hash: item.owned_tree_hash,
                    reason: None,
                })
                .collect();
            ("install_skills", None, rows)
        }
        _ => unreachable!("request validated before reading paths"),
    };
    Ok(OperationPlan {
        kind: kind.into(),
        target,
        checkout_path,
        rows,
    })
}

#[tauri::command]
fn operation_plan(request: OperationRequest) -> Result<OperationPlan, String> {
    // Unsupported requests must not resolve the host environment at all.
    require_plan_request(&request)?;
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    compose_operation_plan(&paths, request)
}

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

/// Editor envelope: the data the GUI editor must round-trip on
/// every save. Bundling the open-time context with the edited
/// material keeps the frontend stateless — the GUI never has
/// to remember `prior_hash` across pages, only include it in
/// the next save. Every field on this struct has a meaning:
///
/// - `checkout_path` is the absolute configured checkout that
///   was `Ready` when the editor opened. `compose_save_agent_edit`
///   re-reads `settings.json` and refuses any save that targets
///   a different checkout (or a checkout that no longer
///   validates); without that check, a draft the user started
///   against checkout A could be saved into checkout B if the
///   user swapped configurations in another tab.
/// - `original_name` is the agent's name when the editor opened,
///   even when the draft is renamed. New agents have both
///   `original_name` and `prior_hash` set to None (JSON null).
/// - `prior_hash` is the lowercase hex SHA-256 of the canonical
///   bytes the parser consumed when the editor opened. The
///   lib's `save_canonical` re-hashes the file on save and
///   refuses to write if the live hash differs — the editor
///   just hands that captured hash through, so the same
///   "changed on disk" error the TUI surfaces reaches the
///   GUI verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEditContext {
    pub checkout_path: String,
    pub original_name: Option<String>,
    pub prior_hash: Option<String>,
}

/// Editor DTO: every editable field of an `agenthd::agent::Agent`.
/// Carries `prompt` and `permissions` (unlike the read-only
/// `AgentSummary`), since the editor needs both to render and
/// to write back. Mirrors the `Agent` shape one-for-one so the
/// frontend never has to translate between wire types and
/// canonical types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEditDto {
    pub name: String,
    pub description: String,
    pub mode: String,
    pub model: Option<String>,
    pub prompt: String,
    /// String-keyed permissions map. Unknown keys are preserved
    /// verbatim on round-trip; the lib rejects unknown keys at
    /// `save_canonical` time, so the editor stays defensible
    /// without a parallel vocabulary list.
    pub permissions: std::collections::BTreeMap<String, String>,
}

/// Open an existing canonical agent for the GUI editor. Reuses
/// `agenthd::store::load_agent_for_edit` so the parser, the
/// fail-closed canonical-source gate, the symlink rejection,
/// and the single-read / single-hash contract are identical
/// to what the lib tests already pin. Wraps the lib result in
/// the editor DTO + context pair the GUI round-trips.
///
/// **P1 runtime guard:** the `paths` argument arrives from
/// the command's `Paths::from_env()` (or a test's
/// `Paths::resolve(...)`) with `canonical_dir` pointing at
/// the historical default `<agenthd_root>/agents`. The
/// helper MUST NOT use that default to look up the agent —
/// if it did, an editor open against a configured checkout
/// would silently read from the agenthd-root agents (or
/// fail with the wrong error) and could later write back
/// there. The same fix the `compose_agents_list` helper
/// already uses is applied here: resolve `Ready` once,
/// derive a scoped `Paths::clone().with_settings(...)`, and
/// use that scoped value for the canonical read. The
/// context's `checkout_path` is the wire guard the save
/// side re-checks; it never drives a directory.
///
/// On success: returns the editable fields the editor renders
/// AND the `AgentEditContext` (open-time `checkout_path`,
/// `original_name`, `prior_hash`) the GUI must include on the
/// subsequent save. The error arm surfaces the lib's `anyhow`
/// text verbatim — the same string the TUI's editor produces
/// in the same conditions.
pub fn compose_load_agent_for_edit(
    paths: &Paths,
    name: &str,
) -> Result<(AgentEditDto, AgentEditContext), String> {
    // Resolve `Ready` from the persisted settings file. We
    // resolve from `paths.settings_file` directly (the same
    // call `compose_apply_checkout` ultimately persists
    // through) rather than going through
    // `workflows::list_canonical_agents`, because that
    // workflow would silently fall back to a never-
    // reconfigured `paths.canonical_dir` when `settings.json`
    // is empty. The editor MUST have a checkout the user
    // picked; opening an empty settings is an error here,
    // not a phantom read into the local default.
    let ready_path = match read_checkout(&paths.settings_file) {
        Ok(CheckoutStatus::Ready(path)) => path,
        Ok(other) => {
            return Err(format!(
                "cannot open editor: configured checkout is {} (configure a checkout first)",
                checkout_status_label(&other)
            ));
        }
        Err(err) => return Err(format!("read settings: {err}")),
    };

    // Derive a scoped `Paths` whose `canonical_dir` points at
    // the configured checkout's `agents/`. Every canonical
    // operation in this helper (and its save counterpart) is
    // run against this scoped value; the input `paths`
    // `canonical_dir` is intentionally left untouched so a
    // caller holding the original `Paths` still observes its
    // original canonical-dir value.
    let settings = agenthd::store::Settings::new(ready_path.to_string_lossy().into_owned());
    let scoped = paths
        .clone()
        .with_settings(&settings)
        .map_err(|e| format!("resolve configured checkout: {e}"))?;

    let (agent, prior_hash) =
        agenthd::store::load_agent_for_edit(&scoped, name).map_err(|e| e.to_string())?;

    let context = AgentEditContext {
        checkout_path: ready_path.to_string_lossy().into_owned(),
        original_name: Some(agent.name.clone()),
        prior_hash: Some(prior_hash),
    };
    let dto = project_agent_to_dto(agent);
    Ok((dto, context))
}

/// Short, single-word labels for `CheckoutStatus` non-Ready arms.
/// Kept local to this helper so the wire message does not embed
/// the full `SettingsStatus` discriminator shape the
/// `settings_status` command returns.
fn checkout_status_label(status: &CheckoutStatus) -> &'static str {
    match status {
        CheckoutStatus::Empty => "empty",
        CheckoutStatus::Stale { .. } => "stale",
        CheckoutStatus::Ready(_) => "ready",
    }
}

/// Save a new or edited canonical agent. The frontend sends the
/// `AgentEditContext` it received on `load_agent_for_edit`
/// (verbatim) plus the edited `AgentEditDto`. The helper
/// re-reads `settings.json`, refuses any drift, refuses a
/// missing canonical file (this slice does NOT allow edit
/// to silently become a create), and delegates to
/// `workflows::save_agent` so the TUI's stale-write error
/// reaches the GUI verbatim.
///
/// **P1 runtime guard:** mirrors the fix in
/// `compose_load_agent_for_edit`. The `paths` argument's
/// `canonical_dir` is the historical default
/// `<agenthd_root>/agents`; the helper resolves `Ready`
/// itself, derives a scoped `Paths::clone().with_settings(...)`,
/// and uses that scoped value for the canonical-exists
/// check AND for `workflows::save_agent`. The input
/// `paths` is never used to compute a write target. The
/// context `checkout_path` is a wire guard only (refused
/// if it does not match the persisted Ready path), never
/// a directory driver — the scoped `Paths` owns that.
///
/// Rename uses the root workflow's non-atomic behavior without rollback.
/// On success: returns the saved `name`. On
/// failure: returns the lib's error text verbatim. The GUI
/// preserves its draft + context on failure (the frontend
/// owns that policy); this helper does not retry, refresh,
/// or mutate state on either branch.
pub fn compose_save_agent_edit(
    paths: &Paths,
    context: AgentEditContext,
    agent_edit: AgentEditDto,
) -> Result<String, String> {
    let scoped = scoped_edit_paths(paths, &context)?;
    if let Some(name) = &context.original_name {
        let canonical_path = agenthd::agent::canonical_path(&scoped.canonical_dir, name)
            .map_err(|e| e.to_string())?;
        if !canonical_path.exists() {
            return Err(format!(
                "refusing to save: `{}` no longer exists on disk; re-open the editor after confirming the canonical source",
                canonical_path.display()
            ));
        }
    }
    let material = dto_into_agent(agent_edit)?;
    save_agent(&scoped, context.original_name, context.prior_hash, material)
        .map_err(|e| e.to_string())
}

/// Resolve the persisted checkout, never a directory supplied by the editor.
fn scoped_edit_paths(paths: &Paths, context: &AgentEditContext) -> Result<Paths, String> {
    // Empty-context check: the frontend should never call save
    // without a context, but a defensive check here keeps a
    // buggy frontend from sneaking past the ready-checkout
    // gate below. The literal text matches the canonical
    // "must not be empty" messages the lib already produces
    // for the editor's other fields.
    if context.checkout_path.trim().is_empty()
        || !matches!(
            (&context.original_name, &context.prior_hash),
            (None, None) | (Some(_), Some(_))
        )
        || context
            .original_name
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
        || context
            .prior_hash
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
    {
        return Err(
            "edit context is incomplete (checkout_path is required; original_name and prior_hash must both be null or nonempty)"
                .to_string(),
        );
    }

    // Re-read the persisted checkout. The save MUST target the
    // same checkout the editor opened against — otherwise a
    // concurrent `apply_checkout` in another tab could land a
    // draft into a checkout the user no longer means to edit,
    // producing an agent with a name that already exists in
    // the other repo (the homonymous-agent-in-another-repo
    // scenario the spike is closing off in this block). The
    // `Ready` path we resolve here is what we then scope our
    // own canonical operations against; it is the directory
    // authority, not the input `paths` `canonical_dir` and
    // not the `context.checkout_path`.
    let ready_path = match read_checkout(&paths.settings_file)
        .map_err(|e| format!("read settings: {e}"))?
    {
        CheckoutStatus::Ready(path) => path,
        other => {
            return Err(format!(
                "refusing to save: configured checkout is {}; open the editor again after fixing settings",
                checkout_status_label(&other)
            ));
        }
    };
    let current_checkout = ready_path.to_string_lossy().into_owned();
    if current_checkout != context.checkout_path {
        return Err(format!(
            "refusing to save: configured checkout changed from `{}` to `{}`; reload the editor",
            context.checkout_path, current_checkout
        ));
    }

    // Derive the scoped `Paths` whose `canonical_dir` points
    // at the configured checkout's `agents/`. Every canonical
    // operation below uses this value.
    let settings = agenthd::store::Settings::new(current_checkout.clone());
    paths
        .clone()
        .with_settings(&settings)
        .map_err(|e| format!("resolve configured checkout: {e}"))
}

/// New material comes from the root defaults; opening it writes nothing.
pub fn compose_load_new_agent_for_edit(
    paths: &Paths,
    name: &str,
) -> Result<(AgentEditDto, AgentEditContext), String> {
    let ready_path =
        match read_checkout(&paths.settings_file).map_err(|e| format!("read settings: {e}"))? {
            CheckoutStatus::Ready(path) => path,
            other => {
                return Err(format!(
                    "cannot open editor: configured checkout is {}",
                    checkout_status_label(&other)
                ))
            }
        };
    let context = AgentEditContext {
        checkout_path: ready_path.to_string_lossy().into_owned(),
        original_name: None,
        prior_hash: None,
    };
    scoped_edit_paths(paths, &context)?;
    let agent = agenthd::agent::Agent::new_default(name.to_string()).map_err(|e| e.to_string())?;
    Ok((project_agent_to_dto(agent), context))
}

/// Delete only canonical material, after verifying the captured single-read hash.
pub fn compose_delete_agent_edit(paths: &Paths, context: AgentEditContext) -> Result<(), String> {
    let scoped = scoped_edit_paths(paths, &context)?;
    let name = context
        .original_name
        .as_deref()
        .ok_or("cannot delete an unsaved new agent")?;
    agenthd::agent::Agent::validate_name(name).map_err(|e| e.to_string())?;
    let canonical_path =
        agenthd::agent::canonical_path(&scoped.canonical_dir, name).map_err(|e| e.to_string())?;
    // symlink_metadata distinguishes a dangling symlink from an absent file.
    match std::fs::symlink_metadata(&canonical_path) {
        Ok(_) => {
            let (_, hash) =
                agenthd::store::load_agent_for_edit(&scoped, name).map_err(|e| e.to_string())?;
            if Some(hash.as_str()) != context.prior_hash.as_deref() {
                return Err(format!("`{}` changed on disk since this edit started; reload to pick up the latest version", canonical_path.display()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    agenthd::store::delete_canonical(&scoped, name).map_err(|e| e.to_string())
}

/// `AgentEditDto` → `agenthd::agent::Agent`. Centralised so the
/// wire-shape → canonical-shape mapping is testable in
/// isolation (see `dto_round_trip_preserves_every_field`) and
/// so the helper stays readable. Returns `Result<_, String>`
/// directly so the helper does not need an extra
/// `anyhow`-style dependency in the spike's `Cargo.toml`.
fn dto_into_agent(dto: AgentEditDto) -> Result<agenthd::agent::Agent, String> {
    use agenthd::agent::{Agent, Mode, PermissionAction};
    let mode = match dto.mode.as_str() {
        "subagent" => Mode::subagent,
        "primary" => Mode::primary,
        "all" => Mode::all,
        other => {
            return Err(format!(
                "invalid mode `{other}` (expected subagent, primary, or all)"
            ));
        }
    };
    let mut permissions = std::collections::BTreeMap::new();
    for (key, value) in dto.permissions {
        let action = match value.as_str() {
            "allow" => PermissionAction::Allow,
            "ask" => PermissionAction::Ask,
            "deny" => PermissionAction::Deny,
            other => {
                return Err(format!(
                    "invalid permission action `{other}` for `{key}` (expected allow, ask, or deny)"
                ));
            }
        };
        permissions.insert(key, action);
    }
    Ok(Agent {
        name: dto.name,
        description: dto.description,
        mode,
        model: dto.model,
        prompt: dto.prompt,
        permissions,
    })
}

/// `agenthd::agent::Agent` → `AgentEditDto`. Mirrors
/// `dto_into_agent`; round-trip is asserted by
/// `dto_round_trip_preserves_every_field`.
fn project_agent_to_dto(a: agenthd::agent::Agent) -> AgentEditDto {
    AgentEditDto {
        name: a.name,
        description: a.description,
        mode: a.mode.as_str().to_string(),
        model: a.model,
        prompt: a.prompt,
        permissions: a
            .permissions
            .into_iter()
            .map(|(k, v)| (k, v.as_str().to_string()))
            .collect(),
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
///
/// **D3 short-sync guard:** the command acquires the registry's
/// short reservation before calling `compose_apply_checkout`; a
/// long job in flight (or another short sync running) causes
/// `OperationError::Busy`. The reservation is held only for
/// the synchronous call. `Result<String, String>` is preserved
/// (Busy surfaces as `String` here so the frontend's `catch`
/// can render it without a schema break) — the
/// `OperationError::Busy` text is the lib-style message the
/// rest of the slice already renders.
#[tauri::command]
fn apply_checkout(
    checkout_path: String,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<String, String> {
    // **Env-read inside the guarded closure.** Pre-fix,
    // `Paths::from_env()` ran before the reservation was
    // acquired, so a slow / failing env read could block
    // the window's close decision and never released a
    // reservation (the busy guard ran but the env read
    // was outside it). Reading env INSIDE the closure
    // keeps the busy guard covering the full path, so a
    // `apply_checkout` that races with a long job is
    // surfaced as `OperationError::Busy` cleanly and the
    // close handler never sees a stale busy state.
    registry_guarded_short(state.inner(), checkout_path, |input| {
        let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
        compose_apply_checkout(&paths, &input)
    })
    .map_err(|e| format!("{e}"))
}

/// Open an existing canonical agent for the GUI editor. See
/// `compose_load_agent_for_edit` for the contract. Returns the
/// editable fields + the `AgentEditContext` (open-time
/// `checkout_path`, `original_name`, `prior_hash`) the GUI
/// must round-trip into `save_agent_edit`. Errors land in the
/// JS rejection verbatim.
#[tauri::command]
fn load_agent_for_edit(name: String) -> Result<AgentEditLoadResponse, String> {
    let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
    let (agent, context) = compose_load_agent_for_edit(&paths, &name)?;
    Ok(AgentEditLoadResponse { agent, context })
}

#[tauri::command]
fn load_new_agent_for_edit(name: String) -> Result<AgentEditLoadResponse, String> {
    let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
    let (agent, context) = compose_load_new_agent_for_edit(&paths, &name)?;
    Ok(AgentEditLoadResponse { agent, context })
}

#[tauri::command]
fn delete_agent_edit(
    context: AgentEditContext,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<(), String> {
    registry_guarded_short(state.inner(), context, |context| {
        let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
        compose_delete_agent_edit(&paths, context).map(|()| String::new())
    })
    .map(|_| ())
    .map_err(|e| format!("{e}"))
}

/// Wrapper struct so the frontend receives the editor DTO and
/// its context under a single top-level key. Keeps the wire
/// shape flat: `{ "agent": {...}, "context": {...} }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEditLoadResponse {
    pub agent: AgentEditDto,
    pub context: AgentEditContext,
}

/// Save canonical material. See `compose_save_agent_edit`
/// for context re-validation and file-still-exists checks.
/// Returns `Ok(name)` on success and forwards the
/// lib's error text on failure.
///
/// **D3 short-sync guard:** the command acquires the registry's
/// short reservation before calling `compose_save_agent_edit`;
/// a long job in flight (or another short sync running) causes
/// `OperationError::Busy`. The reservation is held only for
/// the synchronous call. `Result<String, String>` is preserved
/// for the frontend's `catch`.
#[tauri::command]
fn save_agent_edit(
    context: AgentEditContext,
    agent: AgentEditDto,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<String, String> {
    // **Env-read inside the guarded closure.** Mirrors
    // `apply_checkout`: the `Paths::from_env()` call lives
    // inside the `guarded_short` closure so the busy
    // guard covers the env read, not just the helper.
    // A `save_agent_edit` that races with a long job
    // surfaces `OperationError::Busy` and the close
    // handler never sees a stale busy state.
    registry_guarded_short(state.inner(), (context, agent), |(context, agent)| {
        let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
        compose_save_agent_edit(&paths, context, agent)
    })
    .map_err(|e| format!("{e}"))
}

// ===========================================================================
// D3 long-job command surface
//
// The `start_operation` / `current_operation` /
// `operation_status` / `cancel_operation` commands are the
// D3 GUI's view of the long-running workflows the lib
// already exposes. The registry in `jobs.rs` is the
// single source of truth; the commands here are thin
// wrappers that resolve `Paths` + `State` from the env,
// hand them to the registry, and forward the result to
// the frontend. The worker thread the registry spawns
// emits the `agenthd-operation` event (see
// `jobs::OPERATION_EVENT`) outside the registry lock; the
// frontend listens for that name and reconciles the
// visible state.
//
// The registry is shared via `tauri::Builder::manage` so
// the Tauri command layer can hand `app.state()` to each
// command and the worker threads can take an `Arc` clone
// from the same `Manage` cell. Tests do not exercise this
// path (they call the registry helpers directly with a
// `None` `AppHandle`); the production wiring is the only
// place the `AppHandle` ever reaches the registry.
// ===========================================================================

/// `op_start(request)` — D3 long-running job entry
/// point. The frontend calls this for the
/// `SyncAgents` / `InstallSkills` / `InstallTool` /
/// `DiscoverModels` buttons; the registry either accepts
/// the request and returns the new job id + the initial
/// `CurrentView`, or refuses with `OperationError::Busy`
/// (a long job is already in flight or a short sync is
/// running). The frontend reconciles the running state
/// through the `agenthd-operation` event stream;
/// `op_current` is the recovery entry point for any
/// missed event.
///
/// **Runtime-path contract:** paths / state resolution
/// runs INSIDE the accepted worker (after the
/// reservation), not here. Reading env / settings /
/// state BEFORE the reservation would let a slow /
/// failing env read hold a reservation; reading it
/// AFTER keeps the registry responsive to close /
/// status calls during the resolve. The resolver is the
/// factory in `jobs::production_request_resolve_paths_state`.
#[tauri::command]
fn op_start(
    request: OperationRequest,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<LongOutcome, OperationError> {
    let emit = registry_emit_fn(app);
    registry_start(Arc::clone(state.inner()), emit, request)
}

/// `op_current()` — read the latest `CurrentView` the
/// registry knows about. Always returns a `CurrentView`
/// (never `null`); the freshly-built registry returns
/// `{seq:"0",job:null}`. The frontend uses this call to
/// seed its reducer on the first `listen` registration,
/// and as the recovery fallback when an event is
/// missed.
#[tauri::command]
fn op_current(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> CurrentView {
    let emit = registry_emit_fn(app);
    registry_current(state.inner(), &emit)
}

/// `op_status(job_id)` — read the latest `CurrentView`
/// for a specific job id. The recovery entry point for
/// missed terminals: the frontend can re-fetch a
/// terminal at any time during / after the job,
/// regardless of whether the corresponding
/// `agenthd-operation` event was received. Unknown ids
/// surface as `OperationError::UnknownJob`.
#[tauri::command]
fn op_status(
    job_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<CurrentView, OperationError> {
    let emit = registry_emit_fn(app);
    registry_status(state.inner(), &emit, &job_id)
}

/// `op_cancel(job_id)` — request cancellation of an
/// active long job. Returns `Ok(CurrentView)` when the
/// cancel is in flight (the worker reports the cancel
/// at the next safe checkpoint); the same idempotent
/// `Ok(CurrentView)` when the id matches a terminal
/// snapshot the registry is retaining; and
/// `Err(OperationError::UnknownJob)` when the id is
/// unknown or expired. Cancellation is NEVER an error
/// in this slice — the frontend does not need to
/// distinguish "active cancel" from "already settled".
#[tauri::command]
fn op_cancel(
    job_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<OperationRegistry>>,
) -> Result<CurrentView, OperationError> {
    let emit = registry_emit_fn(app);
    registry_cancel(state.inner(), &emit, &job_id)
}

/// `tool_catalog_status()` — backend-authoritative
/// catalog of installable entries from
/// `agenthd::tools::DEFAULT_CATALOG`, each row
/// (`tool_id` / `display` / `status` / `detail` /
/// `destination`) computed by
/// `agenthd::tools::tool_status(&paths, entry)`. The
/// frontend does NOT maintain a parallel catalog list;
/// a future change to the lib's `DEFAULT_CATALOG` is
/// picked up automatically.
///
/// **Result, not `Vec<ToolItem>`.** Pre-fix, a `None`
/// env fell through to `Paths::resolve(None, None).unwrap()`
/// — a panic surface for any caller whose env was
/// invalid (CI, tests). Pre-fix, a single bad catalog
/// entry returned a synthetic `Conflict` row that
/// silently invented a non-`Ok` status. The fixed
/// helper returns `Result<ToolStatusList, String>` so
/// the frontend receives an explicit `{ rows, error }`
/// payload — no panic, no fabricated status.
#[tauri::command]
fn tool_catalog_status() -> Result<jobs::ToolStatusList, String> {
    let paths = Paths::from_env().map_err(|e| format!("resolve config paths: {e}"))?;
    let rows = registry_tool_status(&paths).map_err(|e| e.to_string())?;
    Ok(jobs::ToolStatusList {
        rows: rows.into_iter().map(jobs::ToolStatusRow::from).collect(),
    })
}

/// `permission_keys()` — backend-authoritative list of
/// supported permission keys (mirrors
/// `agenthd::agent::PERMISSION_KEYS`). The frontend
/// renders the editor's known-key dropdown from this
/// list so a future change to the lib's vocabulary
/// does not require a parallel frontend update.
#[tauri::command]
fn permission_keys() -> jobs::PermissionKeys {
    registry_permission_keys()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The D3 registry is process-global. The Tauri
    // command layer hands `app.state()` to each
    // command; the worker threads inside `start_operation`
    // take an `Arc` clone from the same `Manage` cell so
    // the registry stays a single source of truth across
    // the Tauri runtime.
    let registry = Arc::new(OperationRegistry::new());
    let registry_for_event = Arc::clone(&registry);
    tauri::Builder::default()
        .manage(registry)
        .on_window_event(move |window, event| {
            use tauri::Manager as _;
            // `CloseRequested` is gated by the
            // registry's reservation: a long job in
            // flight or a short sync running blocks the
            // window's close so the user cannot yank the
            // runtime while state is in flight.
            //
            // Pre-fix, the handler only called
            // `should_block_close` + `api.prevent_close`.
            // That prevented the yank but never asked the
            // registry to cancel the in-flight job, so the
            // worker kept producing progress events the
            // user never saw because the window was
            // already detached. The fixed handler routes
            // through `handle_close_requested`, which under
            // ONE lock: reads the reservation, requests
            // cancellation of the active long job (when
            // one is in flight), bumps the seq ONCE,
            // installs a sticky `cancel_requested` flag
            // on the latest snapshot, and returns the
            // (should_block, view) pair the wiring uses.
            // The view is emitted outside the lock so the
            // frontend sees the cancel-requested state
            // before the worker settles.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let (should_block, view) = registry_for_event.handle_close_requested();
                if should_block {
                    api.prevent_close();
                    eprintln!(
                        "agenthd-gui: close requested while a job is active; \
                         blocked until the registry settles"
                    );
                }
                // The emit closure requires a real
                // `AppHandle<R>` (the previous
                // `Option`-shaped factory was a silent
                // no-op that never reached the
                // frontend). The window's
                // `Manager::app_handle()` is the typed
                // accessor that produces one; we clone it
                // because `app_handle` returns a borrow.
                if let Some(view) = view {
                    let emit = registry_emit_fn(window.app_handle().clone());
                    emit(&view);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            settings_status,
            list_agents,
            apply_checkout,
            load_agent_for_edit,
            load_new_agent_for_edit,
            delete_agent_edit,
            save_agent_edit,
            op_start,
            operation_plan,
            op_current,
            op_status,
            op_cancel,
            tool_catalog_status,
            permission_keys
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

    fn plan_fixture(dir: &TempDir) -> (Paths, std::path::PathBuf) {
        let paths = paths_in(dir);
        let checkout = dir.path().join("persisted-checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        fs::write(
            checkout.join("agents/scout.md"),
            make_agent("scout", AgentMode::subagent).render(),
        )
        .unwrap();
        fs::create_dir_all(checkout.join("skills/example")).unwrap();
        fs::write(
            checkout.join("skills/example/SKILL.md"),
            "---\nname: example\ndescription: Example skill\n---\nbody\n",
        )
        .unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        write_md(
            &paths.canonical_dir,
            "decoy",
            "invalid decoy must not be read",
        );
        (paths, checkout)
    }

    fn plan_tree(path: &std::path::Path) -> BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
        let mut entries = BTreeMap::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                entries.insert(path.clone(), None);
                entries.extend(plan_tree(&path));
            } else {
                entries.insert(path.clone(), Some(fs::read(path).unwrap()));
            }
        }
        entries
    }

    #[test]
    fn operation_plan_raw_paths_scope_persisted_checkout_and_never_write() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = plan_fixture(&dir);
        let before = plan_tree(dir.path());
        for target in [jobs::SyncAgentTarget::Opencode, jobs::SyncAgentTarget::Pi] {
            let plan =
                compose_operation_plan(&paths, OperationRequest::SyncAgents { target }).unwrap();
            assert_eq!(plan.checkout_path, checkout.to_string_lossy());
            assert_eq!(plan.rows.len(), 1);
            let row = &plan.rows[0];
            assert_eq!(row.name, "scout.md");
            assert_eq!(row.status, "not installed");
            assert_eq!(
                row.source_path,
                checkout.join("agents/scout.md").to_string_lossy()
            );
            let dest = match target {
                jobs::SyncAgentTarget::Opencode => &paths.target_dir,
                jobs::SyncAgentTarget::Pi => &paths.pi_target_dir,
            };
            assert_eq!(row.target_path, dest.join("scout.md").to_string_lossy());
            assert!(row.source_hash.is_some());
            assert!(row.reason.is_some());
        }
        let skills = compose_operation_plan(&paths, OperationRequest::InstallSkills).unwrap();
        assert_eq!(skills.rows.len(), 1);
        assert_eq!(skills.rows[0].name, "example");
        assert_eq!(skills.rows[0].status, "install");
        assert_eq!(
            skills.rows[0].source_path,
            checkout.join("skills/example").to_string_lossy()
        );
        assert_eq!(
            skills.rows[0].target_path,
            paths.skills_dir.join("example").to_string_lossy()
        );
        assert!(skills.rows[0].source_hash.is_some());
        assert_eq!(plan_tree(dir.path()), before);
        assert_ne!(paths.canonical_dir, checkout.join("agents"));
    }

    #[test]
    fn operation_plan_per_target_inventory_uses_root_status_and_reason() {
        let dir = TempDir::new().unwrap();
        let (paths, _) = plan_fixture(&dir);
        fs::create_dir_all(&paths.target_dir).unwrap();
        fs::create_dir_all(&paths.pi_target_dir).unwrap();
        write_md(&paths.target_dir, "scout", "external modification");
        write_md(&paths.target_dir, "opencode-only", "unowned");
        write_md(&paths.pi_target_dir, "pi-only", "unowned");
        write_md(&paths.pi_target_dir, "removed", "owned removed");
        let mut state = agenthd::store::State::default();
        state.pi_installed.insert(
            "removed.md".into(),
            agenthd::store::hash_file(&paths.pi_target_dir.join("removed.md"))
                .unwrap()
                .unwrap(),
        );
        fs::write(&paths.state_file, serde_json::to_vec(&state).unwrap()).unwrap();
        let before = plan_tree(dir.path());
        for target in [jobs::SyncAgentTarget::Opencode, jobs::SyncAgentTarget::Pi] {
            let plan =
                compose_operation_plan(&paths, OperationRequest::SyncAgents { target }).unwrap();
            let scoped = paths
                .clone()
                .with_settings(
                    &agenthd::store::load_settings(&paths.settings_file)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
            let expected =
                agenthd::store::plan_for(&scoped, &state, target.as_lib_target()).unwrap();
            assert_eq!(plan.rows.len(), expected.len());
            for (row, item) in plan.rows.iter().zip(expected) {
                assert_eq!(row.name, item.filename);
                assert_eq!(row.status, item.status.label());
                assert_eq!(row.reason, Some(item.reason()));
            }
            let own = if target == jobs::SyncAgentTarget::Pi {
                "pi-only.md"
            } else {
                "opencode-only.md"
            };
            let other = if target == jobs::SyncAgentTarget::Pi {
                "opencode-only.md"
            } else {
                "pi-only.md"
            };
            assert!(plan
                .rows
                .iter()
                .any(|r| r.name == own && r.status == "unowned"));
            assert!(!plan.rows.iter().any(|r| r.name == other));
            if target == jobs::SyncAgentTarget::Pi {
                assert!(plan
                    .rows
                    .iter()
                    .any(|r| r.name == "removed.md" && r.status == "remove"));
            } else {
                assert!(plan
                    .rows
                    .iter()
                    .any(|r| r.name == "scout.md" && r.status == "conflict"));
            }
        }
        assert_eq!(plan_tree(dir.path()), before);
    }

    #[test]
    fn operation_plan_missing_ready_and_malformed_state_fail_closed() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        assert!(
            compose_operation_plan(&paths, OperationRequest::InstallSkills)
                .unwrap_err()
                .contains("configure a checkout first")
        );
        assert!(!paths.agenthd_root.exists());
        let (paths, checkout) = plan_fixture(&dir);
        fs::write(&paths.state_file, "malformed state").unwrap();
        let before = plan_tree(dir.path());
        for request in [
            OperationRequest::InstallSkills,
            OperationRequest::SyncAgents {
                target: jobs::SyncAgentTarget::Pi,
            },
        ] {
            assert!(compose_operation_plan(&paths, request)
                .unwrap_err()
                .contains("malformed"));
        }
        assert_eq!(plan_tree(dir.path()), before);
        fs::remove_dir_all(checkout.join("agents")).unwrap();
        assert!(
            compose_operation_plan(&paths, OperationRequest::InstallSkills)
                .unwrap_err()
                .contains("configure a checkout first")
        );
    }

    #[test]
    fn operation_plan_unsupported_requests_reject_before_paths_or_env() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        for request in [
            OperationRequest::DiscoverModels,
            OperationRequest::InstallTool {
                tool_id: "anything".into(),
            },
        ] {
            assert_eq!(
                compose_operation_plan(&paths, request.clone()).unwrap_err(),
                require_plan_request(&request).unwrap_err()
            );
            assert_eq!(
                operation_plan(request.clone()).unwrap_err(),
                require_plan_request(&request).unwrap_err()
            );
        }
        assert!(plan_tree(dir.path()).is_empty());
    }

    #[test]
    fn operation_plan_skills_inventory_retains_root_owned_removal_statuses() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = plan_fixture(&dir);
        let scoped = paths
            .clone()
            .with_settings(&Settings::new(checkout.to_string_lossy().into_owned()))
            .unwrap();
        let plan = agenthd::store::plan_skills(&scoped, &agenthd::store::State::default()).unwrap();
        let hash = plan[0].source_tree_hash.clone().unwrap();
        let mut state = agenthd::store::State::default();
        for name in ["removed", "modified", "ghost"] {
            state.installed_skills.insert(
                name.into(),
                agenthd::store::OwnedSkill {
                    tree_hash: hash.clone(),
                    skill_name: "example".into(),
                },
            );
        }
        fs::create_dir_all(paths.skills_dir.join("removed")).unwrap();
        fs::copy(
            checkout.join("skills/example/SKILL.md"),
            paths.skills_dir.join("removed/SKILL.md"),
        )
        .unwrap();
        fs::create_dir_all(paths.skills_dir.join("modified")).unwrap();
        fs::write(
            paths.skills_dir.join("modified/SKILL.md"),
            "external modification",
        )
        .unwrap();
        fs::write(&paths.state_file, serde_json::to_vec(&state).unwrap()).unwrap();
        let before = plan_tree(dir.path());
        let inventory = compose_operation_plan(&paths, OperationRequest::InstallSkills).unwrap();
        for (name, label) in [
            ("example", "install"),
            ("removed", "remove"),
            ("modified", "preserve modified"),
            ("ghost", "ghost"),
        ] {
            assert!(
                inventory
                    .rows
                    .iter()
                    .any(|row| row.name == name && row.status == label),
                "{name}: {label}"
            );
        }
        assert_eq!(plan_tree(dir.path()), before);
    }

    #[test]
    fn operation_plan_ready_empty_inventory_is_valid_not_an_error() {
        let dir = TempDir::new().unwrap();
        let (paths, checkout) = plan_fixture(&dir);
        fs::remove_file(checkout.join("agents/scout.md")).unwrap();
        fs::remove_dir_all(checkout.join("skills/example")).unwrap();
        for request in [
            OperationRequest::InstallSkills,
            OperationRequest::SyncAgents {
                target: jobs::SyncAgentTarget::Opencode,
            },
        ] {
            assert!(compose_operation_plan(&paths, request)
                .unwrap()
                .rows
                .is_empty());
        }
        fs::remove_dir(checkout.join("skills")).unwrap();
        assert!(
            compose_operation_plan(&paths, OperationRequest::InstallSkills)
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn operation_plan_wire_and_command_registration() {
        let dir = TempDir::new().unwrap();
        let (paths, _) = plan_fixture(&dir);
        for wire in [
            r#"{"kind":"sync_agents","target":"opencode"}"#,
            r#"{"kind":"sync_agents","target":"pi"}"#,
            r#"{"kind":"install_skills"}"#,
        ] {
            let request: OperationRequest = serde_json::from_str(wire).unwrap();
            let plan = compose_operation_plan(&paths, request.clone()).unwrap();
            let json = serde_json::to_value(plan).unwrap();
            let req = serde_json::to_value(request).unwrap();
            assert_eq!(json["kind"], req["kind"]);
            assert_eq!(json.get("target"), req.get("target"));
            assert!(json["rows"].is_array());
            assert!(json["rows"][0]["status"].is_string());
        }
        assert!(include_str!("lib.rs").contains("            operation_plan,"));
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

    // ---------- editor: load_agent_for_edit / save_agent_edit ----------
    //
    // The GUI editor is a thin pair over `agenthd::store::load_agent_for_edit`
    // + `workflows::save_agent`. The helpers here add the GUI-only
    // preconditions (resolved `checkout_path` matches the open-time
    // one, existing edit file still exists) on top of the lib's
    // fail-closed seam. The tests below pin each precondition and the
    // round-trip through the wire DTOs.

    /// Build a ready `Paths` + seeded checkout + applied
    /// `settings.json` so the editor helpers see a `Ready`
    /// checkout. The returned `Paths` is **raw** — it comes
    /// straight out of `Paths::resolve(...)` so its
    /// `canonical_dir` still points at the historical default
    /// `<agenthd_root>/agents`. The configured checkout lives
    /// at `<tmpdir>/checkout`, distinct from the default,
    /// and `settings.json` points at it. The helper also
    /// optionally seeds a same-named `<name>.md` at the
    /// DEFAULT canonical dir so the editor tests can assert
    /// the helper never reads or writes through the
    /// default — only the configured checkout.
    ///
    /// This mirrors the runtime shape: `#[tauri::command]`
    /// helpers call `Paths::from_env()` which yields the
    /// same raw `Paths` (canonical_dir = default), and the
    /// composition helpers MUST resolve `Ready` and derive a
    /// scoped `Paths` themselves. Pre-applying
    /// `paths.with_settings(...)` here would mask the P1
    /// runtime bug this regression test pins.
    fn ready_paths_with_seed_agent(
        dir: &TempDir,
        configured_name: &str,
        configured_body: &str,
    ) -> Paths {
        let paths = paths_in(dir);
        // Configured checkout, distinct from
        // `<agenthd_root>/agents`.
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(&agents_dir, configured_name, configured_body);

        // Optional decoy at the default canonical dir: a
        // same-named `<name>.md` the editor MUST NOT read or
        // write through. If the helper used the input
        // `paths.canonical_dir` directly, this decoy would
        // be the file it operates on, which is the P1 bug.
        let decoy_agents = paths.canonical_dir.clone();
        fs::create_dir_all(&decoy_agents).unwrap();
        write_md(
            &decoy_agents,
            configured_name,
            "---\ndescription: decoy at default\nmode: subagent\n---\nDECOY body — must not be loaded or saved\n",
        );

        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();
        // Return raw `Paths` (canonical_dir = default). Do
        // NOT call `with_settings` here.
        paths
    }

    #[test]
    fn editor_crud_scopes_raw_paths_and_preserves_targets_and_manifests() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let source = dir.path().join("checkout/agents");
        fs::create_dir_all(&paths.target_dir).unwrap();
        fs::create_dir_all(&paths.pi_target_dir).unwrap();
        fs::write(paths.target_dir.join("scout.md"), "installed opencode").unwrap();
        fs::write(paths.pi_target_dir.join("scout.md"), "installed pi").unwrap();
        fs::write(&paths.state_file, "state sentinel").unwrap();
        let settings_before = fs::read(&paths.settings_file).unwrap();
        let decoy_before = fs::read(paths.canonical_dir.join("scout.md")).unwrap();
        let (mut dto, context) = compose_load_new_agent_for_edit(&paths, "fresh").unwrap();
        assert_eq!(
            dto,
            project_agent_to_dto(agenthd::agent::Agent::new_default("fresh".into()).unwrap())
        );
        assert_eq!(context.original_name, None);
        assert_eq!(context.prior_hash, None);
        let wire = serde_json::to_value(&context).unwrap();
        assert!(wire["original_name"].is_null());
        assert!(wire["prior_hash"].is_null());
        assert!(!source.join("fresh.md").exists());
        dto.description = "created".into();
        dto.prompt = "body".into();
        compose_save_agent_edit(&paths, context.clone(), dto.clone()).unwrap();
        assert!(compose_save_agent_edit(&paths, context, dto)
            .unwrap_err()
            .contains("overwrite existing"));
        let (mut dto, context) = compose_load_agent_for_edit(&paths, "fresh").unwrap();
        dto.description = "edited".into();
        compose_save_agent_edit(&paths, context, dto).unwrap();
        let (mut dto, context) = compose_load_agent_for_edit(&paths, "fresh").unwrap();
        dto.name = "renamed".into();
        compose_save_agent_edit(&paths, context, dto).unwrap();
        assert!(!source.join("fresh.md").exists());
        let (_, context) = compose_load_agent_for_edit(&paths, "renamed").unwrap();
        compose_delete_agent_edit(&paths, context.clone()).unwrap();
        compose_delete_agent_edit(&paths, context).unwrap(); // Root NotFound is success.
        assert!(!source.join("renamed.md").exists());
        let (_, context) = compose_load_agent_for_edit(&paths, "scout").unwrap();
        compose_delete_agent_edit(&paths, context).unwrap();
        assert_eq!(
            fs::read(paths.canonical_dir.join("scout.md")).unwrap(),
            decoy_before
        );
        assert!(!paths.canonical_dir.join("fresh.md").exists());
        assert!(!paths.canonical_dir.join("renamed.md").exists());
        assert_eq!(fs::read(&paths.settings_file).unwrap(), settings_before);
        assert_eq!(
            fs::read_to_string(&paths.state_file).unwrap(),
            "state sentinel"
        );
        assert_eq!(
            fs::read_to_string(paths.target_dir.join("scout.md")).unwrap(),
            "installed opencode"
        );
        assert_eq!(
            fs::read_to_string(paths.pi_target_dir.join("scout.md")).unwrap(),
            "installed pi"
        );
    }

    #[test]
    fn editor_new_rejects_mixed_context_and_invalid_name() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        assert!(compose_load_new_agent_for_edit(&paths, "../escape").is_err());
        let (mut dto, context) = compose_load_new_agent_for_edit(&paths, "fresh").unwrap();
        dto.description = "x".into();
        dto.prompt = "body".into();
        for (original_name, prior_hash) in
            [(Some("scout".into()), None), (None, Some("a".repeat(64)))]
        {
            let mixed = AgentEditContext {
                original_name,
                prior_hash,
                ..context.clone()
            };
            assert!(compose_save_agent_edit(&paths, mixed.clone(), dto.clone())
                .unwrap_err()
                .contains("incomplete"));
            assert!(compose_delete_agent_edit(&paths, mixed)
                .unwrap_err()
                .contains("incomplete"));
        }
        assert!(compose_delete_agent_edit(&paths, context)
            .unwrap_err()
            .contains("unsaved new"));
        assert!(!dir.path().join("checkout/agents/fresh.md").exists());
    }

    #[test]
    fn editor_rename_and_delete_reject_stale_hash_and_collisions() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let source = dir.path().join("checkout/agents/scout.md");
        let (mut dto, context) = compose_load_agent_for_edit(&paths, "scout").unwrap();
        dto.name = "renamed".into();
        let drift = "---\ndescription: external\nmode: subagent\n---\nexternal body\n";
        fs::write(&source, drift).unwrap();
        assert!(
            compose_save_agent_edit(&paths, context.clone(), dto.clone())
                .unwrap_err()
                .contains("changed on disk")
        );
        assert!(compose_delete_agent_edit(&paths, context)
            .unwrap_err()
            .contains("changed on disk"));
        assert_eq!(fs::read_to_string(&source).unwrap(), drift);
        assert!(!dir.path().join("checkout/agents/renamed.md").exists());
        let (_, context) = compose_load_agent_for_edit(&paths, "scout").unwrap();
        let destination = dir.path().join("checkout/agents/renamed.md");
        fs::write(&destination, "occupied").unwrap();
        let err = compose_save_agent_edit(&paths, context, dto).unwrap_err();
        assert!(err.starts_with("rename:"), "root workflow error: {err}");
        assert_eq!(fs::read_to_string(&source).unwrap(), drift);
        assert_eq!(fs::read_to_string(destination).unwrap(), "occupied");
    }

    #[test]
    fn editor_rename_preserves_root_partial_failure_without_rollback() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let source = dir.path().join("checkout/agents/scout.md");
        let before = fs::read(&source).unwrap();
        let (mut dto, context) = compose_load_agent_for_edit(&paths, "scout").unwrap();
        dto.name = "renamed".into();
        dto.description.clear(); // Root save validation happens after root rename.
        assert_eq!(
            compose_save_agent_edit(&paths, context, dto).unwrap_err(),
            "save: description must not be empty"
        );
        assert!(!source.exists());
        assert_eq!(
            fs::read(dir.path().join("checkout/agents/renamed.md")).unwrap(),
            before
        );
    }

    #[test]
    fn editor_all_mutations_reject_checkout_drift_or_missing_source() {
        for failure in ["changed", "empty", "stale"] {
            let dir = TempDir::new().unwrap();
            let paths = ready_paths_with_seed_agent(
                &dir,
                "scout",
                "---\ndescription: x\nmode: subagent\n---\nbody\n",
            );
            let (dto, context) = compose_load_agent_for_edit(&paths, "scout").unwrap();
            let (mut new_dto, new_context) =
                compose_load_new_agent_for_edit(&paths, "fresh").unwrap();
            new_dto.description = "x".into();
            new_dto.prompt = "body".into();
            let source = dir.path().join("checkout/agents/scout.md");
            let before = fs::read(&source).unwrap();
            let other = dir.path().join("other");
            fs::create_dir_all(other.join("agents")).unwrap();
            fs::write(other.join("agents/scout.md"), "homonym").unwrap();
            match failure {
                "changed" => save_settings(
                    &paths.settings_file,
                    &Settings::new(other.to_string_lossy().into_owned()),
                )
                .unwrap(),
                "empty" => fs::remove_file(&paths.settings_file).unwrap(),
                _ => fs::remove_dir_all(dir.path().join("checkout/agents")).unwrap(),
            }
            let mut rename = dto.clone();
            rename.name = "renamed".into();
            assert!(compose_save_agent_edit(&paths, context.clone(), dto).is_err());
            assert!(compose_save_agent_edit(&paths, context.clone(), rename).is_err());
            assert!(compose_delete_agent_edit(&paths, context).is_err());
            assert!(compose_save_agent_edit(&paths, new_context, new_dto).is_err());
            assert!(
                compose_load_new_agent_for_edit(&paths, "fresh").is_err() || failure == "changed"
            );
            if failure != "stale" {
                assert_eq!(fs::read(source).unwrap(), before);
            }
            assert_eq!(
                fs::read_to_string(other.join("agents/scout.md")).unwrap(),
                "homonym"
            );
            assert!(!other.join("agents/fresh.md").exists());
            assert!(!dir.path().join("checkout/agents/fresh.md").exists());
            assert!(!dir.path().join("checkout/agents/renamed.md").exists());
        }
    }

    /// Happy path round-trip: load a starter, then save an
    /// edited version. The wire DTO must carry every field
    /// (`name`, `description`, `mode`, `model`, `prompt`, full
    /// permissions map) and the lib must render the edited
    /// material to disk.
    #[test]
    fn editor_round_trip_load_then_save_persists_edited_bytes() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: original description\nmode: subagent\nmodel: prov/orig\npermission:\n  bash: ask\n  edit: deny\n---\noriginal prompt body\n",
        );
        let checkout = dir.path().join("checkout");

        let (dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        assert_eq!(dto.name, "scout");
        assert_eq!(dto.description, "original description");
        assert_eq!(dto.mode, "subagent");
        assert_eq!(dto.model.as_deref(), Some("prov/orig"));
        assert_eq!(dto.prompt, "original prompt body");
        assert_eq!(dto.permissions.get("bash").map(String::as_str), Some("ask"));
        assert_eq!(
            dto.permissions.get("edit").map(String::as_str),
            Some("deny")
        );
        // Context carries the open-time checkout + name + hash.
        assert!(context.checkout_path.ends_with("checkout"));
        assert_eq!(context.original_name.as_deref(), Some("scout"));
        assert_eq!(context.prior_hash.as_ref().unwrap().len(), 64);

        // Now edit every field the editor exposes and save.
        let mut edited = dto.clone();
        edited.description = "edited description".to_string();
        edited.mode = "primary".to_string();
        edited.model = Some("prov/new".to_string());
        edited.prompt = "edited prompt body".to_string();
        edited
            .permissions
            .insert("bash".to_string(), "allow".to_string());
        edited
            .permissions
            .insert("edit".to_string(), "allow".to_string());

        let saved = compose_save_agent_edit(&paths, context, edited).expect("save must succeed");
        assert_eq!(saved, "scout");

        // Reload via the lib directly so we see what landed
        // on disk, independent of any helper-specific
        // projection. Build a scoped `Paths` whose
        // `canonical_dir` points at the configured checkout
        // — using the raw input `paths` would read from the
        // default `<agenthd_root>/agents` (the decoy dir),
        // which is exactly what the P1 bug would do.
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        let scoped = paths.clone().with_settings(&settings).unwrap();
        let (reloaded, prior_hash) = agenthd::store::load_agent_for_edit(&scoped, "scout").unwrap();
        assert_eq!(reloaded.description, "edited description");
        assert_eq!(reloaded.mode, agenthd::agent::Mode::primary);
        assert_eq!(reloaded.model.as_deref(), Some("prov/new"));
        assert_eq!(reloaded.prompt, "edited prompt body");
        assert_eq!(
            reloaded.permissions.get("bash"),
            Some(&agenthd::agent::PermissionAction::Allow)
        );
        assert_eq!(
            reloaded.permissions.get("edit"),
            Some(&agenthd::agent::PermissionAction::Allow)
        );
        assert_eq!(prior_hash.len(), 64);

        // P1 regression: the configured checkout now carries
        // the edited bytes; the default canonical_dir (where
        // the decoy lives) is byte-identical to the decoy
        // that was seeded — i.e., the helper never read or
        // wrote through it.
        let decoy_path = paths.canonical_dir.join("scout.md");
        let decoy_bytes = fs::read_to_string(&decoy_path).unwrap();
        assert!(
            decoy_bytes.contains("DECOY body"),
            "default canonical_dir must not have been overwritten by save, got: {decoy_bytes}"
        );
    }

    /// `load_agent_for_edit` on an `Empty` settings file must
    /// refuse — opening the editor when there is no
    /// configured checkout is a user-error, not a phantom
    /// fallback to the local default.
    #[test]
    fn editor_load_refuses_empty_settings() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        // Seed the canonical file so the load would otherwise
        // succeed at the parser level; the empty-settings
        // gate must fire first.
        write_md(
            &checkout.join("agents"),
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );

        let err = compose_load_agent_for_edit(&paths, "scout").unwrap_err();
        assert!(
            err.contains("configure a checkout first"),
            "expected empty-settings guidance, got: {err}"
        );
    }

    /// `load_agent_for_edit` on a missing agent file must
    /// surface a real `load_agent_for_edit` error (NOT an
    /// empty-settings or stale-checkout message). The error
    /// text must mention the requested file so the user can
    /// figure out why the open failed.
    #[test]
    fn editor_load_refuses_missing_agent_file() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        fs::create_dir_all(checkout.join("agents")).unwrap();
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();
        let paths = paths.with_settings(&settings).unwrap();

        let err = compose_load_agent_for_edit(&paths, "ghost").unwrap_err();
        assert!(
            err.contains("ghost.md"),
            "expected missing-file error to name the file, got: {err}"
        );
    }

    /// Rename delegates to the root workflow using the captured context.
    #[test]
    fn editor_save_delegates_name_change_to_workflow() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let (mut dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        dto.name = "renamed".to_string();

        let captured_hash = context.prior_hash.clone();
        assert_eq!(
            compose_save_agent_edit(&paths, context, dto).unwrap(),
            "renamed"
        );
        assert!(!dir.path().join("checkout/agents/scout.md").exists());
        let (_, renamed_context) = compose_load_agent_for_edit(&paths, "renamed").unwrap();
        assert!(captured_hash.is_some());
        assert!(renamed_context.prior_hash.is_some());
    }

    /// `save_agent_edit` must refuse when the canonical file
    /// has been removed between open and save. The
    /// "no-silent-create" guarantee: the editor must not
    /// convert a removed-target edit into a fresh
    /// `<name>.md`.
    #[test]
    fn editor_save_rejects_when_canonical_disappeared() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let checkout = dir.path().join("checkout");
        let (dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        fs::remove_file(checkout.join("agents").join("scout.md")).unwrap();

        let mut rename = dto.clone();
        rename.name = "renamed".into();
        assert!(compose_save_agent_edit(&paths, context.clone(), rename)
            .unwrap_err()
            .contains("no longer exists on disk"));
        assert!(!checkout.join("agents/renamed.md").exists());
        let err = compose_save_agent_edit(&paths, context, dto).unwrap_err();
        assert!(
            err.contains("no longer exists on disk"),
            "expected missing-canonical refusal, got: {err}"
        );
        // No new file was created. Both the configured
        // checkout and the default canonical_dir stay
        // empty (the configured one was the one we
        // deleted, and we never wrote to the default).
        assert!(
            !checkout.join("agents").join("scout.md").exists(),
            "save must not create a fresh file when canonical disappeared"
        );
        // The default canonical_dir must also stay
        // untouched: only the decoy the helper seeded
        // is there, untouched.
        let decoy = paths.canonical_dir.join("scout.md");
        if decoy.exists() {
            let bytes = fs::read_to_string(&decoy).unwrap();
            assert!(
                bytes.contains("DECOY body"),
                "default canonical_dir must not be written, got: {bytes}"
            );
        }
    }

    /// `save_agent_edit` with a `prior_hash` that no longer
    /// matches the live file (someone edited it under the
    /// editor) must surface the standard
    /// "`...` changed on disk" error from
    /// `workflows::save_agent` / `save_canonical`. The lib is
    /// the single source of truth for this check; the helper
    /// just hands the context through.
    #[test]
    fn editor_save_propagates_stale_write_error() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let checkout = dir.path().join("checkout");
        let (dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");

        // External writer mutates the configured canonical
        // file (the helper's actual write target). Mutating
        // `paths.canonical_dir` (the default) would silently
        // leave the helper's stale-check valid against the
        // decoy and miss the real bug surface.
        let canonical = checkout.join("agents").join("scout.md");
        let original = fs::read_to_string(&canonical).unwrap();
        fs::write(&canonical, original.replace("body\n", "tampered\n")).unwrap();

        let err = compose_save_agent_edit(&paths, context, dto).unwrap_err();
        assert!(
            err.contains("changed on disk"),
            "expected stale-write error from the lib, got: {err}"
        );
    }

    /// `save_agent_edit` must refuse when the configured
    /// checkout changed between open and save. This is the
    /// "homonymous agent in another repo" guard: a draft the
    /// user opened against checkout A must not land in
    /// checkout B when checkout B already has a different
    /// agent with the same name (or even the same name but
    /// different content — same outcome, surface an error).
    #[test]
    fn editor_save_rejects_when_checkout_switched() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let (dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");

        // Switch the configured checkout to a different
        // directory. The save must refuse because the
        // open-time `checkout_path` no longer matches.
        let other = dir.path().join("other-checkout");
        fs::create_dir_all(other.join("agents")).unwrap();
        save_settings(
            &paths.settings_file,
            &Settings::new(other.to_string_lossy().into_owned()),
        )
        .unwrap();

        let err = compose_save_agent_edit(&paths, context, dto).unwrap_err();
        assert!(
            err.contains("configured checkout changed"),
            "expected checkout-switch refusal, got: {err}"
        );
    }

    /// `save_agent_edit` with an empty context must be
    /// refused up front so a buggy frontend cannot sneak
    /// past the ready-checkout gate. The error message
    /// mentions every required context field.
    #[test]
    fn editor_save_rejects_empty_context() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        let (dto, _ctx) = compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");

        let empty_ctx = AgentEditContext {
            checkout_path: String::new(),
            original_name: Some(dto.name.clone()),
            prior_hash: Some("a".repeat(64)),
        };
        let err = compose_save_agent_edit(&paths, empty_ctx, dto).unwrap_err();
        assert!(
            err.contains("edit context is incomplete"),
            "expected empty-context refusal, got: {err}"
        );
    }

    /// `save_agent_edit` must NOT mutate settings.json,
    /// state.json, or any target-tree file. The save only
    /// touches `<canonical_dir>/<name>.md`. After a
    /// successful save, all four filesystems below are
    /// byte-for-byte the same as before the call.
    #[test]
    fn editor_save_does_not_touch_settings_state_or_targets() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        // Seed a state.json so the helper would have
        // something to (not) touch.
        fs::write(&paths.state_file, b"{\"installed\":{}}").unwrap();

        let before_settings = fs::read(&paths.settings_file).unwrap();
        let before_state = fs::read(&paths.state_file).unwrap();

        let (dto, context) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        let mut edited = dto.clone();
        edited.prompt = "edited body".to_string();
        compose_save_agent_edit(&paths, context, edited).expect("save must succeed");

        assert_eq!(
            fs::read(&paths.settings_file).unwrap(),
            before_settings,
            "save_agent_edit must not rewrite settings.json"
        );
        assert_eq!(
            fs::read(&paths.state_file).unwrap(),
            before_state,
            "save_agent_edit must not rewrite state.json"
        );
        assert!(
            !paths.target_dir.join("scout.md").exists(),
            "save_agent_edit must not write to target_dir"
        );
        assert!(
            !paths.pi_target_dir.join("scout.md").exists(),
            "save_agent_edit must not write to pi_target_dir"
        );

        // P1 regression: the decoy at the default
        // canonical_dir is byte-identical to what was seeded.
        let decoy_path = paths.canonical_dir.join("scout.md");
        let decoy_bytes = fs::read_to_string(&decoy_path).unwrap();
        assert!(
            decoy_bytes.contains("DECOY body"),
            "default canonical_dir must not be written by save, got: {decoy_bytes}"
        );
    }

    /// `load_agent_for_edit` must NOT touch settings.json or
    /// state.json (read-only). The check pins the read-only
    /// contract at the byte level.
    #[test]
    fn editor_load_does_not_touch_settings_or_state() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\n---\nbody\n",
        );
        fs::write(&paths.state_file, b"{\"installed\":{}}").unwrap();
        let before_settings = fs::read(&paths.settings_file).unwrap();
        let before_state = fs::read(&paths.state_file).unwrap();

        let _ = compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        assert_eq!(fs::read(&paths.settings_file).unwrap(), before_settings);
        assert_eq!(fs::read(&paths.state_file).unwrap(), before_state);
    }

    /// `AgentSummary` JSON pin: the read-only list payload
    /// stays at exactly four fields. The editor helpers
    /// never widen that payload — they surface their data
    /// through `AgentEditDto` (separate wire type) instead.
    #[test]
    fn editor_dto_does_not_leak_into_agent_summary() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\npermission:\n  bash: deny\n---\nSECRET body\n",
        );
        let list = compose_agents_list(&paths);
        let json = serde_json::to_string(&list).unwrap();
        // The summary must not contain prompt / permission
        // keys or the secret body.
        assert!(
            !json.contains("SECRET body"),
            "summary payload must not include prompt body, got: {json}"
        );
        assert!(
            !json.contains("\"prompt\"") && !json.contains("\"permissions\""),
            "summary payload must not reference editor-only keys, got: {json}"
        );
    }

    /// `AgentEditDto` round-trip pin: the wire shape carries
    /// every editable field. The lib must accept what the
    /// helper produces from the wire shape and the helper
    /// must accept what the lib produces. Without this
    /// round-trip the GUI editor and the lib could silently
    /// disagree about which fields are editable.
    #[test]
    fn dto_round_trip_preserves_every_field() {
        let dir = TempDir::new().unwrap();
        let paths = ready_paths_with_seed_agent(
            &dir,
            "scout",
            "---\ndescription: x\nmode: subagent\nmodel: prov/x\npermission:\n  bash: ask\n  edit: deny\n---\nbody\n",
        );
        let (dto, _ctx) = compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        // JSON shape: top-level keys are exactly the
        // editor-DTO fields + context fields, nothing else.
        let value = serde_json::to_value(&dto).unwrap();
        let keys: std::collections::BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "description",
                "mode",
                "model",
                "name",
                "permissions",
                "prompt"
            ]
            .into_iter()
            .collect()
        );
        // JSON shape: context top-level keys are exactly the
        // three open-time fields.
        let ctx_value =
            serde_json::to_value(compose_load_agent_for_edit(&paths, "scout").unwrap().1).unwrap();
        let ctx_keys: std::collections::BTreeSet<&str> = ctx_value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            ctx_keys,
            ["checkout_path", "original_name", "prior_hash"]
                .into_iter()
                .collect()
        );
    }

    // ---------- P1 regression tests ----------
    //
    // These tests pin the P1 fix in
    // `compose_load_agent_for_edit` /
    // `compose_save_agent_edit`: the helpers MUST resolve
    // `Ready` themselves, derive a scoped `Paths`, and use
    // that scoped value for every canonical operation.
    // They MUST NOT use the input `paths.canonical_dir`
    // (the historical default `<agenthd_root>/agents`),
    // and they MUST NOT use `context.checkout_path` as a
    // directory driver — the context is a wire guard only.
    //
    // The fixtures use `Paths::resolve(...)` RAW (no
    // `with_settings` re-pointing). They also seed a
    // homonymous decoy at the default canonical_dir and
    // assert it stays untouched: a save that goes through
    // the default would either overwrite the decoy or be
    // flagged as the P1 bug.

    /// `compose_load_agent_for_edit` against a raw
    /// `Paths::resolve(...)` (i.e. the runtime shape)
    /// reads from the configured checkout, never from the
    /// default canonical_dir where a homonymous decoy
    /// lives. Pre-P1 the helper would load the decoy.
    #[test]
    fn p1_load_uses_configured_checkout_not_default() {
        let dir = TempDir::new().unwrap();
        // Raw Paths — canonical_dir is the default.
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(
            &agents_dir,
            "scout",
            "---\ndescription: configured source\nmode: subagent\n---\nCONFIGURED body\n",
        );
        // Homonymous decoy at the default canonical_dir.
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        write_md(
            &paths.canonical_dir,
            "scout",
            "---\ndescription: default decoy\nmode: subagent\n---\nDECOY body — must not be loaded\n",
        );
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();

        let (dto, _ctx) = compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        assert_eq!(
            dto.description, "configured source",
            "load must pull from configured checkout, not the default"
        );
        assert_eq!(dto.prompt, "CONFIGURED body");
        // Decoy is intact.
        let decoy_bytes = fs::read_to_string(paths.canonical_dir.join("scout.md")).unwrap();
        assert!(
            decoy_bytes.contains("DECOY body"),
            "default decoy must not have been read or replaced"
        );
    }

    /// `compose_save_agent_edit` against a raw
    /// `Paths::resolve(...)` writes to the configured
    /// checkout, never to the default canonical_dir where
    /// a homonymous decoy lives. Pre-P1 the helper would
    /// write into the default and either overwrite the
    /// decoy or, when the canonical-dir safety gates
    /// tripped, surface the wrong error.
    #[test]
    fn p1_save_uses_configured_checkout_not_default() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(
            &agents_dir,
            "scout",
            "---\ndescription: configured source\nmode: subagent\n---\nCONFIGURED body\n",
        );
        // Homonymous decoy at the default canonical_dir.
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        write_md(
            &paths.canonical_dir,
            "scout",
            "---\ndescription: default decoy\nmode: subagent\n---\nDECOY body\n",
        );
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();

        let (mut dto, ctx) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        dto.prompt = "EDITED body".to_string();
        compose_save_agent_edit(&paths, ctx, dto).expect("save must succeed");

        // Configured checkout has the edit.
        let configured_bytes = fs::read_to_string(agents_dir.join("scout.md")).unwrap();
        assert!(
            configured_bytes.contains("EDITED body"),
            "configured checkout must hold the edited bytes"
        );
        // Default canonical_dir decoy is intact — the
        // helper never wrote through it.
        let decoy_bytes = fs::read_to_string(paths.canonical_dir.join("scout.md")).unwrap();
        assert!(
            decoy_bytes.contains("DECOY body"),
            "default canonical_dir must not have been written, got: {decoy_bytes}"
        );
        // Sanity: the default canonical_dir also still
        // holds its `<name>.md` (the decoy), proving the
        // historical `<agenthd_root>/agents` default was
        // never repurposed by the helper.
        assert!(
            paths.canonical_dir.join("scout.md").is_file(),
            "default canonical_dir's decoy file must still exist"
        );
    }

    /// After a load-then-save roundtrip with raw Paths,
    /// the helper MUST NOT have recreated the historical
    /// default `<agenthd_root>/agents` directory when the
    /// configured checkout is the only valid source.
    /// This is the no-default-recreated contract the
    /// single-source design retired; the editor MUST NOT
    /// reintroduce it.
    #[test]
    fn p1_editor_does_not_recreate_default_canonical_dir() {
        let dir = TempDir::new().unwrap();
        // Raw Paths — agenthd_root/agents does not yet
        // exist. The configured checkout lives elsewhere
        // and `settings.json` points at it. The editor
        // must NEVER create `<agenthd_root>/agents/` as a
        // side-effect.
        let paths = paths_in(&dir);
        assert!(
            !paths.canonical_dir.exists(),
            "preflight: default canonical_dir must not exist yet"
        );
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(
            &agents_dir,
            "scout",
            "---\ndescription: configured\nmode: subagent\n---\nconfigured body\n",
        );
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();

        let (mut dto, ctx) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        dto.prompt = "EDITED body".to_string();
        compose_save_agent_edit(&paths, ctx, dto).expect("save must succeed");

        assert!(
            !paths.canonical_dir.exists(),
            "editor must not recreate <agenthd_root>/agents/ — got: {:?}",
            paths.canonical_dir
        );
        // And the configured checkout has the edit.
        let configured_bytes = fs::read_to_string(agents_dir.join("scout.md")).unwrap();
        assert!(
            configured_bytes.contains("EDITED body"),
            "configured checkout must hold the edited bytes"
        );
    }

    /// Runtime-style helper against `Paths::from_env()` is
    /// not directly callable from tests (no env mutation),
    /// so this test simulates the same shape by passing a
    /// raw `Paths::resolve(...)` to `compose_agents_list`
    /// and asserting the helper resolves Ready itself —
    /// mirroring how the editor helpers MUST do the same.
    /// The summary list at the configured checkout shows
    /// the configured agent, NOT the decoy at the default.
    #[test]
    fn p1_list_helper_uses_configured_checkout_pattern() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        write_md(
            &agents_dir,
            "configured-agent",
            "---\ndescription: at the configured checkout\nmode: subagent\n---\nbody\n",
        );
        // Decoy at default.
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        write_md(
            &paths.canonical_dir,
            "default-agent",
            "---\ndescription: at the default dir\nmode: subagent\n---\nbody\n",
        );
        let settings = Settings::new(checkout.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings).unwrap();

        let list = compose_agents_list(&paths);
        let names: Vec<&str> = list.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["configured-agent"],
            "list helper must surface configured checkout only, not the default"
        );
    }

    /// Settings switch despite `Paths` having a raw
    /// `canonical_dir`: changing the configured checkout
    /// in `settings.json` between load and save MUST
    /// surface the explicit `configured checkout changed`
    /// error — not silently save into the previously-
    /// configured checkout, and not write into the
    /// default canonical_dir.
    #[test]
    fn p1_save_rejects_settings_switch_with_raw_paths() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout_a = dir.path().join("checkout-a");
        let agents_a = checkout_a.join("agents");
        fs::create_dir_all(&agents_a).unwrap();
        write_md(
            &agents_a,
            "scout",
            "---\ndescription: at A\nmode: subagent\n---\nbody A\n",
        );
        fs::create_dir_all(&paths.canonical_dir).unwrap();
        write_md(
            &paths.canonical_dir,
            "scout",
            "---\ndescription: default decoy\nmode: subagent\n---\nDECOY\n",
        );
        let settings_a = Settings::new(checkout_a.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings_a).unwrap();

        let (mut dto, ctx) =
            compose_load_agent_for_edit(&paths, "scout").expect("load must succeed");
        dto.prompt = "EDITED".to_string();

        // Switch the configured checkout before save.
        let checkout_b = dir.path().join("checkout-b");
        fs::create_dir_all(checkout_b.join("agents")).unwrap();
        write_md(
            &checkout_b.join("agents"),
            "scout",
            "---\ndescription: at B\nmode: subagent\n---\nbody B\n",
        );
        let settings_b = Settings::new(checkout_b.to_string_lossy().into_owned());
        save_settings(&paths.settings_file, &settings_b).unwrap();

        let err = compose_save_agent_edit(&paths, ctx, dto).unwrap_err();
        assert!(
            err.contains("configured checkout changed"),
            "raw-Paths runtime save must reject a settings switch, got: {err}"
        );
        // Neither checkout should have been touched: no
        // save landed in either A or B, and the default
        // decoy is intact.
        let a_bytes = fs::read_to_string(agents_a.join("scout.md")).unwrap();
        assert!(
            a_bytes.contains("body A"),
            "checkout A must be untouched, got: {a_bytes}"
        );
        let b_bytes = fs::read_to_string(checkout_b.join("agents").join("scout.md")).unwrap();
        assert!(
            b_bytes.contains("body B"),
            "checkout B must be untouched, got: {b_bytes}"
        );
        let decoy_bytes = fs::read_to_string(paths.canonical_dir.join("scout.md")).unwrap();
        assert!(
            decoy_bytes.contains("DECOY"),
            "default canonical_dir must not have been written, got: {decoy_bytes}"
        );
    }
}
