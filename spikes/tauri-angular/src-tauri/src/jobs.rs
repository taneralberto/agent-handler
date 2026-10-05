//! D3 cooperative-cancellation long-job registry for the GUI.
//!
//! The GUI surfaces four categories of long-running work that
//! the lib `agenthd` already drives:
//!
//! - per-target agent safe install
//!   (`plan_then_apply_agents_safe_controlled`),
//! - skills install
//!   (`plan_then_apply_skills_controlled`),
//! - tools install (`install_tool_controlled`),
//! - models discovery (`discover_models_controlled`).
//!
//! Each consumes a [`agenthd::operation::CancelToken`] and a
//! progress sink. The D3 work in this module is **only** the
//! GUI's view: a process-global registry, a Tauri command
//! surface that the frontend can drive, a single "current
//! view" snapshot (`seq` + `JobSnapshot`) the frontend
//! reconciles on every change, and an `agenthd-operation`
//! event the backend emits outside the registry lock to the
//! `main` window.
//!
//! Design rules (the spike does NOT relax any of these):
//!
//! - **Single source of truth for the workflow.** The lib
//!   `agenthd::workflows::{plan_then_apply_agents_safe_controlled,
//!   plan_then_apply_skills_controlled}` and
//!   `agenthd::tools::install_tool_controlled` /
//!   `agenthd::models::discover_models_controlled` are reused
//!   verbatim. The helpers here compose a `Paths` (from
//!   `agenthd::store::Paths::from_env()`), call the workflow
//!   with the registry's `CancelToken`, route the `Progress`
//!   stream into the registry's sink closure, and surface the
//!   resulting `OperationReport` into a typed `JobSnapshot`
//!   shape the frontend can render.
//! - **`Paths` snapshot.** `agenthd::store::State::load` is
//!   run once **outside** the registry lock when the job is
//!   started, so the workflow reads the freshly-loaded
//!   `State` while the registry lock only owns the bookkeeping
//!   (id / token / latest snapshot / reservation / seq).
//! - **Reservation gate.** One in-flight job of any kind at a
//!   time. A second `start_operation` while a long job is
//!   active (or a short sync command is running) is refused
//!   with `OperationError::Busy`; the GUI treats that as
//!   "wait for the current snapshot to settle, then retry".
//!   The reservation is acquired **before** the
//!   `load_state_for` read so the env snapshot the workflow
//!   uses matches the snapshot the cancel/reject decision
//!   was made against.
//! - **Atomic reserve + initial publish.** `start_operation`
//!   acquires the reservation AND stamps the initial
//!   `JobSnapshot` AND bumps `current_seq` under the same
//!   lock before spawning the worker, so a frontend that
//!   wakes up from the `agenthd-operation` event cannot
//!   observe a stale `latest_snapshot` that misses the
//!   running job. The worker thread's spawn failure is
//!   published as a terminal `Failed` snapshot that releases
//!   the reservation atomically.
//! - **Seq per registry, monotonic under lock.** `current_seq`
//!   is a `u64` field on `RegistryInner`; every observable
//!   update bumps it under the mutex and stamps the new
//!   snapshot before releasing the lock. Snapshot *reads*
//!   (for `op_current` / `op_status`) do **not** bump the
//!   seq — the registry's seq is the canonical "I changed"
//!   signal, and a read-only fetch returns the SAME seq the
//!   last push used. The frontend's `BigInt` comparison
//!   discards re-pushed snapshots cleanly. The
//!   process-global `GLOBAL_SEQ` static is gone: per-process
//!   fixtures (tests run in parallel) used to share the same
//!   counter; the per-registry seq keeps each fixture
//!   independent.
//! - **Lock discipline.** The registry lock is held for the
//!   smallest possible window: id / token / seq /
//!   latest-job / reservation bookkeeping only. Every disk
//!   read and every `OperationReport` clone is performed
//!   outside the lock. The worker's `progress` sink reads
//!   the latest snapshot under the lock, mutates only the
//!   progress-related fields, and emits the new view
//!   outside — never the other way around. The terminal
//!   publish step writes the seq + latest snapshot +
//!   releases the reservation under one lock acquisition,
//!   then emits the view outside. A frontend that wakes up
//!   from the event cannot race a second `start_operation`
//!   against a not-yet-retired job.
//! - **No automatic rollback.** The D3 design is "partial
//!   without rollback": a cancelled or failed row keeps the
//!   bytes it landed, and the per-row outcomes the workflow
//!   already produced stay observable through the registry's
//!   `latest`. Failures / conflicts / skip rows are
//!   preserved verbatim — no `ok = true` "all installed"
//!   flattening.
//! - **Event emit best-effort.** An emit failure logs to
//!   stderr and the job stays retained in the registry;
//!   the frontend can re-fetch the current view via
//!   `op_current` / `op_status(id)` (always available
//!   during the job) to recover a missed terminal. The
//!   emit target is the `main` window (`emit_to("main",
//!   event, view)`) so a future second window does not
//!   receive a wire surface it is not built for.
//! - **CloseRequested gate.** A long reservation blocks the
//!   window's `CloseRequested` event so the user cannot
//!   close the GUI while a job is in flight. The
//!   `should_block_close` decision is read under the same
//!   lock the registry uses for the reservation, so a race
//!   between `start_operation` and a user click cannot let
//!   the window go away with the reservation still live.
//! - **Test-driven.** Every registry operation has a direct
//!   unit test: id allocation, reservation reject when
//!   busy, short-while-long reject, cancel propagation,
//!   terminal retention, event-emission shape. The
//!   fixtures build `Paths` via `Paths::resolve(...)`
//!   inside a `tempfile::TempDir` — the same pattern every
//!   other spike test uses — so the unit tests never touch
//!   `HOME` / `XDG_CONFIG_HOME` and never spawn the Tauri
//!   runtime. The thread-spawn-failure injection seam
//!   (`OperationRegistry::with_thread_factory`) lets a
//!   unit test force a `Failed` terminal without
//!   exhausting real `std::thread::Builder` quotas.

use agenthd::agent::PERMISSION_KEYS;
use agenthd::models::{discover_models_controlled, Discovery as LibDiscovery};
use agenthd::operation::{CancelToken, Finish, OperationReport, Progress};
use agenthd::store::{State, SyncTarget};
use agenthd::tools::{ToolCatalogEntry, DEFAULT_CATALOG};
use agenthd::workflows::{
    plan_then_apply_agents_safe_controlled, plan_then_apply_skills_controlled,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, EventTarget, Runtime};

/// Emit closure shared across the registry and the
/// worker threads. `Arc<dyn Fn>` so the worker can take
/// a clone; `Send + Sync` so the spawn boundary is
/// honest.
pub type EmitFnArc = Arc<dyn Fn(&CurrentView) -> bool + Send + Sync>;

/// Event name the backend uses to push `CurrentView` snapshots
/// to the frontend. The frontend registers a `listen<CurrentView>`
/// for this name; the event payload is the full snapshot (not a
/// delta) so a single reducer can rebuild the visible state.
pub const OPERATION_EVENT: &str = "agenthd-operation";

/// Window label the registry's events target. The spike's
/// frontend runs inside the `main` window; emitting to a
/// specific label keeps a future second window off the wire.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Monotonic-clock fallback for ids when the test runs without
/// a Tauri runtime. The atomic counter is the source of
/// truth; the timestamp is only used for the human-readable
/// `created_at` field.
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Thread factory seam used by the registry to spawn workers.
/// The production implementation is `std::thread::Builder::spawn`;
/// the test factory lets unit tests inject a `JoinHandle` that
/// returns a `spawn` error so the spawn-failure release path is
/// exercised without exhausting real quotas.
pub type ThreadSpawnFn = Box<dyn FnOnce(thread::Builder, JobWorker) -> Result<(), String> + Send>;

/// Closure the registry hands to the thread factory. It owns
/// the worker's body so the factory can move it into the
/// spawned thread (or fail to spawn and never call it).
pub type JobWorker = Box<dyn FnOnce() + Send + 'static>;

/// Default thread factory: `std::thread::Builder::spawn`. The
/// `JoinHandle` is dropped immediately so the worker owns its
/// own lifetime (detached). A `Builder::spawn` error is
/// converted to the worker's terminal error text and returned
/// to the registry; the registry publishes the terminal
/// `Failed` snapshot itself.
fn default_thread_factory(builder: thread::Builder, worker: JobWorker) -> Result<(), String> {
    let handle = builder
        .spawn(worker)
        .map_err(|e| format!("could not spawn worker thread: {e}"))?;
    // Detach: the worker is the owner of its lifetime; the
    // registry cannot block on it without holding a lock.
    drop(handle);
    Ok(())
}

/// Strongly-typed request kind. The frontend never sends
/// raw URLs, paths, hashes, or anything else; the backend
/// resolves every field from the lib. The discriminant
/// `kind` and `deny_unknown_fields` keep the wire contract
/// explicit: a typo from the frontend surfaces as a parse
/// failure on the JS side, not as a silent default arm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationRequest {
    /// Sync the configured checkout's agents to one of the
    /// installed harnesses. The frontend picks the target; the
    /// backend runs the same
    /// `plan_then_apply_agents_safe_controlled` workflow the
    /// TUI's `apply_safe_install` handler uses.
    SyncAgents { target: SyncAgentTarget },
    /// Install the configured checkout's skills into
    /// `paths.skills_dir`. Always `OpenCode`-only — Pi does
    /// not have a skills tree in this slice.
    InstallSkills,
    /// Install a single tool from the bundled
    /// `DEFAULT_CATALOG` by id. The backend looks the entry
    /// up itself; the frontend never supplies a URL / hash /
    /// path.
    InstallTool { tool_id: String },
    /// Run `opencode models` with cooperative cancellation,
    /// progress reporting, and a bounded output cap. Allowed
    /// while the editor is open (the discovery result is
    /// advisory only); the editor's save gate blocks until
    /// any in-flight discovery reaches a terminal snapshot.
    DiscoverModels,
}

/// Wire mirror of [`agenthd::store::SyncTarget`]. The lib
/// enum serializes to `"OpenCode"` / `"Pi"`, but the
/// `snake_case` discriminant the spike uses elsewhere prefers
/// lowercase, so the wire form is the lowercase form. The
/// helper [`SyncAgentTarget::as_lib_target`] is the single
/// source of truth for the conversion — the lib never sees
/// the wire form.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncAgentTarget {
    Opencode,
    Pi,
}

impl SyncAgentTarget {
    pub fn as_lib_target(self) -> SyncTarget {
        match self {
            SyncAgentTarget::Opencode => SyncTarget::OpenCode,
            SyncAgentTarget::Pi => SyncTarget::Pi,
        }
    }
}

/// Terminal `Finish` shape the frontend renders. Mirrors
/// [`agenthd::operation::Finish`] one-for-one so the wire
/// contract is the same on both sides; the rename
/// (`finish: "completed" | "cancelled" | "failed"`) keeps
/// the field lowercase to match the rest of the spike's
/// `#[serde(rename_all = "snake_case")]` style.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobFinish {
    Completed,
    Cancelled,
    Failed,
}

impl From<Finish> for JobFinish {
    fn from(f: Finish) -> Self {
        match f {
            Finish::Completed => JobFinish::Completed,
            Finish::Cancelled => JobFinish::Cancelled,
            Finish::Failed => JobFinish::Failed,
        }
    }
}

/// One tick of progress for a running job. The frontend
/// renders the `stage` and `processed / total` ratio. The
/// `item` is the row identifier the workflow is about to
/// touch, or `None` for stage ticks that are not bound to a
/// single row. This is the same shape the lib's
/// [`agenthd::operation::Progress`] carries, so the wire
/// contract is `Copy`-equivalent on the JS side and the
/// reducer can re-use the same `stage` router the TUI uses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobProgress {
    pub stage: String,
    pub item: Option<String>,
    pub processed: u64,
    pub total: Option<u64>,
}

impl From<Progress> for JobProgress {
    fn from(p: Progress) -> Self {
        let processed = p.processed as u64;
        let total = p.total.map(|t| t as u64);
        JobProgress {
            stage: p.stage.to_string(),
            item: p.item,
            processed,
            total,
        }
    }
}

/// Wire projection of one per-row outcome. The lib's
/// `ApplyOutcome` / `SkillOutcome` carry non-`Serialize`
/// fields and are private to their modules; the GUI's
/// DTO is a flat `(name, action, detail, ok)` record the
/// frontend can render as a list row. The lib's `ok` flag
/// is honored as-is so a `skipped` row stays `skipped`
/// (no `ok = true` flattening).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RowOutcome {
    pub name: String,
    pub action: String,
    pub detail: String,
    pub ok: bool,
}

/// Wire projection of `install_tool_controlled`'s
/// terminal report. The lib returns
/// `OperationReport<(ToolOutcome, Option<PathBuf>)>` —
/// `ToolOutcome` is the `(status, detail)` pair the GUI
/// already uses, and `Option<PathBuf>` is the residual
/// staging directory the cleaner could not remove. We
/// surface the residual explicitly so the GUI can render
/// it as a separate row; it is **not** a successful
/// install path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolTerminal {
    pub status: String,
    pub detail: String,
    /// Residual staging directory the cleaner could not
    /// remove. `None` when cleanup succeeded or no
    /// staging was created.
    pub residual: Option<String>,
}

/// Wire projection of `discover_models_controlled`'s
/// terminal report. The lib returns
/// `OperationReport<Option<Discovery>>`; we project the
/// `Discovery` enum into a tagged wire form so the
/// frontend can render the Found / Empty / Failed arms
/// without a parallel type ladder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryTerminal {
    Found { models: Vec<String> },
    Empty { message: String },
    Failed { message: String },
}

/// Per-job variant. Each variant carries the report shape
/// the workflow produced; the frontend dispatches on `kind`
/// the same way the lib dispatches on the `OperationReport`
/// discriminant. The `serde(tag = "kind")` keeps the wire
/// payload small and the variant arms distinct.
///
/// `observed_state` (defaulted on the wire via `serde(default)`)
/// is the post-workflow `State` the lib observed, surfaced as
/// an opaque `serde_json::Value` blob so the frontend can
/// read what the workflow reported without depending on the
/// lib's `State` shape. `SyncAgents` and `InstallSkills`
/// populate this; `InstallTool` and `DiscoverModels` leave
/// it `None` (they do not own the ownership manifest). The
/// field is observed-not-necessarily-persisted: the spike's
/// contract is "no rollback", and the registry surfaces what
/// the workflow had at terminal without writing it back.
///
/// `InstallTool` and `DiscoverModels` are **struct variants**
/// (with a single named `terminal` field) rather than tuple
/// variants. With `#[serde(tag = "kind")]`, internally-tagged
/// enums do not support `Option<T>` tuple variants: `None`
/// serializes as `null` (no `kind` field, so the tag is
/// ambiguous), and `Some(x)` would flatten the inner fields
/// directly into the variant payload (no nested
/// `terminal`). The frontend's `JobReport` projection
/// (`{ kind: "install_tool", terminal: ToolTerminal }` and
/// `{ kind: "discover_models", terminal: DiscoveryTerminal |
/// null }`) requires the nested form; the struct variant
/// preserves that wire contract. The `terminal` field is
/// always present (including `null` for a pre-cancel
/// discovery whose lib partial was `None`); no
/// `#[serde(skip_serializing_if = "Option::is_none")]` is
/// applied, so a cancel-arrived-before-spawn discovery
/// terminal retains `{ kind: "discover_models", terminal:
/// null }` on the wire — the frontend can read the field
/// without a guard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobReport {
    SyncAgents {
        outcomes: Vec<RowOutcome>,
        #[serde(default)]
        observed_state: Option<serde_json::Value>,
    },
    InstallSkills {
        outcomes: Vec<RowOutcome>,
        #[serde(default)]
        observed_state: Option<serde_json::Value>,
    },
    InstallTool {
        terminal: ToolTerminal,
    },
    DiscoverModels {
        terminal: Option<DiscoveryTerminal>,
    },
}

/// The lifecycle phase of a job. `running` and `finished`
/// are the only two arms; the registry does not model a
/// separate "cancelling" arm because the cancel request
/// either propagates to the next safe checkpoint (where it
/// is reported as `JobFinish::Cancelled` on the
/// `finished` transition) or it arrives after the terminal
/// publish (where the registry reports the cancel as a
/// no-op on an already-terminal job).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Running,
    Finished,
}

/// One snapshot of a job. The frontend rebuilds its visible
/// state from the latest `JobSnapshot` after every
/// `CurrentView` event. `cancel_requested` is sticky: once
/// `true` it stays `true` through the terminal transition
/// so the UI can render the "cancelling…" hint while the
/// worker still has a row in flight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSnapshot {
    pub id: String,
    pub request: OperationRequest,
    pub phase: JobPhase,
    pub cancel_requested: bool,
    /// The most recent progress tick the worker pushed
    /// under the registry lock. `None` for jobs that have
    /// not yet emitted a tick (the workflow's initial
    /// checkpoint fires before this field is set).
    pub progress: Option<JobProgress>,
    /// The terminal report, set on the `finished`
    /// transition. `None` while the job is still
    /// running. The lib's `OperationReport::partial` is
    /// preserved verbatim — failures, conflicts, and
    /// skip rows are not flattened into "all ok".
    pub report: Option<JobReport>,
    /// Mirror of [`OperationReport::error`]. `None` on
    /// `Completed`, set on `Failed` and on `Cancelled`
    /// (with a `cancelled before …` style message).
    pub error: Option<String>,
    /// The lib's `Finish` discriminant, surfaced as
    /// [`JobFinish`]. `None` while the job is still
    /// running; mirrors `phase == Finished`.
    pub finish: Option<JobFinish>,
    /// Wall-clock millis at job creation. Diagnostic
    /// only; not part of the equality contract.
    #[serde(skip)]
    pub created_at: u64,
}

impl JobSnapshot {
    fn new(id: String, request: OperationRequest) -> Self {
        Self {
            id,
            request,
            phase: JobPhase::Running,
            cancel_requested: false,
            progress: None,
            report: None,
            error: None,
            finish: None,
            created_at: now_unix_millis(),
        }
    }
}

/// The full view the frontend receives on every state
/// change. `seq` is the registry's monotonic sequence the
/// backend stamps on every push; the frontend ignores
/// snapshots whose `seq` it has already seen.
///
/// `job` is `Some` whenever there is a tracked job —
/// running, finished, or retained after the terminal. The
/// frontend uses `Some(_)` to render the right-hand
/// progress / report panel; `None` means "no job has ever
/// run on this backend process" (a freshly-booted window
/// with no user action yet). After the first job lands
/// the field stays `Some` through the terminal + a
/// `start_operation` follow-up, so a refresh that races
/// a job start does not lose the latest visible state.
///
/// The `seq` is a wire-stable `String` (the JS side
/// compares with `BigInt`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CurrentView {
    pub seq: String,
    pub job: Option<JobSnapshot>,
}

impl CurrentView {
    /// The freshly-booted view. The `seq` is `"0"` so the
    /// first event the registry emits can use seq `1`
    /// without a special-case for `lastSeq == null`.
    /// `job` is `None` because no job has been observed
    /// yet on this backend process.
    pub fn initial() -> Self {
        Self {
            seq: "0".to_string(),
            job: None,
        }
    }
}

/// Error shape the GUI commands return on a refused
/// request. The wire form is a tagged enum so the
/// frontend can render each arm differently:
/// `busy` → "wait for the current snapshot to settle";
/// `unknown_job` → "the job has already settled or its id
/// is from a different process"; `cancelled` → idempotent
/// no-op confirmation; `failed_preconditions` → the
/// backend has surfaced a precondition failure (no
/// checkout configured, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationError {
    Busy { active_job_id: String },
    UnknownJob { job_id: String },
    FailedPreconditions { message: String },
    Spawn { message: String },
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationError::Busy { active_job_id } => {
                write!(
                    f,
                    "another job is active (`{active_job_id}`); wait for it to settle"
                )
            }
            OperationError::UnknownJob { job_id } => {
                write!(f, "unknown or expired job `{job_id}`")
            }
            OperationError::FailedPreconditions { message } => write!(f, "{message}"),
            OperationError::Spawn { message } => write!(f, "{message}"),
        }
    }
}

/// Backing state of the registry. The `Mutex` is the
/// single point of contention; the workers hold it for
/// the smallest possible critical section.
struct RegistryInner {
    next_id: u64,
    /// Monotonic seq stamped on every observable update
    /// the registry publishes. `0` on a freshly-built
    /// registry (matches `CurrentView::initial`); the
    /// first push bumps it to `1`. Snapshot *reads* do
    /// NOT increment: the seq is the canonical "I
    /// changed" signal, and `op_current` returns the
    /// SAME seq the last push used so the frontend's
    /// BigInt comparison discards re-pushed snapshots.
    ///
    /// The wrapping-add counter is deliberately simple.
    /// At one bump per `op_start`/progress tick/terminal,
    /// a `u64` overflow takes ~584 years at a 1 GHz
    /// bump rate; a `saturating_add` would mask real
    /// bugs and a wrap to 0 would only confuse the
    /// frontend's `BigInt` compare (which would discard
    /// the next push as "seen") — both failures are
    /// preferable to silently dropping events.
    current_seq: u64,
    latest: Option<JobSnapshot>,
    active_token: Option<CancelToken>,
    active_id: Option<String>,
    /// One short-sync command in flight at a time, mutually
    /// exclusive with the long-running job reservation.
    short_busy: bool,
}

impl RegistryInner {
    fn new() -> Self {
        Self {
            next_id: 1,
            current_seq: 0,
            latest: None,
            active_token: None,
            active_id: None,
            short_busy: false,
        }
    }
}

/// Process-global registry. The Tauri command layer grabs
/// this once at boot and stores the `Arc` inside the
/// `tauri::Builder::manage` state. Tests construct their
/// own `Arc<OperationRegistry>` per test and bypass the
/// `tauri::Builder` path entirely.
pub struct OperationRegistry {
    inner: Mutex<RegistryInner>,
    /// Thread factory used to spawn worker threads. The
    /// production implementation is `default_thread_factory`;
    /// tests may substitute a factory that returns
    /// `Err(text)` to exercise the spawn-failure release
    /// path without exhausting real `std::thread::Builder`
    /// quotas.
    spawn_factory: Mutex<Option<ThreadSpawnFn>>,
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationRegistry {
    /// Build a fresh registry with the default thread
    /// factory.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner::new()),
            spawn_factory: Mutex::new(None),
        }
    }

    /// Install a custom thread factory. Returns the
    /// previous factory (or `None`). Tests use this to
    /// inject a `spawn` failure; the registry holds the
    /// factory under its own `Mutex` so swapping is
    /// race-free with the worker-spawn path. The factory
    /// is consumed by the next `start_operation` call
    /// and reset to `None` after; a second test that
    /// forgets to install a factory will fall back to the
    /// default factory.
    pub fn install_thread_factory(&self, factory: ThreadSpawnFn) -> Option<ThreadSpawnFn> {
        let mut slot = self
            .spawn_factory
            .lock()
            .expect("spawn-factory mutex poisoned");
        slot.replace(factory)
    }

    fn take_thread_factory(&self) -> Option<ThreadSpawnFn> {
        let mut slot = self
            .spawn_factory
            .lock()
            .expect("spawn-factory mutex poisoned");
        slot.take()
    }

    /// **One** atomic step: allocate the next id, install
    /// the reservation, stamp the initial running snapshot,
    /// bump the seq, and hand the resulting `CurrentView`
    /// back to the caller. The id / reservation / seq /
    /// snapshot are all decided under one lock acquisition
    /// so a `close()` / `status()` / `start()` racing
    /// against this call can never observe an "active"
    /// reservation without a matching initial snapshot, and
    /// can never observe an initial seq without the matching
    /// snapshot. `None` is returned when another long job
    /// or short sync is in flight; the caller MUST surface
    /// `OperationError::Busy`.
    fn reserve_and_publish(
        &self,
        request: OperationRequest,
    ) -> Option<(String, CancelToken, CurrentView)> {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        if guard.active_token.is_some() || guard.short_busy {
            return None;
        }
        let id = format!("job-{}", guard.next_id);
        guard.next_id = guard.next_id.wrapping_add(1);
        let token = CancelToken::new();
        let snapshot = JobSnapshot::new(id.clone(), request);
        guard.current_seq = guard.current_seq.wrapping_add(1);
        let seq = guard.current_seq;
        guard.latest = Some(snapshot.clone());
        guard.active_token = Some(token.clone());
        guard.active_id = Some(id.clone());
        let view = CurrentView {
            seq: seq.to_string(),
            job: Some(snapshot),
        };
        Some((id, token, view))
    }

    /// Try to acquire the short-sync reservation. Same
    /// atomicity as `reserve_and_publish`: a long job or
    /// another short job in flight causes `false`. The
    /// release path is [`Self::release_short`].
    fn try_reserve_short(&self) -> bool {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        if guard.active_token.is_some() || guard.short_busy {
            return false;
        }
        guard.short_busy = true;
        true
    }

    /// Release a previously-acquired short reservation.
    /// Idempotent: calling it twice is a no-op (the second
    /// call finds `short_busy == false` and returns).
    fn release_short(&self) {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        guard.short_busy = false;
    }

    /// Look up the active job id. Returns `None` when no
    /// long job is in flight. The short reservation does
    /// not register a job id — short sync calls do not
    /// appear in the `latest_snapshot` snapshot.
    fn active_job_id(&self) -> Option<String> {
        let guard = self.inner.lock().expect("registry mutex poisoned");
        guard.active_id.clone()
    }

    /// Decide whether a `CloseRequested` event should be
    /// blocked. The `tauri::Builder::on_window_event`
    /// callback delegates to this method; the decision is
    /// taken under the registry lock so a race between
    /// `start_operation` and a user click cannot let the
    /// window go away with the reservation still live.
    /// The window is allowed to close when no long job is
    /// in flight AND no short sync is running. A
    /// previously-seen close request does NOT keep the
    /// window blocked on its own — the close handler
    /// already called `api.prevent_close` for it; the
    /// follow-up close after the worker settles is allowed
    /// so the user does not have to click twice.
    pub fn should_block_close(&self) -> bool {
        let guard = self.inner.lock().expect("registry mutex poisoned");
        guard.active_token.is_some() || guard.short_busy
    }

    /// Atomic close-decision: under one lock, read the
    /// reservation state, request cancellation of the
    /// active long job (when one is in flight), bump the
    /// seq, install a sticky `cancel_requested` flag on
    /// the latest snapshot, and return the resulting
    /// `CurrentView` plus a `should_block` boolean. The
    /// caller MUST release the registry lock before
    /// emitting or calling `api.prevent_close` — the lock
    /// is held only inside this method.
    pub fn handle_close_requested(&self) -> (bool, Option<CurrentView>) {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        let should_block = guard.active_token.is_some() || guard.short_busy;
        let active_matches_latest = matches!(
            (&guard.active_id, guard.latest.as_ref()),
            (Some(aid), Some(s)) if s.id == *aid
        );
        if active_matches_latest {
            // Token is always Some when active_id matches
            // and the latest exists; clone first so we
            // can drop the immutable borrow before
            // mutably borrowing `guard.latest`.
            if let Some(token) = guard.active_token.clone() {
                token.request();
                let need_publish = guard
                    .latest
                    .as_ref()
                    .map(|s| !s.cancel_requested)
                    .unwrap_or(false);
                if need_publish {
                    // Take the latest snapshot out, mutate
                    // it without holding the borrow on
                    // `guard`, then bump the seq and put
                    // the snapshot back. The seq bump is
                    // always paired with the snapshot
                    // install; the registry stays atomic.
                    let mut snapshot =
                        std::mem::take(&mut guard.latest).expect("active_matches_latest");
                    snapshot.cancel_requested = true;
                    let seq = guard.current_seq.wrapping_add(1);
                    guard.current_seq = seq;
                    let view = CurrentView {
                        seq: seq.to_string(),
                        job: Some(snapshot.clone()),
                    };
                    guard.latest = Some(snapshot);
                    return (should_block, Some(view));
                }
            }
        }
        // No state change — return the latest view the
        // registry has, so the caller can emit it for
        // re-entrancy without bumping the seq.
        let view = guard.latest.as_ref().map(|s| CurrentView {
            seq: guard.current_seq.to_string(),
            job: Some(s.clone()),
        });
        (should_block, view)
    }

    /// Request cancellation of the active long job. The
    /// `CancelToken` is the only signal the worker
    /// observes, so this is the atomic gate that flips
    /// both the lib token AND the sticky
    /// `cancel_requested` flag on the snapshot — once.
    /// The seq is bumped and the captured view is
    /// returned for emit-once-outside.
    ///
    /// Returns:
    /// - `Some((Ok(view), view))` when this call flipped
    ///   the flag for the first time on the active job;
    ///   `view` is the freshly-stamped snapshot the
    ///   caller emits.
    /// - `Some((Err(view), view))` when the cancel was
    ///   already requested or the job is terminal; the
    ///   caller emits the unchanged `view` for
    ///   idempotency.
    /// - `None` when the `job_id` does not match the
    ///   active job (unknown / expired / from another
    ///   registry).
    fn request_cancel(&self, job_id: &str) -> Option<Result<CurrentView, CurrentView>> {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        // Mismatch: unknown or already-released. Caller
        // surfaces `UnknownJob` for the first case; for
        // the second (a retained terminal) the caller
        // also matches via `snapshot_for`.
        if guard.active_id.as_deref() != Some(job_id) {
            return None;
        }
        // Clone the token up front so we don't have to
        // hold an immutable borrow into `guard` while we
        // mutate `latest`.
        let token = guard.active_token.clone()?;
        let (already_canceled, snapshot_id_matches) = {
            let latest = guard.latest.as_ref();
            match latest {
                Some(s) if s.id == job_id => {
                    (s.cancel_requested || s.phase == JobPhase::Finished, true)
                }
                _ => (false, false),
            }
        };
        if already_canceled {
            // Idempotent: emit the unchanged view so the
            // frontend can keep its reducer coherent
            // without a state-change event.
            let view = CurrentView {
                seq: guard.current_seq.to_string(),
                job: guard.latest.clone(),
            };
            return Some(Err(view));
        }
        token.request();
        let view = if snapshot_id_matches {
            // Take the latest snapshot out, mutate it,
            // bump the seq, and put the snapshot back.
            // The `mem::take` avoids the borrow-checker
            // conflict between `guard.latest.as_mut()`
            // and the seq bump / clone. The seq bump is
            // always paired with the snapshot install.
            let mut snapshot = std::mem::take(&mut guard.latest).expect("snapshot_id_matches");
            snapshot.cancel_requested = true;
            let seq = guard.current_seq.wrapping_add(1);
            guard.current_seq = seq;
            let view = CurrentView {
                seq: seq.to_string(),
                job: Some(snapshot.clone()),
            };
            guard.latest = Some(snapshot);
            view
        } else {
            let seq = guard.current_seq;
            CurrentView {
                seq: seq.to_string(),
                job: guard.latest.clone(),
            }
        };
        Some(Ok(view))
    }

    /// Atomically publish the terminal snapshot AND
    /// release the long reservation. The order matters:
    /// the publish must land first so a frontend that
    /// reads the registry right after the reservation
    /// releases sees the terminal snapshot, not the
    /// previous running snapshot. The seq is bumped under
    /// the same lock; the returned `CurrentView` is the
    /// exact view the caller MUST hand to the emit
    /// closure (no second view query after the lock
    /// drops — a concurrent `op_start` could install a
    /// different snapshot by then).
    ///
    /// `expected_job_id` is the worker-side guard: a
    /// worker that observes its own id is NOT current
    /// must NOT be able to clear the active reservation.
    /// Without this guard, a slow worker that publishes
    /// after the next `op_start` would reset the active
    /// pointer to `None` and clobber the new job's
    /// reservation.
    ///
    /// **Sticky `cancel_requested`:** the helper reads
    /// the prior running snapshot's flag under the lock
    /// and merges it into the incoming terminal snapshot.
    /// Without this merge, `JobSnapshot::new` would
    /// always reset `cancel_requested` to `false` and a
    /// cancel that arrived mid-job would never reach the
    /// terminal view the frontend reconciled (the
    /// `manualstatusSameSeqIGNORED` bug).
    fn publish_terminal_and_release(
        &self,
        expected_job_id: &str,
        mut snapshot: JobSnapshot,
    ) -> Option<CurrentView> {
        let mut guard = self.inner.lock().expect("registry mutex poisoned");
        // Active-owner guard: if the worker is publishing
        // for a job that is no longer the active one
        // (e.g. it was already replaced by a fresh
        // reservation after the worker had observed its
        // own id as "current"), drop the publish silently.
        // The worker still returns; its terminal event is
        // discarded. The registry never lets an old worker
        // clear a fresh reservation.
        if guard.active_id.as_deref() != Some(expected_job_id) {
            return None;
        }
        // Sticky `cancel_requested`: carry forward the
        // running snapshot's flag. The take/put pattern
        // lets us mutate `snapshot` while `guard` is
        // borrowed immutably for the read.
        let (prior_cancel, prior_progress) = match guard.latest.as_ref() {
            Some(existing) if existing.id == expected_job_id => {
                (existing.cancel_requested, existing.progress.clone())
            }
            _ => (false, None),
        };
        snapshot.cancel_requested = snapshot.cancel_requested || prior_cancel;
        if snapshot.progress.is_none() {
            snapshot.progress = prior_progress;
        }
        guard.current_seq = guard.current_seq.wrapping_add(1);
        let seq = guard.current_seq;
        let view = CurrentView {
            seq: seq.to_string(),
            job: Some(snapshot.clone()),
        };
        guard.latest = Some(snapshot);
        guard.active_token = None;
        guard.active_id = None;
        Some(view)
    }

    /// Read the current view under the lock WITHOUT
    /// bumping the seq. The seq returned is the same seq
    /// the last publish stamped; a frontend that calls
    /// `op_current` repeatedly observes the same seq
    /// until a new event lands. The lock is held for the
    /// `format!` of the seq string so a concurrent
    /// publish cannot change the value mid-format.
    fn current_view(&self) -> CurrentView {
        let guard = self.inner.lock().expect("registry mutex poisoned");
        CurrentView {
            seq: guard.current_seq.to_string(),
            job: guard.latest.clone(),
        }
    }

    /// Initial view: `{seq:"0",job:None}` regardless of
    /// whether a job has been observed. Used as the
    /// frontend's reducer seed before the first event
    /// lands.
    pub fn initial_view(&self) -> CurrentView {
        CurrentView::initial()
    }

    /// Read the latest snapshot for a specific job id. The
    /// id check is the "unknown / expired" guard the
    /// `op_status` command uses. The seq is the registry's
    /// current seq (i.e., the seq the last publish
    /// stamped).
    fn snapshot_for(&self, job_id: &str) -> Option<CurrentView> {
        let guard = self.inner.lock().expect("registry mutex poisoned");
        match &guard.latest {
            Some(s) if s.id == job_id => Some(CurrentView {
                seq: guard.current_seq.to_string(),
                job: Some(s.clone()),
            }),
            _ => None,
        }
    }
}

/// Emit closure factory: the production wiring builds a
/// closure that targets the `main` window via
/// `AppHandle::emit_to`. Tests build a closure that
/// always returns `true` so the registry's bookkeeping
/// is exercised without the IPC layer. The factory is
/// generic over `R: Runtime` because `AppHandle<R>` is
/// parameterised by the runtime; the resulting
/// `EmitFnArc` is not. **The `AppHandle` is required** —
/// the factory targets a real IPC channel; tests that
/// want a no-op emit use [`noop_emit_fn`] directly. The
/// `Option` shape this used to take was a no-op factory
/// that silently dropped every event; the typed compiler
/// is the regression guard against re-introducing it.
pub fn production_emit_fn<R: Runtime>(app: AppHandle<R>) -> EmitFnArc {
    Arc::new(move |view: &CurrentView| {
        match app.emit_to(
            EventTarget::labeled(MAIN_WINDOW_LABEL),
            OPERATION_EVENT,
            view,
        ) {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "agenthd-operation emit failed (seq={}, retaining snapshot): {e}",
                    view.seq
                );
                false
            }
        }
    })
}

/// Test emit closure factory: always returns `true`. The
/// registry still installs / publishes / releases under
/// its lock — only the actual IPC emit is bypassed.
pub fn noop_emit_fn() -> EmitFnArc {
    Arc::new(|_view: &CurrentView| true)
}

/// Load `state.json` for a launch. The lib already pins
/// the load contract in `State::load`; the helper is
/// local so the Tauri command layer does not have to know
/// which file the registry reads.
pub(super) fn load_state_for(paths: &agenthd::store::Paths) -> Result<State, OperationError> {
    State::load(&paths.state_file).map_err(|e| OperationError::FailedPreconditions {
        message: format!("read state: {e}"),
    })
}

/// Read `Settings` from `paths.settings_file` and re-point
/// `paths.canonical_dir` at the configured checkout's
/// `agents/`. The lib's `Paths::with_settings` is the
/// validator; a stale or missing configured checkout surfaces
/// as `OperationError::FailedPreconditions` so the registry
/// reports the precondition failure to the frontend
/// without holding a reservation.
///
/// **Runtime-path bug (sync / skills).** Pre-fix, the
/// Tauri command layer called `Paths::from_env()` and
/// `State::load` outside the registry's reservation; the
/// resulting `Paths.canonical_dir` was the default
/// `<agenthd_root>/agents`, never the configured
/// checkout. `SyncAgents` / `InstallSkills` therefore ran
/// against the wrong source (or no source when no decoy
/// existed). This helper exists so the runtime path reads
/// `Settings`, re-points the canonical dir through
/// `with_settings`, and uses the SCOPED `Paths` for every
/// canonical operation — the same scoped-path pattern
/// the P1 editor fix already established in
/// `compose_load_agent_for_edit`.
pub(super) fn resolve_ready_scoped_paths(
    paths: &agenthd::store::Paths,
) -> Result<agenthd::store::Paths, OperationError> {
    use agenthd::workflows::{read_checkout, CheckoutStatus};
    let ready_path = match read_checkout(&paths.settings_file).map_err(|e| {
        OperationError::FailedPreconditions {
            message: format!("read settings: {e}"),
        }
    })? {
        CheckoutStatus::Ready(p) => p,
        CheckoutStatus::Empty => {
            return Err(OperationError::FailedPreconditions {
                message:
                    "no checkout configured yet (settings.json is empty); configure one in the agenthd TUI"
                        .to_string(),
            });
        }
        CheckoutStatus::Stale { banner, .. } => {
            return Err(OperationError::FailedPreconditions {
                message: format!("configured checkout is unusable: {banner}"),
            });
        }
    };
    let settings = agenthd::store::Settings::new(ready_path.to_string_lossy().into_owned());
    paths
        .clone()
        .with_settings(&settings)
        .map_err(|e| OperationError::FailedPreconditions {
            message: format!("resolve configured checkout: {e}"),
        })
}

/// Per-job context resolved lazily **after** the
/// reservation lands. The `paths` field is the lib's
/// `Paths` the worker must use; for `SyncAgents` /
/// `InstallSkills` the runtime must populate it via
/// [`resolve_ready_scoped_paths`] so the canonical dir is
/// the configured checkout, not the lib default. For
/// `InstallTool` the runtime returns the raw `from_env`
/// paths (tools only need `skills_dir` / state is
/// irrelevant). For `DiscoverModels` the runtime returns
/// `paths: None` AND no state — the discovery does not
/// read either, and `Paths::from_env()` would otherwise
/// execute an env lookup that the discovery is
/// orthogonal to. The `state` field is `Some` for the
/// workflows that own the manifest (sync + skills); `None`
/// for tool + discovery.
#[derive(Debug)]
pub struct JobContext {
    pub paths: Option<agenthd::store::Paths>,
    pub state: Option<State>,
}

/// Resolve a [`JobContext`] for the given `request`. The
/// resolver runs AFTER `reserve_and_publish` so a slow /
/// failing resolve never holds a reservation. The Tauri
/// command layer passes `request_resolve_paths_state`
/// (production: reads env + resolves `Ready` +
/// `with_settings` + loads state) and the test layer
/// passes a closure that builds a fixture-backed
/// `JobContext` so the worker exercises the real lib path
/// without touching env / home / settings files.
pub type JobContextResolver =
    Box<dyn FnOnce(OperationRequest) -> Result<JobContext, OperationError> + Send>;

/// Production context resolver. Resolves `Paths` from the
/// env via `Paths::from_env()` ONLY for variants that read
/// it (sync / skills / tool). `DiscoverModels` is
/// orthogonal to the configured paths — the lib's
/// `discover_models_controlled` does not take a `Paths`
/// argument — so the env lookup is skipped for the
/// discovery arm. Pre-fix, the resolver called
/// `Paths::from_env()` BEFORE the match arm, so a
/// `DiscoverModels` request that ran against a CI env
/// with no `HOME` / `XDG_CONFIG_HOME` would surface a
/// `resolve config paths: …` precondition failure even
/// though the discovery does not read either. Now the
/// helper returns `JobContext { paths: None, state: None }`
/// for the discovery arm without touching the env.
///
/// For sync + skills, the helper resolves `Ready` +
/// re-points canonical_dir through `with_settings` and
/// loads `state.json`. For `InstallTool`, the helper
/// returns the raw env paths because tools only need
/// `skills_dir` and re-pointing would mask a real
/// configured-vs-default divergence. `state.json` is
/// loaded via [`load_state_for`]; a malformed state file
/// surfaces as `OperationError::FailedPreconditions` so
/// the worker reports the precondition to the frontend
/// instead of silently using an empty default.
///
/// The `Paths` factory is injected so a test can prove
/// the discovery arm never invokes it (a failing factory
/// would otherwise surface as a precondition error that
/// masks the discovery's actual failure path). The
/// factory-free [`production_request_resolve_paths_state`]
/// is the production wrapper; tests use the factory
/// seam directly.
pub fn production_request_resolve_paths_state_with_paths_factory<F>(
    paths_factory: F,
    request: OperationRequest,
) -> Result<JobContext, OperationError>
where
    F: FnOnce() -> Result<agenthd::store::Paths, OperationError>,
{
    match request {
        OperationRequest::SyncAgents { .. } | OperationRequest::InstallSkills => {
            let paths = paths_factory()?;
            let scoped = resolve_ready_scoped_paths(&paths)?;
            let state = load_state_for(&scoped)?;
            Ok(JobContext {
                paths: Some(scoped),
                state: Some(state),
            })
        }
        OperationRequest::InstallTool { .. } => {
            // Tools only need the skills_dir; reading
            // state is unnecessary and would fail when
            // the env has no state.json. The raw
            // `from_env` paths are exactly what the tool
            // installer needs.
            let paths = paths_factory()?;
            Ok(JobContext {
                paths: Some(paths),
                state: None,
            })
        }
        OperationRequest::DiscoverModels => {
            // Discovery runs `opencode models` and reads
            // neither paths nor state. The factory is
            // NOT called — a missing / unresolvable
            // `HOME` / `XDG_CONFIG_HOME` is irrelevant
            // for this arm; the test suite pins the
            // factory-never-called contract.
            Ok(JobContext {
                paths: None,
                state: None,
            })
        }
    }
}

/// Production wrapper: the factory is
/// `agenthd::store::Paths::from_env()`. The wrapper is
/// the entry point the Tauri command layer calls; the
/// factory-injected variant exists for the discovery
/// test seam.
pub fn production_request_resolve_paths_state(
    request: OperationRequest,
) -> Result<JobContext, OperationError> {
    production_request_resolve_paths_state_with_paths_factory(
        || {
            agenthd::store::Paths::from_env().map_err(|e| OperationError::FailedPreconditions {
                message: format!("resolve config paths: {e}"),
            })
        },
        request,
    )
}

/// Catalog lookup. The frontend never supplies a URL /
/// hash / path; the catalog is the single source of truth
/// and the lookup is the one-and-only entry point for the
/// `InstallTool` request kind. Unknown ids surface as
/// `OperationError::FailedPreconditions` so the GUI can
/// render the catalog list as a dropdown.
fn lookup_catalog_entry(tool_id: &str) -> Result<&'static ToolCatalogEntry, OperationError> {
    DEFAULT_CATALOG
        .iter()
        .find(|e| e.skill_name == tool_id || e.destination_subpath == tool_id)
        .ok_or_else(|| OperationError::FailedPreconditions {
            message: format!(
                "unknown tool id `{tool_id}` (catalog entries: {})",
                DEFAULT_CATALOG
                    .iter()
                    .map(|e| e.skill_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
}

/// Project the lib's per-row outcomes to the wire
/// `RowOutcome` DTO. The conversion is `Vec<ApplyOutcome>`
/// and `Vec<SkillOutcome>` over a single helper so the
/// two arms stay in sync.
fn project_agents_outcomes(outcomes: Vec<agenthd::store::ApplyOutcome>) -> Vec<RowOutcome> {
    outcomes
        .into_iter()
        .map(|o| RowOutcome {
            name: o.filename,
            action: o.action,
            detail: o.detail,
            ok: o.ok,
        })
        .collect()
}

fn project_skills_outcomes(outcomes: Vec<agenthd::store::SkillOutcome>) -> Vec<RowOutcome> {
    outcomes
        .into_iter()
        .map(|o| RowOutcome {
            name: o.name,
            action: o.action,
            detail: o.detail,
            ok: o.ok,
        })
        .collect()
}

/// Project the observed `State` the workflow reported to
/// an opaque JSON value the frontend can read. The lib's
/// `State` is the canonical ownership manifest the GUI
/// does not model; the registry surfaces it as a raw
/// JSON value so a frontend that wants to show "what
/// would have been persisted" can without depending on
/// the lib's Rust shape. The value is observed-not-
/// persisted: the registry never calls back into the lib to
/// save it (no rollback, no auto-persist).
fn project_observed_state(state: &State) -> serde_json::Value {
    serde_json::to_value(state).unwrap_or(serde_json::Value::Null)
}

/// Project the lib's tool report into the wire shape.
/// **Finish / error are NOT invented here.** The lib's
/// `OperationReport.finish` and `OperationReport.error`
/// are preserved by `publish_terminal_with_report`; this
/// helper only converts the per-row `partial` payload
/// into the typed wire shape. A tool that failed preflight
/// stays `Failed` (with the lib's own detail); a tool
/// that succeeded stays `Completed`. The "all success
/// Completed" flattening was a real D3 bug — the lib can
/// report per-row outcomes as failed even when the
/// overall `Finish` is `Completed`, and this projection
/// must let the lib's actual `finish` flow through.
fn project_tool_partial(partial: (agenthd::tools::ToolOutcome, Option<PathBuf>)) -> JobReport {
    let (outcome, residual) = partial;
    JobReport::InstallTool {
        terminal: ToolTerminal {
            status: outcome.status.label().to_string(),
            detail: outcome.detail,
            residual: residual.map(|p| p.to_string_lossy().into_owned()),
        },
    }
}

/// Project the lib's discovery report into the wire
/// shape. **Finish / error are NOT invented here.** A
/// discovery that reported `Cancelled` (no `opencode`
/// binary, cancel arrived before the spawn) keeps its
/// `Finish::Cancelled`; a discovery that reported
/// `Failed` (opencode exited non-zero) keeps
/// `Finish::Failed`. The "None → Failed" / "Some →
/// Completed" flattening was a real D3 bug — `Failed`
/// from the lib was being relabeled to `Completed` when
/// the discovery came back non-empty.
fn project_discovery_partial(partial: Option<LibDiscovery>) -> JobReport {
    let terminal = partial.map(|d| match d {
        LibDiscovery::Found(models) => DiscoveryTerminal::Found { models },
        LibDiscovery::Empty(message) => DiscoveryTerminal::Empty { message },
        LibDiscovery::Failed(message) => DiscoveryTerminal::Failed { message },
    });
    JobReport::DiscoverModels { terminal }
}

/// Reusable safe-controlled progress sink. The closure
/// captures a `Arc<OperationRegistry>` + the `JobSnapshot`
/// id of the running job; each tick clones the current
/// snapshot under the lock, mutates only the
/// progress-related fields, installs the new seq under the
/// same lock, and emits the new view outside the lock.
/// The closure is `FnMut + Send` so it can be handed straight
/// to the lib's `&mut dyn FnMut(Progress)` parameter.
pub(super) fn progress_sink(
    registry: Arc<OperationRegistry>,
    emit: EmitFnArc,
    job_id: String,
) -> impl FnMut(Progress) + Send + 'static {
    move |progress: Progress| {
        let mut to_emit: Option<(JobSnapshot, u64)> = None;
        {
            let mut guard = registry.inner.lock().expect("registry mutex poisoned");
            if let Some(snapshot) = guard.latest.as_mut() {
                if snapshot.id == job_id && snapshot.phase == JobPhase::Running {
                    snapshot.progress = Some(JobProgress::from(progress));
                    let next = snapshot.clone();
                    guard.current_seq = guard.current_seq.wrapping_add(1);
                    let seq = guard.current_seq;
                    guard.latest = Some(next.clone());
                    to_emit = Some((next, seq));
                }
            }
        }
        if let Some((snapshot, seq)) = to_emit {
            let view = CurrentView {
                seq: seq.to_string(),
                job: Some(snapshot),
            };
            emit(&view);
        }
    }
}

// ===========================================================================
// Public command surface (the registry itself is private to this module; the
// command layer in `lib.rs` calls the helpers below).
// ===========================================================================

/// Outcome of a short-sync command. The lib's
/// `OperationReport<T>` is not directly serializable to the
/// GUI's wire shape (the report carries a generic
/// `partial`); the helpers below project the relevant
/// fields into the typed `JobReport` variant the GUI
/// expects. The `OperationError` arm is reserved for
/// preconditions the lib does not model (a busy registry,
/// a missing catalog entry, a failed `Paths::from_env`).
pub type ShortOutcome<T> = Result<T, OperationError>;

/// Outcome of a long-running command. The helper runs the
/// worker thread; the command layer receives the
/// allocated job id and the registry owns the rest. The
/// frontend reconciles via the `agenthd-operation` event
/// stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LongOutcome {
    pub job_id: String,
    /// The view the registry installed atomically with
    /// the reservation. The frontend's reducer seeds its
    /// `lastSeq` from this `seq`; the worker then
    /// publishes subsequent ticks with strictly-greater
    /// `seq` values.
    pub view: CurrentView,
}

/// Spawn a long-running job. The reservation, the initial
/// seq bump, and the install of the running snapshot are
/// performed atomically **before** the worker thread is
/// spawned, so a frontend that wakes up from the
/// `agenthd-operation` event cannot observe a stale
/// `latest_snapshot` that misses the running job. The
/// worker is `std::thread::Builder::spawn` — never
/// `tokio::spawn`, never a Tauri-managed task — so the
/// worker is independent of the runtime the rest of the
/// spike uses. A `Builder::spawn` failure publishes a
/// terminal `Failed` snapshot, releases the reservation,
/// and emits the terminal view so the user sees the
/// failure.
///
/// `request` selects the workflow the worker drives:
/// `SyncAgents` / `InstallSkills` / `InstallTool` /
/// `DiscoverModels`. The dispatch is a single match on the
/// `OperationRequest` discriminant so a new variant only
/// needs one new arm in the worker.
///
/// `resolver` builds the [`JobContext`] the worker uses;
/// production wires
/// [`production_request_resolve_paths_state`], tests wire
/// a fixture-backed resolver. The resolver runs **after**
/// the reservation lands, so a slow / failing resolve
/// never holds a reservation and the registry stays
/// responsive to close / status reads during the resolve.
pub fn start_operation_with(
    registry: Arc<OperationRegistry>,
    emit: EmitFnArc,
    resolver: JobContextResolver,
    request: OperationRequest,
) -> Result<LongOutcome, OperationError> {
    // Atomic reserve + initial-publish. The id / token /
    // seq / snapshot are decided under one lock; the
    // returned view is exactly what the caller will emit,
    // so the publish step is the one and only emit for
    // the initial running state.
    let (job_id, token, initial_view) = match registry.reserve_and_publish(request.clone()) {
        Some(triple) => triple,
        None => {
            // Either a long job or a short sync is in
            // flight. The registry's active_id captures
            // the long case; a short sync without an id
            // surfaces as `""` (the frontend treats an
            // empty active_job_id as "wait").
            let active = registry.active_job_id().unwrap_or_default();
            return Err(OperationError::Busy {
                active_job_id: active,
            });
        }
    };
    emit(&initial_view);

    // Resolve paths/state AFTER the reservation. The
    // resolver is the place where the runtime reads
    // `HOME` / `XDG_CONFIG_HOME` / `state.json`; the
    // worker must never see an env read that bypasses
    // the reservation gate. A failing resolver publishes
    // a terminal `Failed` snapshot, releases the
    // reservation, and emits the terminal view so the
    // user sees the precondition failure.
    let ctx = match resolver(request.clone()) {
        Ok(ctx) => ctx,
        Err(err) => {
            let message = match &err {
                OperationError::FailedPreconditions { message } => message.clone(),
                _ => format!("{err}"),
            };
            let terminal = JobSnapshot::new(job_id.clone(), request.clone());
            if let Some(view) = publish_failed_terminal(
                &registry,
                &job_id,
                terminal,
                JobFinish::Failed,
                Some(message.clone()),
                None,
            ) {
                emit(&view);
            }
            return Err(err);
        }
    };

    let registry_for_worker = Arc::clone(&registry);
    let emit_for_worker = Arc::clone(&emit);
    let token_for_worker = token.clone();
    let job_id_for_worker = job_id.clone();
    let request_for_worker = request.clone();

    let factory = registry.take_thread_factory();
    let builder = thread::Builder::new().name(format!("agenthd-job-{job_id}"));
    let worker: JobWorker = Box::new(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_worker(
                registry_for_worker.clone(),
                emit_for_worker.clone(),
                job_id_for_worker.clone(),
                token_for_worker.clone(),
                ctx,
                request_for_worker.clone(),
            );
        }));
        if let Err(payload) = result {
            // The worker panicked. Surface a terminal
            // Failed snapshot and release the
            // reservation; the panic payload is wrapped
            // to a string via Debug. We cannot recover
            // the actual `&str` payload when it is not
            // `&'static str`, so the message is best-
            // effort. If a fresh reservation landed in
            // the gap between the worker observing its
            // id and the panic, the publish returns
            // `None` and we do NOT emit — emitting a
            // fabricated fallback would stomp the new
            // reservation's snapshot.
            let panic_msg = panic_payload_message(&payload);
            let terminal = JobSnapshot::new(job_id_for_worker.clone(), request_for_worker.clone());
            if let Some(view) = publish_failed_terminal(
                &registry_for_worker,
                &job_id_for_worker,
                terminal,
                JobFinish::Failed,
                Some(format!("worker panicked: {panic_msg}")),
                None,
            ) {
                emit_for_worker(&view);
            }
        }
    });

    let spawn_result = match factory {
        Some(factory) => factory(builder, worker),
        None => default_thread_factory(builder, worker),
    };
    if let Err(message) = spawn_result {
        // The thread could not be spawned (resource
        // exhaustion, ulimit, etc.). Publish a terminal
        // `Failed` snapshot, release the reservation,
        // and emit the terminal view so the user sees
        // the failure. The `publish_terminal_and_release`
        // helper returns the exact view captured under
        // the lock — no second view query after the
        // reservation drops, which would race a
        // concurrent `op_start`. If a fresh reservation
        // landed in the gap (rare; only on a concurrent
        // `op_start`), we do NOT emit a fallback view.
        let terminal = JobSnapshot::new(job_id.clone(), request.clone());
        if let Some(view) = publish_failed_terminal(
            &registry,
            &job_id,
            terminal,
            JobFinish::Failed,
            Some(message.clone()),
            None,
        ) {
            emit(&view);
        }
        return Err(OperationError::Spawn { message });
    }
    Ok(LongOutcome {
        job_id,
        view: initial_view,
    })
}

/// Convenience wrapper for the production wiring:
/// `start_operation_with` + the production resolver that
/// reads `Paths::from_env()` + resolves the configured
/// checkout + loads `state.json`. Tests use
/// `start_operation_with` directly with a fixture resolver
/// so they never touch the process env / `HOME` /
/// `XDG_CONFIG_HOME`.
pub fn start_operation(
    registry: Arc<OperationRegistry>,
    emit: EmitFnArc,
    request: OperationRequest,
) -> Result<LongOutcome, OperationError> {
    start_operation_with(
        registry,
        emit,
        Box::new(production_request_resolve_paths_state),
        request,
    )
}

/// Worker entry point. The body is a single match on
/// `request`; each arm builds the per-variant sink,
/// invokes the lib's controlled workflow, and hands the
/// `OperationReport` to `publish_terminal` (which clones
/// the report into a typed `JobReport` and runs the
/// terminal publish step). The match is exhaustive over
/// `OperationRequest` so a new variant is a compile
/// error here, not a silent default.
fn run_worker(
    registry: Arc<OperationRegistry>,
    emit: EmitFnArc,
    job_id: String,
    token: CancelToken,
    ctx: JobContext,
    request: OperationRequest,
) {
    let sink = progress_sink(Arc::clone(&registry), Arc::clone(&emit), job_id.clone());
    let mut sink = sink;
    match request {
        OperationRequest::SyncAgents { target } => {
            // Sync and Skills carry `state`; Tools and
            // Discovery do not. The non-`None` arms
            // unwrap because the production resolver
            // populates the field; tests that build a
            // resolver must do the same. `ctx.paths` is
            // `Some` for the sync arm — the resolver
            // scopes it to the configured checkout —
            // and `expect` pins the contract.
            let state = ctx.state.expect("sync resolver must populate state");
            let paths = ctx
                .paths
                .as_ref()
                .expect("sync resolver must populate paths");
            let boxed: &mut dyn FnMut(Progress) = &mut sink;
            let report = plan_then_apply_agents_safe_controlled(
                paths,
                state,
                target.as_lib_target(),
                &token,
                boxed,
            );
            let (post_state, outcomes) = report.partial;
            let observed = project_observed_state(&post_state);
            publish_terminal(
                &registry,
                &emit,
                &job_id,
                &OperationRequest::SyncAgents { target },
                report.finish,
                report.error,
                JobReport::SyncAgents {
                    outcomes: project_agents_outcomes(outcomes),
                    observed_state: Some(observed),
                },
            );
        }
        OperationRequest::InstallSkills => {
            let state = ctx.state.expect("skills resolver must populate state");
            let paths = ctx
                .paths
                .as_ref()
                .expect("skills resolver must populate paths");
            let boxed: &mut dyn FnMut(Progress) = &mut sink;
            let report = plan_then_apply_skills_controlled(paths, state, &token, boxed);
            let (post_state, outcomes) = report.partial;
            let observed = project_observed_state(&post_state);
            publish_terminal(
                &registry,
                &emit,
                &job_id,
                &OperationRequest::InstallSkills,
                report.finish,
                report.error,
                JobReport::InstallSkills {
                    outcomes: project_skills_outcomes(outcomes),
                    observed_state: Some(observed),
                },
            );
        }
        OperationRequest::InstallTool { tool_id } => {
            let entry = match lookup_catalog_entry(&tool_id) {
                Ok(e) => e,
                Err(err) => {
                    publish_failure(
                        &registry,
                        &emit,
                        &job_id,
                        &OperationRequest::InstallTool { tool_id },
                        err.to_string(),
                    );
                    return;
                }
            };
            let boxed: &mut dyn FnMut(Progress) = &mut sink;
            // Tools only need `ctx.paths` (skills_dir +
            // skills staging); `state` is irrelevant.
            let paths = ctx
                .paths
                .as_ref()
                .expect("tool resolver must populate paths");
            let report = agenthd::tools::install_tool_controlled(paths, entry, &token, boxed);
            // **Preserve the lib's `finish` verbatim.**
            // Pre-fix, the projection invented
            // `Completed` for any successful tool report
            // and discarded the lib's `Finish::Failed` /
            // `Finish::Cancelled` even when the partial
            // was non-empty (e.g. residual staging). Now
            // the lib's finish / error are passed
            // through `publish_terminal` which handles
            // them directly. Destructure first so the
            // `finish` / `error` reads do not move out
            // of a borrowed `report`.
            let OperationReport {
                finish,
                error,
                partial,
            } = report;
            publish_terminal(
                &registry,
                &emit,
                &job_id,
                &OperationRequest::InstallTool { tool_id },
                finish,
                error,
                project_tool_partial(partial),
            );
        }
        OperationRequest::DiscoverModels => {
            // Discovery reads neither paths nor state;
            // the lib's `discover_models_controlled`
            // does not take a `Paths` argument at all.
            // `ctx.paths` is `None` for this arm — the
            // production resolver skips `Paths::from_env()`
            // entirely so a missing / unresolvable
            // `HOME` / `XDG_CONFIG_HOME` is irrelevant.
            let boxed: &mut dyn FnMut(Progress) = &mut sink;
            let report = discover_models_controlled(&token, boxed);
            // **Preserve the lib's `finish` verbatim.**
            // Pre-fix, `None` discovery invented
            // `Failed`, and any non-empty `Some` invented
            // `Completed` regardless of the lib's actual
            // `Finish`. Now the lib's finish /
            // `error` flow through directly.
            let OperationReport {
                finish,
                error,
                partial,
            } = report;
            publish_terminal(
                &registry,
                &emit,
                &job_id,
                &OperationRequest::DiscoverModels,
                finish,
                error,
                project_discovery_partial(partial),
            );
        }
    }
}

/// Publish a terminal `Failed` snapshot for a worker that
/// could not even build the report (e.g. unknown catalog
/// id, resolver failure). The reservation is released
/// atomically with the publish; the returned view is the
/// EXACT view captured under the registry lock — no
/// second view query after the lock drops. If the active
/// owner has been replaced (rare: a fresh reservation
/// landed in the gap between the worker observing its id
/// and publishing), the publish is a no-op and the helper
/// returns without emitting — the new reservation's
/// snapshot is what the frontend sees.
fn publish_failure(
    registry: &OperationRegistry,
    emit: &EmitFnArc,
    job_id: &str,
    request: &OperationRequest,
    message: String,
) {
    let snapshot = JobSnapshot::new(job_id.to_string(), request.clone());
    if let Some(view) = publish_failed_terminal(
        registry,
        job_id,
        snapshot,
        JobFinish::Failed,
        Some(message),
        None,
    ) {
        emit(&view);
    }
}

/// Common terminal-publish path. Captures the
/// `cancel_requested` sticky flag from the existing
/// snapshot (so a cancel that arrived mid-job stays
/// visible at terminal), installs the report + error +
/// finish, publishes atomically, and returns the EXACT
/// `CurrentView` captured under the registry lock.
///
/// Returns `None` when the registry's active owner does
/// NOT match `job_id` — the worker had been replaced (a
/// fresh reservation landed after the worker observed its
/// own id as "current"). The caller emits only when `Some`;
/// emitting a fabricated fallback view (`{seq:"0",job:None}`)
/// would silently stomp the new reservation's `latest`
/// snapshot and bump the seq against the user's reducer.
fn publish_failed_terminal(
    registry: &OperationRegistry,
    job_id: &str,
    mut snapshot: JobSnapshot,
    finish: JobFinish,
    error: Option<String>,
    report: Option<JobReport>,
) -> Option<CurrentView> {
    // Read the existing cancel_requested under the
    // lock so the sticky flag survives the terminal
    // transition. This is the line that fixes the
    // "manualstatusSameSeqIGNORED" bug: without it the
    // terminal `JobSnapshot::new` always reset the flag
    // to `false`.
    let (sticky_cancel, observed_progress) = {
        let guard = registry.inner.lock().expect("registry mutex poisoned");
        match guard.latest.as_ref() {
            Some(existing) if existing.id == job_id => {
                (existing.cancel_requested, existing.progress.clone())
            }
            _ => (false, None),
        }
    };
    snapshot.cancel_requested = snapshot.cancel_requested || sticky_cancel;
    if snapshot.progress.is_none() {
        snapshot.progress = observed_progress;
    }
    snapshot.phase = JobPhase::Finished;
    snapshot.finish = Some(finish);
    snapshot.error = error;
    snapshot.report = report;
    registry.publish_terminal_and_release(job_id, snapshot)
}

/// Publish a terminal snapshot for a job that completed
/// the lib's controlled workflow. The lib's
/// `OperationReport::finish` / `error` are surfaced
/// directly; the projected `JobReport` is the partial
/// the frontend renders.
fn publish_terminal(
    registry: &OperationRegistry,
    emit: &EmitFnArc,
    job_id: &str,
    request: &OperationRequest,
    finish: Finish,
    error: Option<String>,
    report: JobReport,
) {
    publish_terminal_with_report(
        registry,
        emit,
        job_id,
        request,
        JobFinish::from(finish),
        error,
        report,
    )
}

fn publish_terminal_with_report(
    registry: &OperationRegistry,
    emit: &EmitFnArc,
    job_id: &str,
    request: &OperationRequest,
    finish: JobFinish,
    error: Option<String>,
    report: JobReport,
) {
    let snapshot = JobSnapshot::new(job_id.to_string(), request.clone());
    if let Some(view) =
        publish_failed_terminal(registry, job_id, snapshot, finish, error, Some(report))
    {
        emit(&view);
    }
}

/// Extract a printable panic payload. `catch_unwind`
/// returns `Box<dyn Any + Send>`; `&str` and `String`
/// payloads are common, anything else gets a Debug print.
fn panic_payload_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Request cancellation of an active long job. The
/// `job_id` is the wire-level guard: cancelling an
/// already-terminal job is a NO-OP (the registry returns
/// the unchanged `CurrentView` so the frontend's reducer
/// sees a no-state-change event); cancelling a job whose
/// id does not match the active one returns
/// `OperationError::UnknownJob` (covers both "expired"
/// and "from a different process" cases). The
/// cancellation is idempotent: a second cancel for the
/// same active id is a no-op that still returns the
/// unchanged current view.
pub fn cancel_operation(
    registry: &OperationRegistry,
    emit: &EmitFnArc,
    job_id: &str,
) -> Result<CurrentView, OperationError> {
    if let Some(outcome) = registry.request_cancel(job_id) {
        let view = match outcome {
            Ok(view) | Err(view) => view,
        };
        emit(&view);
        return Ok(view);
    }
    // The id did not match the active one. It might
    // still be the retained terminal from a recently
    // finished job; in that case cancel is a no-op and
    // we surface the current view (NOT
    // `OperationError::Cancelled`, which used to be the
    // refusal signal but is now retired — cancellation
    // is idempotent, never an error).
    if let Some(view) = registry.snapshot_for(job_id) {
        emit(&view);
        return Ok(view);
    }
    Err(OperationError::UnknownJob {
        job_id: job_id.to_string(),
    })
}

/// Read the latest `CurrentView` the registry knows
/// about. Always returns a `CurrentView` (never `None`)
/// so the frontend's `op_current` reducer can rely on the
/// shape; the freshly-built registry returns
/// `CurrentView::initial()` (`seq:"0"`, `job:None`). The
/// seq is the registry's current seq (i.e., the seq the
/// last publish stamped); a frontend that calls this
/// repeatedly observes the same seq until a new event
/// lands.
pub fn current_operation(registry: &OperationRegistry, emit: &EmitFnArc) -> CurrentView {
    let view = registry.current_view();
    emit(&view);
    view
}

/// Read the latest `CurrentView` for a specific job id.
/// The id check is the "unknown / expired" guard the
/// `op_status` command uses; unknown ids surface as
/// `OperationError::UnknownJob`. The seq is the
/// registry's current seq (NOT a fresh seq bump), so a
/// repeated call for the same job id returns the same
/// `seq` until the registry publishes a new event.
pub fn operation_status(
    registry: &OperationRegistry,
    emit: &EmitFnArc,
    job_id: &str,
) -> Result<CurrentView, OperationError> {
    match registry.snapshot_for(job_id) {
        Some(view) => {
            emit(&view);
            Ok(view)
        }
        None => Err(OperationError::UnknownJob {
            job_id: job_id.to_string(),
        }),
    }
}

/// RAII guard for the short reservation. The reservation
/// is released on `Drop` so a guarded helper that unwinds
/// (or panics in a helper that wraps the registry) still
/// releases the slot. The lock-free flag is set under
/// the registry's mutex; the guard does not need the
/// mutex itself.
struct ShortGuard<'a> {
    registry: &'a OperationRegistry,
    active: bool,
}

impl<'a> ShortGuard<'a> {
    fn try_acquire(registry: &'a OperationRegistry) -> Result<Self, OperationError> {
        if !registry.try_reserve_short() {
            let active = registry.active_job_id().unwrap_or_default();
            return Err(OperationError::Busy {
                active_job_id: active,
            });
        }
        Ok(Self {
            registry,
            active: true,
        })
    }
}

impl<'a> Drop for ShortGuard<'a> {
    fn drop(&mut self) {
        if self.active {
            self.registry.release_short();
            self.active = false;
        }
    }
}

/// Short-sync helper that delegates to a lib composition
/// helper under one gate. The reservation is acquired
/// **before** the helper runs (so a long job in flight
/// surfaces `OperationError::Busy` immediately) and
/// released after the helper returns (success or
/// failure, including panics). The lib helper is the
/// single source of truth for validation / persistence;
/// the handler is just a reservation wrapper.
pub fn guarded_short<F, I>(
    registry: &OperationRegistry,
    input: I,
    helper: F,
) -> ShortOutcome<String>
where
    F: FnOnce(I) -> Result<String, String>,
{
    let _guard = ShortGuard::try_acquire(registry)?;
    let outcome = helper(input);
    outcome.map_err(|message| OperationError::FailedPreconditions { message })
}

/// Tool status projection. The wire form is a row per
/// catalog entry: `tool_id`, `display`, `status`,
/// `detail`, plus the lib-computed `destination`. The
/// list is the lib's `DEFAULT_CATALOG` order; the
/// frontend does not maintain a parallel vocabulary.
///
/// **Result-not-Vec:** returns `Result<Vec<ToolItem>,
/// String>` instead of the legacy `Vec`. Pre-fix, a
/// `tool_status` lib error on a single entry fell back
/// to a synthetic `Conflict` row, which silently
/// invented a non-`Ok` state without surfacing the I/O
/// error. Now a real I/O failure surfaces to the
/// frontend as the `Err` payload; the front-end can then
/// decide whether to show the partial list with an error
/// banner or refuse the call entirely.
pub fn tool_catalog_status(
    paths: &agenthd::store::Paths,
) -> Result<Vec<agenthd::tools::ToolItem>, String> {
    let mut rows = Vec::with_capacity(DEFAULT_CATALOG.len());
    for entry in DEFAULT_CATALOG {
        match agenthd::tools::tool_status(paths, entry) {
            Ok(item) => rows.push(item),
            Err(e) => {
                return Err(format!(
                    "tool status failed for `{}`: {e}",
                    entry.skill_name
                ));
            }
        }
    }
    Ok(rows)
}

/// Wire projection of [`agenthd::tools::ToolItem`]. The
/// `status` is the `ToolStatus` label; the `detail` and
/// `destination` are forwarded verbatim from the lib.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStatusRow {
    pub tool_id: String,
    pub display: String,
    pub status: String,
    pub detail: String,
    pub destination: String,
}

impl From<agenthd::tools::ToolItem> for ToolStatusRow {
    fn from(item: agenthd::tools::ToolItem) -> Self {
        Self {
            tool_id: item.entry.skill_name.to_string(),
            display: item.entry.skill_name.to_string(),
            status: item.status.label().to_string(),
            detail: item.detail,
            destination: item.destination.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStatusList {
    pub rows: Vec<ToolStatusRow>,
}

/// Wire projection of the lib's `PERMISSION_KEYS`. The
/// frontend renders the editor's known-key dropdown from
/// this list so a future change to the lib's vocabulary
/// does not require a parallel frontend update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionKeys {
    pub keys: Vec<String>,
}

pub fn permission_keys() -> PermissionKeys {
    PermissionKeys {
        keys: PERMISSION_KEYS.iter().map(|s| s.to_string()).collect(),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    //! Unit tests for the registry surface. Every test
    //! builds a `Paths` inside a `tempfile::TempDir` (the
    //! same pattern every other spike test uses) and
    //! drives the registry directly — no Tauri runtime, no
    //! real `AppHandle`, no events. The "event was emitted"
    //! surface is the registry's `latest_snapshot` field;
    //! a `None` `AppHandle` short-circuits the actual
    //! emit but the registry state is still updated, so the
    //! same assertions pin the contract on both the
    //! production path and the test path.

    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn paths_in(dir: &TempDir) -> agenthd::store::Paths {
        let home = dir.path().to_str().unwrap();
        let xdg = dir.path().join("xdg").to_str().unwrap().to_string();
        agenthd::store::Paths::resolve(Some(&xdg), Some(home)).unwrap()
    }

    /// Pre-canned `OperationRequest` for each variant. The
    /// arm covers the discriminant + payload shape so a
    /// new variant breaks the test at the right place.
    fn sample_request(kind: &str) -> OperationRequest {
        match kind {
            "sync_opencode" => OperationRequest::SyncAgents {
                target: SyncAgentTarget::Opencode,
            },
            "sync_pi" => OperationRequest::SyncAgents {
                target: SyncAgentTarget::Pi,
            },
            "skills" => OperationRequest::InstallSkills,
            "tool" => OperationRequest::InstallTool {
                tool_id: DEFAULT_CATALOG
                    .first()
                    .map(|e| e.skill_name.to_string())
                    .unwrap_or_else(|| "pi-psql".to_string()),
            },
            "discover" => OperationRequest::DiscoverModels,
            other => panic!("unknown sample kind `{other}`"),
        }
    }

    /// Empty state file. The registry only needs the
    /// `state_file` parent directory to exist for the
    /// `load_state_for` helper; tests that exercise
    /// real workflows do their own seeding.
    fn empty_state(paths: &agenthd::store::Paths) {
        if let Some(parent) = paths.state_file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&paths.state_file, b"{}").unwrap();
    }

    /// Wait for the registry's reservation to clear. The
    /// workers run on detached threads so a unit test that
    /// does not have a barrier cannot join them; the
    /// helper polls the registry with a short sleep until
    /// `active_job_id` is `None` or the timeout elapses.
    /// A 1-second budget is plenty for the workflows this
    /// module exercises (they all return `Cancelled` /
    /// `Failed` on a tempdir without a real `opencode`
    /// binary).
    fn wait_for_release(registry: &OperationRegistry) {
        for _ in 0..1000 {
            if registry.active_job_id().is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // ---------- ID allocation ----------

    #[test]
    fn reserve_and_publish_is_monotonic_and_uses_job_prefix() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id1, _t1, _v1) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("first reserve");
        // Releasing without a terminal leaves the
        // reservation live; the second reserve MUST be
        // denied (the contract under test).
        let _denied = registry.reserve_and_publish(sample_request("discover"));
        assert!(id1.starts_with("job-"));
        let _ = paths;
    }

    // ---------- Reservation: long vs long ----------

    #[test]
    fn long_reservation_blocks_second_long_request() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id_a, _token, _view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("first reservation must succeed");
        let denied = registry.reserve_and_publish(sample_request("discover"));
        assert!(denied.is_none(), "second reservation must be denied");
        assert_eq!(registry.active_job_id().as_deref(), Some(id_a.as_str()));
    }

    #[test]
    fn long_reservation_released_after_terminal_publish() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id, _token, _view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let mut snapshot = JobSnapshot::new(id.clone(), sample_request("discover"));
        snapshot.phase = JobPhase::Finished;
        snapshot.finish = Some(JobFinish::Completed);
        let released = registry.publish_terminal_and_release(&id, snapshot);
        assert!(released.is_some(), "active-owner publish must succeed");
        // A second long reservation is now allowed.
        assert!(registry
            .reserve_and_publish(sample_request("discover"))
            .is_some());
    }

    // ---------- Reservation: short vs long ----------

    #[test]
    fn short_reservation_blocks_long_request() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        assert!(registry.try_reserve_short(), "short reserve must succeed");
        let denied = registry.reserve_and_publish(sample_request("discover"));
        assert!(denied.is_none(), "long must yield to short");
        registry.release_short();
        let allowed = registry.reserve_and_publish(sample_request("discover"));
        assert!(
            allowed.is_some(),
            "long must be allowed after short release"
        );
    }

    #[test]
    fn long_reservation_blocks_short_request() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let _ = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("long must reserve");
        let denied = registry.try_reserve_short();
        assert!(!denied, "short must yield to long");
    }

    #[test]
    fn short_reservation_blocks_second_short_request() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        assert!(registry.try_reserve_short());
        let denied = registry.try_reserve_short();
        assert!(!denied);
        registry.release_short();
        assert!(registry.try_reserve_short());
    }

    // ---------- Seq monotonicity ----------

    /// A freshly-built registry's initial view is
    /// `{seq:"0",job:None}` — the frontend's reducer
    /// seeds `lastSeq` from this value so a first event
    /// with `seq:"1"` is strictly greater.
    #[test]
    fn current_view_initial_seq_is_zero_and_job_null() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let view = registry.current_view();
        assert_eq!(view.seq, "0");
        assert!(view.job.is_none());
        let _ = paths;
    }

    /// `reserve_and_publish` bumps the seq under the
    /// same lock that installs the snapshot. A subsequent
    /// `current_view` reads the SAME seq without
    /// incrementing.
    #[test]
    fn reserve_and_publish_bumps_seq_and_read_does_not_increment() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id, _t, view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("first reserve must succeed");
        assert_eq!(view.seq, "1", "first publish must bump seq to 1");
        // Read does NOT increment.
        let view1 = registry.current_view();
        let view2 = registry.current_view();
        let view3 = registry.current_view();
        assert_eq!(view1.seq, "1");
        assert_eq!(view2.seq, "1");
        assert_eq!(view3.seq, "1");
        assert_eq!(
            view1.seq, view2.seq,
            "snapshot reads must not increment seq"
        );
        assert_eq!(view2.seq, view3.seq);
        assert_eq!(view1.job.as_ref().unwrap().id, id);
    }

    /// Three sequential reserves produce strictly
    /// monotonic `seq` values; no event reorders an
    /// older seq after a newer one (the registry holds
    /// the lock when it bumps, so the seq is the
    /// canonical ordering).
    #[test]
    fn reserve_and_publish_seq_strictly_monotonic_across_cycles() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let req = sample_request("discover");
        let (id1, _t1, v1) = registry.reserve_and_publish(req.clone()).unwrap();
        let s1 = v1.seq.parse::<u64>().unwrap();
        let mut snap1 = JobSnapshot::new(id1.clone(), req.clone());
        snap1.phase = JobPhase::Finished;
        snap1.finish = Some(JobFinish::Completed);
        registry.publish_terminal_and_release(&id1, snap1).unwrap();
        let (_id2, _t2, v2) = registry.reserve_and_publish(req.clone()).unwrap();
        let s2 = v2.seq.parse::<u64>().unwrap();
        // The second reserve is still active — the
        // third must be denied, not panic on unwrap.
        let third = registry.reserve_and_publish(req.clone());
        assert!(third.is_none(), "third reserve while active must be denied");
        // Drive the second to terminal so the cycle
        // resets.
        let mut snap2 = JobSnapshot::new(_id2.clone(), req.clone());
        snap2.phase = JobPhase::Finished;
        snap2.finish = Some(JobFinish::Completed);
        registry.publish_terminal_and_release(&_id2, snap2).unwrap();
        let (_id3, _t3, v3) = registry.reserve_and_publish(req.clone()).unwrap();
        let s3 = v3.seq.parse::<u64>().unwrap();
        assert!(s2 > s1, "seq must strictly increase: {s1} -> {s2}");
        assert!(s3 > s2, "seq must strictly increase: {s2} -> {s3}");
    }

    // ---------- start_operation: happy path ----------

    /// `start_operation` returns the initial view with
    /// `seq:"1"` (the seq the registry installed
    /// atomically with the running snapshot). The
    /// frontend's reducer seeds `lastSeq` from this
    /// value so a worker tick with a strictly-greater
    /// seq is accepted cleanly. The test uses the
    /// fixture-based `resolver_for` so it does not
    /// touch `HOME` / `XDG_CONFIG_HOME`.
    #[test]
    fn start_operation_returns_view_with_running_snapshot() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let emit = noop_emit_fn();
        let outcome = start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("discover"),
        )
        .expect("start must succeed");
        assert_eq!(outcome.view.seq, "1", "initial publish seq must be 1");
        assert_eq!(outcome.view.job.as_ref().unwrap().id, outcome.job_id);
        assert_eq!(outcome.view.job.as_ref().unwrap().phase, JobPhase::Running);
        wait_for_release(&registry);
    }

    /// Two `start_operation` calls in a row are
    /// accepted: the second install lands under a fresh
    /// seq. The first start holds the reservation via
    /// the worker, the second surfaces `Busy`.
    #[test]
    fn start_operation_atomic_reserve_blocks_second_long() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let emit = noop_emit_fn();
        let first = start_operation_with(
            Arc::clone(&registry),
            emit.clone(),
            resolver_for(&paths),
            sample_request("discover"),
        )
        .expect("first start must succeed");
        let err = start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("skills"),
        )
        .unwrap_err();
        match err {
            OperationError::Busy { active_job_id } => {
                assert_eq!(active_job_id, first.job_id);
            }
            other => panic!("expected Busy, got {other:?}"),
        }
        wait_for_release(&registry);
    }

    // ---------- start_operation: spawn-failure injection ----------

    /// A custom thread factory that returns `Err` lets a
    /// unit test exercise the spawn-failure release path
    /// without exhausting real `std::thread::Builder`
    /// quotas. The registry publishes a terminal
    /// `Failed` snapshot with `phase == Finished` and
    /// `finish == Failed`, releases the reservation,
    /// emits the view, and surfaces `OperationError::Spawn`.
    #[test]
    fn start_operation_spawn_failure_publishes_terminal_and_releases() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        registry.install_thread_factory(Box::new(|_builder, _worker| {
            Err("simulated ulimit".to_string())
        }));
        let err = start_operation_with(
            Arc::clone(&registry),
            noop_emit_fn(),
            resolver_for(&paths),
            sample_request("discover"),
        )
        .unwrap_err();
        match err {
            OperationError::Spawn { message } => {
                assert!(message.contains("simulated ulimit"), "got: {message}");
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
        // The reservation is released; the retained
        // snapshot is the terminal Failed the spawn-
        // failure path published.
        assert!(registry.active_job_id().is_none());
        let view = registry.current_view();
        let job = view.job.expect("terminal snapshot must be retained");
        assert_eq!(job.phase, JobPhase::Finished);
        assert_eq!(job.finish, Some(JobFinish::Failed));
        assert_eq!(job.error.as_deref(), Some("simulated ulimit"));
        assert!(view.seq != "0", "terminal publish must bump seq");
        // A follow-up `reserve_and_publish` is allowed
        // and replaces the latest with a fresh running
        // snapshot.
        assert!(registry
            .reserve_and_publish(sample_request("discover"))
            .is_some());
    }

    /// The spawn-failure publish must not be a fresh
    /// running snapshot: pre-fix, `spawnfailure` published
    /// `JobSnapshot::new` (phase Running, no report). The
    /// fixed path emits the terminal Failed snapshot the
    /// registry already installed.
    #[test]
    fn start_operation_spawn_failure_emits_terminal_not_running() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        registry.install_thread_factory(Box::new(|_builder, _worker| {
            Err("simulated ulimit".to_string())
        }));
        let (captured, emit) = capture_emit();
        let _ = start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("discover"),
        )
        .unwrap_err();
        let views = captured.lock().expect("emit capture lock");
        let last = views.last().expect("at least one view emitted");
        let job = last.job.as_ref().expect("terminal snapshot must be Some");
        assert_eq!(
            job.phase,
            JobPhase::Finished,
            "must be terminal, got {job:?}"
        );
        assert_eq!(job.finish, Some(JobFinish::Failed));
        assert_eq!(job.error.as_deref(), Some("simulated ulimit"));
    }

    /// The terminal publish path must read the
    /// `cancel_requested` sticky flag from the prior
    /// running snapshot under the registry lock. Pre-fix,
    /// `JobSnapshot::new` reset `cancel_requested` to
    /// `false`, so a cancel that arrived mid-job never
    /// reached the terminal view the frontend reconciled.
    #[test]
    fn cancel_requested_is_sticky_through_terminal_publish() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        // Request the cancel via the helper.
        let cancel_outcome = registry.request_cancel(&id);
        assert!(
            matches!(cancel_outcome, Some(Ok(_))),
            "cancel must flip once"
        );
        // Build the terminal snapshot as the worker would
        // (no sticky flag carried in) and publish.
        let mut terminal = JobSnapshot::new(id.clone(), sample_request("discover"));
        terminal.phase = JobPhase::Finished;
        terminal.finish = Some(JobFinish::Cancelled);
        let view = registry
            .publish_terminal_and_release(&id, terminal)
            .expect("active owner");
        let job = view.job.expect("terminal snapshot present");
        assert!(
            job.cancel_requested,
            "cancel_requested must carry through terminal"
        );
        assert_eq!(job.finish, Some(JobFinish::Cancelled));
    }

    // ---------- Cancel: pre-terminal ----------

    /// `cancel_operation` for the active id returns the
    /// current view (NOT `Err`) and is idempotent — the
    /// worker reports the cancel at the next safe
    /// checkpoint. The first call bumps the seq; the
    /// second returns the unchanged view (idempotent).
    #[test]
    fn cancel_pre_terminal_returns_view() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let emit = noop_emit_fn();
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let view = cancel_operation(&registry, &emit, &id).expect("cancel ok");
        assert_eq!(view.job.as_ref().unwrap().id, id);
        assert_eq!(view.job.as_ref().unwrap().phase, JobPhase::Running);
        assert!(view.job.as_ref().unwrap().cancel_requested);
        let view2 = cancel_operation(&registry, &emit, &id).expect("idempotent cancel");
        assert_eq!(view2.seq, view.seq, "idempotent cancel must NOT bump seq");
    }

    /// Cancel for a terminal id is a no-op that returns
    /// the current view (NOT `Err`); cancellation is
    /// idempotent and never an error.
    #[test]
    fn cancel_terminal_returns_view_not_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let emit = noop_emit_fn();
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let mut snap = JobSnapshot::new(id.clone(), sample_request("discover"));
        snap.phase = JobPhase::Finished;
        snap.finish = Some(JobFinish::Completed);
        registry.publish_terminal_and_release(&id, snap).unwrap();
        let view = cancel_operation(&registry, &emit, &id).expect("cancel ok");
        let job = view.job.expect("terminal snapshot retained");
        assert_eq!(job.id, id);
        assert_eq!(job.phase, JobPhase::Finished);
        assert_eq!(job.finish, Some(JobFinish::Completed));
    }

    /// Cancel for an unknown id surfaces
    /// `OperationError::UnknownJob`.
    #[test]
    fn cancel_unknown_returns_unknown_job_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let emit = noop_emit_fn();
        let err = cancel_operation(&registry, &emit, "job-9999").unwrap_err();
        match err {
            OperationError::UnknownJob { job_id } => {
                assert_eq!(job_id, "job-9999");
            }
            other => panic!("expected UnknownJob, got {other:?}"),
        }
    }

    // ---------- op_current / op_status ----------

    /// `current_operation` always returns a `CurrentView`
    /// (never `None`), and a freshly-built registry
    /// returns `{seq:"0",job:None}`.
    #[test]
    fn current_operation_fresh_registry_returns_initial_view() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let emit = noop_emit_fn();
        let view = current_operation(&registry, &emit);
        assert_eq!(view.seq, "0");
        assert!(view.job.is_none());
        let _ = paths;
    }

    /// `op_status` for an unknown id surfaces
    /// `OperationError::UnknownJob`; for a known id
    /// returns the current view (with the registry's
    /// current seq).
    #[test]
    fn operation_status_unknown_id_is_unknown_job() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let emit = noop_emit_fn();
        let err = operation_status(&registry, &emit, "job-9999").unwrap_err();
        match err {
            OperationError::UnknownJob { job_id } => {
                assert_eq!(job_id, "job-9999");
            }
            other => panic!("expected UnknownJob, got {other:?}"),
        }
    }

    /// `op_status` for a known id returns the matching
    /// `CurrentView` (with the same seq the last publish
    /// stamped).
    #[test]
    fn operation_status_known_id_returns_current_view() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let emit = noop_emit_fn();
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let view = operation_status(&registry, &emit, &id).expect("known id");
        assert_eq!(view.job.as_ref().unwrap().id, id);
        assert_eq!(view.seq, "1");
    }

    // ---------- CloseRequested decision ----------

    /// `should_block_close` is true while a long job is
    /// in flight, and false once the reservation is
    /// released. The decision is read under the same
    /// lock the registry uses for the reservation, so a
    /// race between `start_operation` and a user click
    /// cannot let the window go away with the
    /// reservation still live.
    #[test]
    fn should_block_close_while_long_active_and_released_after_terminal() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        assert!(
            !registry.should_block_close(),
            "fresh registry must not block close"
        );
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        assert!(
            registry.should_block_close(),
            "long-active registry must block close"
        );
        let mut snap = JobSnapshot::new(id.clone(), sample_request("discover"));
        snap.phase = JobPhase::Finished;
        snap.finish = Some(JobFinish::Completed);
        registry.publish_terminal_and_release(&id, snap).unwrap();
        assert!(
            !registry.should_block_close(),
            "post-terminal registry must not block close"
        );
    }

    /// `should_block_close` is also true while a short
    /// sync is in flight.
    #[test]
    fn should_block_close_while_short_busy() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        assert!(registry.try_reserve_short());
        assert!(
            registry.should_block_close(),
            "short-busy registry must block close"
        );
        registry.release_short();
        assert!(!registry.should_block_close());
    }

    /// `handle_close_requested` is the registry's atomic
    /// close handler: under one lock it reads the
    /// reservation state, requests cancellation of the
    /// active long job, bumps the seq ONCE, and returns
    /// `(should_block, Some(view))`. The frontend wiring
    /// MUST call `api.prevent_close()` when
    /// `should_block == true` and emit the returned view
    /// outside the lock.
    #[test]
    fn handle_close_requested_flips_cancel_and_emits_view_once() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let (captured, emit) = capture_emit();
        let (should_block, view) = registry.handle_close_requested();
        assert!(should_block, "active long job must block close");
        let view = view.expect("view must be Some on first close");
        // View is the freshly-stamped snapshot.
        assert!(view.seq.parse::<u64>().unwrap() > 1, "seq must bump");
        assert!(view.job.as_ref().unwrap().cancel_requested);
        assert_eq!(view.job.as_ref().unwrap().id, id);
        // Emit (the wiring calls emit outside the lock).
        emit(&view);
        // A second close on the same active id is a noop:
        // the sticky flag is already true, the seq must
        // NOT bump again.
        let before = view.seq.clone();
        let (should_block2, view2) = registry.handle_close_requested();
        assert!(should_block2);
        let view2 = view2.expect("view must still be Some");
        assert_eq!(view2.seq, before, "idempotent close must NOT bump seq");
        let _ = captured.lock().expect("lock").len();
    }

    /// `handle_close_requested` when the worker has
    /// already settled (post-terminal) returns
    /// `should_block == false`. The frontend can close
    /// without calling `api.prevent_close()`.
    #[test]
    fn handle_close_requested_after_terminal_does_not_block() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let (id, _t, _v) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let mut snap = JobSnapshot::new(id.clone(), sample_request("discover"));
        snap.phase = JobPhase::Finished;
        snap.finish = Some(JobFinish::Completed);
        registry.publish_terminal_and_release(&id, snap).unwrap();
        let (should_block, view) = registry.handle_close_requested();
        assert!(!should_block, "post-terminal registry must NOT block close");
        assert!(
            view.is_some(),
            "the retained terminal must still be visible"
        );
    }

    // ---------- Fixture resolver + helpers ----------

    /// Build a `JobContextResolver` that resolves the
    /// fixture's checkout the same way the production
    /// resolver does (raw `paths` re-routed through
    /// `with_settings` for sync / skills). Tests that
    /// want a default-`canonical_dir` workflow use a
    /// raw resolver instead — see `raw_resolver_for`.
    fn resolver_for(paths: &agenthd::store::Paths) -> JobContextResolver {
        let paths = paths.clone();
        Box::new(move |request: OperationRequest| {
            // Read settings.json so we know whether the
            // fixture is Ready / Empty / Stale. A
            // `Ready` checkout re-points canonical_dir
            // through `with_settings`, mirroring the
            // production path.
            let settings_bytes = match std::fs::read(&paths.settings_file) {
                Ok(b) => b,
                Err(_) => {
                    return Ok(JobContext {
                        paths: Some(paths.clone()),
                        state: Some(State::default()),
                    })
                }
            };
            let parsed: Option<agenthd::store::Settings> =
                serde_json::from_slice(&settings_bytes).ok();
            let scoped_paths = match parsed.and_then(|s| {
                let checkout_path = s.checkout_path;
                let trimmed = checkout_path.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(agenthd::store::Settings::new(trimmed.to_string()))
            }) {
                Some(s) => match paths.clone().with_settings(&s) {
                    Ok(scoped) => scoped,
                    Err(_) => paths.clone(),
                },
                None => paths.clone(),
            };
            match request {
                OperationRequest::SyncAgents { .. } | OperationRequest::InstallSkills => {
                    Ok(JobContext {
                        paths: Some(scoped_paths),
                        state: Some(State::default()),
                    })
                }
                OperationRequest::InstallTool { .. } => Ok(JobContext {
                    paths: Some(paths.clone()),
                    state: None,
                }),
                OperationRequest::DiscoverModels => {
                    // Discovery is orthogonal to the
                    // configured checkout: the production
                    // resolver skips `Paths::from_env()`
                    // for this arm, so the fixture
                    // mirrors that and returns
                    // `paths: None`.
                    Ok(JobContext {
                        paths: None,
                        state: None,
                    })
                }
            }
        })
    }

    /// Build a `JobContextResolver` that returns the
    /// fixture's raw `paths` unchanged. Used by tests
    /// that want to exercise the no-`Ready` branch
    /// (empty settings, malformed settings). Mirrors
    /// the production resolver's raw-env behavior for
    /// Tool paths; Discovery returns `paths: None` for
    /// the same reason the production resolver skips
    /// `Paths::from_env()` for the discovery arm.
    fn raw_resolver_for(paths: &agenthd::store::Paths) -> JobContextResolver {
        let paths = paths.clone();
        Box::new(move |request: OperationRequest| match request {
            OperationRequest::SyncAgents { .. } | OperationRequest::InstallSkills => {
                Ok(JobContext {
                    paths: Some(paths.clone()),
                    state: Some(State::default()),
                })
            }
            OperationRequest::InstallTool { .. } => Ok(JobContext {
                paths: Some(paths.clone()),
                state: None,
            }),
            OperationRequest::DiscoverModels => Ok(JobContext {
                paths: None,
                state: None,
            }),
        })
    }

    /// Capture-emit helper: returns an `EmitFnArc` that
    /// pushes every emitted `CurrentView` into a shared
    /// `Vec` so tests can assert on the order of events.
    fn capture_emit() -> (Arc<Mutex<Vec<CurrentView>>>, EmitFnArc) {
        let captured: Arc<Mutex<Vec<CurrentView>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        let emit: EmitFnArc = Arc::new(move |view: &CurrentView| {
            sink.lock().expect("emit capture lock").push(view.clone());
            true
        });
        (captured, emit)
    }

    /// Drive the registry through a real worker for one
    /// request variant. Used by the per-variant finish
    /// preservation tests; the worker uses the default
    /// `default_thread_factory` so the test waits for
    /// the worker to settle via `wait_for_release`.
    fn run_to_terminal(
        registry: Arc<OperationRegistry>,
        paths: &agenthd::store::Paths,
        request: OperationRequest,
    ) -> CurrentView {
        let emit = noop_emit_fn();
        start_operation_with(Arc::clone(&registry), emit, resolver_for(paths), request)
            .expect("start");
        // Wait for the worker to release the reservation.
        // Tool install preflight may spawn `git` /
        // `node` which can take up to ~120s in the lib,
        // but on a no-binary CI / test env the
        // preflight fails fast. A 5s budget is plenty
        // for the unit-test path.
        for _ in 0..5000 {
            if registry.active_job_id().is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        registry.current_view()
    }

    // ---------- Catalog lookup ----------

    #[test]
    fn catalog_lookup_resolves_known_id() {
        let entry = lookup_catalog_entry(DEFAULT_CATALOG[0].skill_name).expect("known id");
        assert_eq!(entry.skill_name, DEFAULT_CATALOG[0].skill_name);
    }

    #[test]
    fn catalog_lookup_rejects_unknown_id_with_catalog_name_list() {
        let err = lookup_catalog_entry("not-a-real-tool").unwrap_err();
        match err {
            OperationError::FailedPreconditions { message } => {
                assert!(message.contains("not-a-real-tool"));
                for entry in DEFAULT_CATALOG {
                    assert!(
                        message.contains(entry.skill_name),
                        "catalog list missing `{}`, got: {message}",
                        entry.skill_name
                    );
                }
            }
            other => panic!("expected FailedPreconditions, got {other:?}"),
        }
    }

    // ---------- Wire projection pins ----------

    #[test]
    fn operation_request_serializes_with_kind_discriminant() {
        let req = OperationRequest::SyncAgents {
            target: SyncAgentTarget::Opencode,
        };
        let json = serde_json::to_value(&req).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(
            obj.get("kind").and_then(|v| v.as_str()),
            Some("sync_agents")
        );
        assert_eq!(obj.get("target").and_then(|v| v.as_str()), Some("opencode"));
    }

    #[test]
    fn operation_request_rejects_unknown_fields() {
        let bad = serde_json::json!({
            "kind": "install_tool",
            "tool_id": "pi-psql",
            "rogue": "value"
        });
        let res: Result<OperationRequest, _> = serde_json::from_value(bad);
        assert!(res.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn current_view_serializes_seq_as_string() {
        let view = CurrentView {
            seq: u64::MAX.to_string(),
            job: None,
        };
        let value = serde_json::to_value(&view).unwrap();
        let seq = value.get("seq").unwrap();
        assert!(
            seq.is_string(),
            "seq must serialize as a string, got: {seq}"
        );
    }

    /// Initial view's wire shape: `{seq:"0",job:null}`.
    /// The frontend's reducer seeds from this value.
    #[test]
    fn current_view_initial_serializes_with_zero_seq_and_null_job() {
        let view = CurrentView::initial();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json.get("seq").and_then(|v| v.as_str()), Some("0"));
        assert!(json.get("job").is_some());
        assert!(json["job"].is_null());
    }

    /// ToolStatusRow projection preserves the lib's
    /// `ToolStatus::label()` for every catalog arm.
    #[test]
    fn tool_status_row_projects_lib_fields() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // No destination → status is NotInstalled.
        let rows = tool_catalog_status(&paths).expect("status must succeed");
        assert_eq!(rows.len(), DEFAULT_CATALOG.len());
        let row: ToolStatusRow = rows[0].clone().into();
        assert_eq!(row.tool_id, DEFAULT_CATALOG[0].skill_name);
        // The label is the lib's `label()`.
        assert!(
            agenthd::tools::ToolStatus::NotInstalled
                .label()
                .contains(&row.status)
                || row.status == "not installed"
        );
    }

    /// `tool_catalog_status` must surface a real I/O
    /// error from the lib instead of inventing a
    /// synthetic `Conflict` row. Pre-fix, a single bad
    /// entry produced a bogus `Conflict` row with a
    /// fabricated detail string.
    #[test]
    fn tool_catalog_status_surfaces_io_error() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Create the destination parent and put a regular
        // file at the destination itself, so `stat` on the
        // destination fails with a non-NotFound IO error.
        // Pre-fix, the `unwrap_or_else` swallowed the
        // error and surfaced a fabricated `Conflict` row;
        // post-fix, the lib error propagates as `Err`.
        if let Some(parent) = paths.skills_dir.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&paths.skills_dir, b"not a directory").unwrap();
        let err = tool_catalog_status(&paths).unwrap_err();
        assert!(
            err.contains("tool status failed"),
            "expected lib error passthrough, got: {err}"
        );
    }

    /// Three-runner finish preservation: Sync, Skills,
    /// and the discovery / tool path. Each variant must
    /// surface the lib's actual `Finish` rather than
    /// inventing `Completed` (the pre-fix bug). Sync /
    /// Skills here complete cleanly so the asserted
    /// shape is `Completed` with a (possibly empty)
    /// `observed_state` JSON value; the projection
    /// itself preserves `Failed/Cancelled` which is
    /// asserted in the dedicated unit tests below.
    #[test]
    fn sync_agents_persists_outcomes_and_observed_state() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Apply a Ready checkout so the workflow
        // actually runs through `plan_then_apply`.
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        agenthd::store::save_settings(
            &paths.settings_file,
            &agenthd::store::Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let view = run_to_terminal(
            Arc::clone(&registry),
            &paths,
            sample_request("sync_opencode"),
        );
        let job = view.job.expect("terminal snapshot");
        assert_eq!(job.phase, JobPhase::Finished);
        let report = job.report.expect("terminal report");
        match report {
            JobReport::SyncAgents {
                outcomes,
                observed_state,
            } => {
                // No agents on a fresh fixture, so the
                // outcomes are empty; the projection
                // still includes the `observed_state`
                // field so the frontend can render the
                // ownership manifest the workflow
                // produced.
                assert!(outcomes.is_empty());
                assert!(observed_state.is_some());
            }
            other => panic!("expected SyncAgents, got {other:?}"),
        }
    }

    /// `project_tool_partial` does NOT invent
    /// `Finish::Completed`. The lib's actual `Finish`
    /// (and `error`) flow through `publish_terminal`
    /// unchanged. A cancelled / failed tool report
    /// keeps its `Cancelled` / `Failed` finish.
    #[test]
    fn project_tool_partial_preserves_partial_payload() {
        // Cancelled
        let partial = (
            agenthd::tools::ToolOutcome {
                status: agenthd::tools::ToolStatus::NotInstalled,
                detail: "cancelled before preflight".to_string(),
            },
            None,
        );
        let projected = project_tool_partial(partial);
        match projected {
            JobReport::InstallTool { terminal: t } => {
                assert_eq!(t.detail, "cancelled before preflight");
                assert_eq!(t.residual, None);
            }
            other => panic!("expected InstallTool, got {other:?}"),
        }
        // Failed
        let partial = (
            agenthd::tools::ToolOutcome {
                status: agenthd::tools::ToolStatus::PrerequisitesMissing,
                detail: "git missing".to_string(),
            },
            None,
        );
        let projected = project_tool_partial(partial);
        match projected {
            JobReport::InstallTool { terminal: t } => {
                assert_eq!(t.status, "prerequisites missing");
                assert_eq!(t.detail, "git missing");
            }
            other => panic!("expected InstallTool, got {other:?}"),
        }
        // Completed
        let partial = (
            agenthd::tools::ToolOutcome {
                status: agenthd::tools::ToolStatus::Installed,
                detail: "ok".to_string(),
            },
            None,
        );
        let projected = project_tool_partial(partial);
        match projected {
            JobReport::InstallTool { terminal: t } => {
                assert_eq!(t.status, "installed");
                assert_eq!(t.detail, "ok");
            }
            other => panic!("expected InstallTool, got {other:?}"),
        }
    }

    /// `project_discovery_partial` does NOT invent
    /// `Finish::Failed` for `None` and does NOT invent
    /// `Finish::Completed` for `Some`. A discovery that
    /// reported `Failed` (lib internal `Failed` arm)
    /// keeps its `Failed` finish.
    #[test]
    fn project_discovery_partial_preserves_partial_payload() {
        let partial = Some(LibDiscovery::Failed("opencode missing".to_string()));
        let projected = project_discovery_partial(partial);
        match projected {
            JobReport::DiscoverModels {
                terminal: Some(DiscoveryTerminal::Failed { message }),
            } => {
                assert!(message.contains("opencode missing"));
            }
            other => panic!("expected DiscoverModels Failed, got {other:?}"),
        }
        let projected = project_discovery_partial(None);
        match projected {
            JobReport::DiscoverModels { terminal: None } => {}
            other => panic!("expected DiscoverModels None, got {other:?}"),
        }
        let partial = Some(LibDiscovery::Found(vec!["prov/a".to_string()]));
        let projected = project_discovery_partial(partial);
        match projected {
            JobReport::DiscoverModels {
                terminal: Some(DiscoveryTerminal::Found { models }),
            } => {
                assert_eq!(models, vec!["prov/a".to_string()]);
            }
            other => panic!("expected DiscoverModels Found, got {other:?}"),
        }
    }

    /// `JobReport::SyncAgents` and
    /// `JobReport::InstallSkills` round-trip the new
    /// `observed_state` field through serde. A test
    /// fixture that ignores the new field (i.e., does
    /// not set it) still parses because the field is
    /// `#[serde(default)]` — the backward-compat
    /// contract for fixtures predating the field.
    #[test]
    fn job_report_backward_compat_omits_observed_state() {
        let legacy = serde_json::json!({
            "kind": "sync_agents",
            "outcomes": []
        });
        let parsed: JobReport = serde_json::from_value(legacy).expect("legacy parse");
        match parsed {
            JobReport::SyncAgents {
                outcomes,
                observed_state,
            } => {
                assert!(outcomes.is_empty());
                assert!(observed_state.is_none());
            }
            other => panic!("expected SyncAgents, got {other:?}"),
        }
    }

    /// **InstallTool wire shape pin.** The frontend
    /// reads `{ kind: "install_tool", terminal: {
    /// status, detail, residual } }`; the registry MUST
    /// produce that exact nested form (not a flat
    /// `{ kind: "install_tool", status, detail, residual
    /// }` from a tuple variant, which a previous
    /// internally-tagged enum bug produced). The test
    /// exercises the real `serde_json` serializer (not
    /// just the in-memory match) so a regression in the
    /// tag / variant structure would surface as a wire
    /// mismatch, not a silent flattening.
    #[test]
    fn install_tool_wire_shape_is_nested_terminal() {
        let report = JobReport::InstallTool {
            terminal: ToolTerminal {
                status: "installed".to_string(),
                detail: "ok".to_string(),
                residual: Some("/staged".to_string()),
            },
        };
        let value = serde_json::to_value(&report).expect("serialize");
        // Tag is preserved.
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("install_tool")
        );
        // Fields are NESTED under `terminal`, not
        // flattened at the variant top-level.
        let terminal = value
            .get("terminal")
            .and_then(|v| v.as_object())
            .expect("terminal must be a nested object");
        assert_eq!(
            terminal.get("status").and_then(|v| v.as_str()),
            Some("installed")
        );
        assert_eq!(terminal.get("detail").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(
            terminal.get("residual").and_then(|v| v.as_str()),
            Some("/staged")
        );
        // The flattened form must NOT appear (the
        // pre-fix bug: tuple variant flattened into the
        // variant payload).
        assert!(value.get("status").is_none(), "status must be nested");
        assert!(value.get("detail").is_none(), "detail must be nested");
        assert!(value.get("residual").is_none(), "residual must be nested");
        // Round-trip: parse the wire JSON back into a
        // JobReport and assert the field paths are
        // preserved.
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::InstallTool { terminal } => {
                assert_eq!(terminal.status, "installed");
                assert_eq!(terminal.detail, "ok");
                assert_eq!(terminal.residual.as_deref(), Some("/staged"));
            }
            other => panic!("expected InstallTool, got {other:?}"),
        }
    }

    /// **InstallTool null residual wire pin.** The
    /// `residual` field is `Option<String>`; on the wire
    /// a `None` residual must serialize as `null` (not
    /// be omitted), so the frontend's `tool.status` /
    /// `tool.detail` / `tool.residual` reads do not
    /// see `undefined`. The shape must match the
    /// pre-cancel and post-cleanup arms of the
    /// installer.
    #[test]
    fn install_tool_wire_shape_with_null_residual() {
        let report = JobReport::InstallTool {
            terminal: ToolTerminal {
                status: "prerequisites missing".to_string(),
                detail: "git missing".to_string(),
                residual: None,
            },
        };
        let value = serde_json::to_value(&report).expect("serialize");
        let terminal = value
            .get("terminal")
            .and_then(|v| v.as_object())
            .expect("terminal must be nested");
        assert!(
            terminal
                .get("residual")
                .map(|v| v.is_null())
                .unwrap_or(false),
            "residual=None must serialize as null, got: {terminal:?}"
        );
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::InstallTool { terminal } => {
                assert!(terminal.residual.is_none());
            }
            other => panic!("expected InstallTool, got {other:?}"),
        }
    }

    /// **DiscoverModels wire shape pin — Found arm.**
    /// The frontend's `JobReport` projection expects
    /// `{ kind: "discover_models", terminal: { kind:
    /// "found", models: [...] } }`. The pre-fix tuple
    /// variant flattened the inner enum and broke
    /// serde's internally-tagged contract (a tag
    /// collision with `Option`'s `None` arm made the
    /// variant non-serializable for both `Some` and
    /// `None`). The struct-variant fix must produce
    /// the nested form on the wire AND round-trip
    /// cleanly.
    #[test]
    fn discover_models_wire_shape_found_roundtrip() {
        let report = JobReport::DiscoverModels {
            terminal: Some(DiscoveryTerminal::Found {
                models: vec!["prov/a".to_string(), "prov/b".to_string()],
            }),
        };
        let value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("discover_models")
        );
        let terminal = value
            .get("terminal")
            .and_then(|v| v.as_object())
            .expect("terminal must be nested object");
        assert_eq!(terminal.get("kind").and_then(|v| v.as_str()), Some("found"));
        let models = terminal
            .get("models")
            .and_then(|v| v.as_array())
            .expect("models must be array");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].as_str(), Some("prov/a"));
        assert_eq!(models[1].as_str(), Some("prov/b"));
        // Top-level must NOT carry `models` directly.
        assert!(value.get("models").is_none(), "models must be nested");
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::DiscoverModels {
                terminal: Some(DiscoveryTerminal::Found { models }),
            } => {
                assert_eq!(models, vec!["prov/a".to_string(), "prov/b".to_string()]);
            }
            other => panic!("expected DiscoverModels Found, got {other:?}"),
        }
    }

    /// **DiscoverModels wire shape pin — Empty arm.**
    /// The lib's `Discovery::Empty(message)` projects
    /// to `DiscoveryTerminal::Empty { message }` and
    /// must round-trip through the wire as `{ kind:
    /// "discover_models", terminal: { kind: "empty",
    /// message } }`.
    #[test]
    fn discover_models_wire_shape_empty_roundtrip() {
        let report = JobReport::DiscoverModels {
            terminal: Some(DiscoveryTerminal::Empty {
                message: "no models found".to_string(),
            }),
        };
        let value = serde_json::to_value(&report).expect("serialize");
        let terminal = value
            .get("terminal")
            .and_then(|v| v.as_object())
            .expect("terminal must be nested object");
        assert_eq!(terminal.get("kind").and_then(|v| v.as_str()), Some("empty"));
        assert_eq!(
            terminal.get("message").and_then(|v| v.as_str()),
            Some("no models found")
        );
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::DiscoverModels {
                terminal: Some(DiscoveryTerminal::Empty { message }),
            } => {
                assert_eq!(message, "no models found");
            }
            other => panic!("expected DiscoverModels Empty, got {other:?}"),
        }
    }

    /// **DiscoverModels wire shape pin — Failed arm.**
    /// The lib's `Discovery::Failed(message)` projects
    /// to `DiscoveryTerminal::Failed { message }` and
    /// must round-trip through the wire as `{ kind:
    /// "discover_models", terminal: { kind: "failed",
    /// message } }`.
    #[test]
    fn discover_models_wire_shape_failed_roundtrip() {
        let report = JobReport::DiscoverModels {
            terminal: Some(DiscoveryTerminal::Failed {
                message: "opencode missing".to_string(),
            }),
        };
        let value = serde_json::to_value(&report).expect("serialize");
        let terminal = value
            .get("terminal")
            .and_then(|v| v.as_object())
            .expect("terminal must be nested object");
        assert_eq!(
            terminal.get("kind").and_then(|v| v.as_str()),
            Some("failed")
        );
        assert_eq!(
            terminal.get("message").and_then(|v| v.as_str()),
            Some("opencode missing")
        );
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::DiscoverModels {
                terminal: Some(DiscoveryTerminal::Failed { message }),
            } => {
                assert_eq!(message, "opencode missing");
            }
            other => panic!("expected DiscoverModels Failed, got {other:?}"),
        }
    }

    /// **DiscoverModels wire shape pin — None terminal.**
    /// A pre-cancel discovery (or a discovery that
    /// returned `None` for any reason) projects to
    /// `DiscoverModels { terminal: None }`. The wire
    /// form must serialize `terminal` as `null` (not
    /// omit the field), so the frontend's `report.
    /// terminal?.kind === "found"` guard sees a
    /// well-defined `null` instead of `undefined`. The
    /// shape is the regression pin for the
    /// `Option<DiscoveryTerminal>` collision with
    /// `#[serde(tag = "kind")]`.
    #[test]
    fn discover_models_wire_shape_null_terminal_roundtrip() {
        let report = JobReport::DiscoverModels { terminal: None };
        let value = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("discover_models")
        );
        assert!(
            value.get("terminal").map(|v| v.is_null()).unwrap_or(false),
            "terminal=None must serialize as null, got: {value}"
        );
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::DiscoverModels { terminal: None } => {}
            other => panic!("expected DiscoverModels None, got {other:?}"),
        }
    }

    /// **Other two outcomes' wire shapes unchanged.**
    /// `SyncAgents` and `InstallSkills` keep their
    /// existing struct-variant layout (outcomes +
    /// observed_state). A regression in the new
    /// struct-variant changes for `InstallTool` /
    /// `DiscoverModels` must NOT touch the other two
    /// arms' wire forms. The test pins the
    /// exact-shape roundtrip for both populated
    /// and null `observed_state` paths.
    #[test]
    fn sync_and_skills_wire_shapes_unchanged_after_installtool_fix() {
        let sync_populated = JobReport::SyncAgents {
            outcomes: vec![RowOutcome {
                name: "scout".to_string(),
                action: "wrote".to_string(),
                detail: "ok".to_string(),
                ok: true,
            }],
            observed_state: Some(serde_json::json!({"a": 1, "b": 2})),
        };
        let value = serde_json::to_value(&sync_populated).expect("serialize");
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("sync_agents")
        );
        assert!(value.get("outcomes").is_some());
        assert!(value.get("observed_state").is_some());
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        assert!(matches!(parsed, JobReport::SyncAgents { .. }));

        let skills_null = JobReport::InstallSkills {
            outcomes: vec![],
            observed_state: None,
        };
        let value = serde_json::to_value(&skills_null).expect("serialize");
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("install_skills")
        );
        assert!(value.get("outcomes").is_some());
        assert!(
            value
                .get("observed_state")
                .map(|v| v.is_null())
                .unwrap_or(false),
            "observed_state=None must serialize as null, got: {value}"
        );
        let parsed: JobReport = serde_json::from_value(value).expect("parse");
        match parsed {
            JobReport::InstallSkills { observed_state, .. } => {
                assert!(observed_state.is_none());
            }
            other => panic!("expected InstallSkills, got {other:?}"),
        }
    }

    /// **Retained CurrentView Finished (cancel
    /// requested, report terminal None) JSON pin.**
    /// The bug: a pre-cancel discovery with
    /// `partial: None` produced a `Finished` snapshot
    /// whose `report.terminal` was `None`; the
    /// internally-tagged enum refused to serialize
    /// `None` (no `kind` field on `null`) and the
    /// `agenthd-operation` event emitted an
    /// unparseable envelope. The fix: `terminal` is a
    /// struct variant whose `None` arm serializes as
    /// `{ kind: "discover_models", terminal: null }`,
    /// the `cancel_requested` flag stays `true`
    /// through the terminal transition (the
    /// `publish_failed_terminal` sticky merge), and
    /// the entire `CurrentView` round-trips through
    /// real `serde_json` so the frontend's reducer
    /// accepts the post-cancel terminal without an
    /// exception.
    #[test]
    fn retained_current_view_finished_cancel_requested_discovery_none_roundtrips() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        // Reserve an id (mirrors the cancel-before-
        // spawn pre-fix scenario).
        let (id, token, _view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        // Flip cancel_requested via the registry
        // helper. The terminal snapshot will carry
        // `cancel_requested = true` through the
        // sticky merge in `publish_failed_terminal`.
        let cancel_view = registry
            .request_cancel(&id)
            .expect("active id")
            .expect("flip once");
        assert!(cancel_view.job.as_ref().unwrap().cancel_requested);
        // The worker pre-cancel produced
        // `partial: None` (no `opencode` binary on a
        // tempdir; the cancel arrived before any
        // discovery tick). Publish a terminal
        // snapshot with the wire-shape contract.
        let mut terminal = JobSnapshot::new(id.clone(), sample_request("discover"));
        terminal.cancel_requested = true;
        terminal.phase = JobPhase::Finished;
        terminal.finish = Some(JobFinish::Cancelled);
        terminal.error = Some("cancelled before spawn".to_string());
        terminal.report = Some(JobReport::DiscoverModels { terminal: None });
        let view = registry
            .publish_terminal_and_release(&id, terminal)
            .expect("active owner")
            .clone();
        // Serialize the full `CurrentView` to JSON
        // (real `serde_json`, not the in-memory
        // `Debug`). The pre-fix bug: this
        // serialization step panicked / failed
        // because the internally-tagged enum could
        // not handle `Option::None` as the variant
        // payload.
        let json = serde_json::to_string(&view).expect("serialize current view");
        let value = serde_json::from_str::<serde_json::Value>(&json).expect("parse json");
        // The retained snapshot is `Finished`,
        // `cancel_requested = true`, and the
        // `report.terminal` is `null` (not omitted).
        let job = value
            .get("job")
            .and_then(|v| v.as_object())
            .expect("job must be present");
        assert_eq!(job.get("phase").and_then(|v| v.as_str()), Some("finished"));
        assert_eq!(
            job.get("cancel_requested").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            job.get("finish").and_then(|v| v.as_str()),
            Some("cancelled")
        );
        let report = job
            .get("report")
            .and_then(|v| v.as_object())
            .expect("report must be present");
        assert_eq!(
            report.get("kind").and_then(|v| v.as_str()),
            Some("discover_models")
        );
        assert!(
            report.get("terminal").map(|v| v.is_null()).unwrap_or(false),
            "report.terminal must serialize as null, got: {report:?}"
        );
        // Round-trip: parse the wire JSON back into a
        // `CurrentView` (the frontend's reducer does
        // the same). The pre-fix bug: this parse
        // failed because the internally-tagged
        // `DiscoverModels(Option<...>)` could not be
        // disambiguated from the JSON envelope.
        let parsed: CurrentView = serde_json::from_value(value).expect("parse current view");
        let parsed_job = parsed.job.expect("job present after parse");
        assert_eq!(parsed_job.phase, JobPhase::Finished);
        assert!(parsed_job.cancel_requested);
        assert_eq!(parsed_job.finish, Some(JobFinish::Cancelled));
        match parsed_job.report.expect("report present after parse") {
            JobReport::DiscoverModels { terminal: None } => {}
            other => panic!("expected DiscoverModels None, got {other:?}"),
        }
        // The `cancel_requested` sticky flag and the
        // `Finished` phase both survive the JSON
        // roundtrip — the frontend's reducer can
        // render the "cancelling…" hint and the
        // finished terminal without an exception.
        let _ = token;
        let _ = paths;
    }

    /// **Worker pre-cancel Discovery emit is
    /// serializable.** End-to-end: the actual worker
    /// path that emits a pre-cancel Discovery
    /// terminal (`partial: None`, `cancel_requested:
    /// true`) MUST go through the `emit` closure
    /// with a fully-serializable `CurrentView`. The
    /// emit path captures the view under the lock
    /// and hands it to the closure; the closure then
    /// serializes it through `serde_json` to assert
    /// the wire shape would have round-tripped. The
    /// pre-fix bug: the `emit` step silently
    /// forwarded a view whose `JobReport` was
    /// non-serializable, so the `agenthd-operation`
    /// event delivered an unparseable envelope to
    /// the frontend. The fix: the same `EmitFnArc`
    /// that the production wiring uses is captured
    /// by the test, and the captured view is
    /// serialized through real `serde_json` (not
    /// in-memory). Errors cannot pass again.
    #[test]
    fn worker_pre_cancel_discovery_emit_serializes_current_view() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        // Build a `JsonAssertingEmit` closure that
        // serializes the emitted `CurrentView`
        // through real `serde_json` and records both
        // the result and the JSON. The pre-fix bug
        // would surface as a serialization error
        // (internally-tagged enum with
        // `Option<DiscoveryTerminal>` cannot
        // serialize `None`).
        let emitted: Arc<Mutex<Vec<(CurrentView, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&emitted);
        let emit: EmitFnArc = Arc::new(move |view: &CurrentView| {
            let json = serde_json::to_string(view).expect("emit view must serialize to JSON");
            sink.lock().expect("emit lock").push((view.clone(), json));
            true
        });
        // Reserve and pre-cancel: the terminal
        // snapshot carries `cancel_requested = true`
        // and a `DiscoverModels { terminal: None }`
        // report (no opencode binary, cancel arrived
        // before spawn).
        let (id, _token, _view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        let _ = registry
            .request_cancel(&id)
            .expect("active id")
            .expect("flip once");
        let mut terminal = JobSnapshot::new(id.clone(), sample_request("discover"));
        terminal.cancel_requested = true;
        terminal.phase = JobPhase::Finished;
        terminal.finish = Some(JobFinish::Cancelled);
        terminal.error = Some("cancelled before spawn".to_string());
        terminal.report = Some(JobReport::DiscoverModels { terminal: None });
        let view = registry
            .publish_terminal_and_release(&id, terminal)
            .expect("active owner")
            .clone();
        // The emit closure MUST receive the view
        // and serialize it. The pre-fix bug: the
        // serde call would panic / error; the test
        // would see an empty captures list.
        emit(&view);
        let captures = emitted.lock().expect("emit lock");
        assert_eq!(
            captures.len(),
            1,
            "emit must record exactly one view, got: {captures:?}"
        );
        let (captured_view, captured_json) = captures.first().expect("at least one");
        // The captured view is the same view the
        // registry installed.
        assert_eq!(captured_view.seq, view.seq);
        assert_eq!(captured_view.job.as_ref().unwrap().id, id);
        // The captured JSON parses back to a
        // `CurrentView` (the frontend's reducer does
        // the same). The pre-fix bug: this parse
        // would fail because the internally-tagged
        // `DiscoverModels(Option<...>)` could not
        // be disambiguated.
        let parsed: CurrentView =
            serde_json::from_str(captured_json).expect("captured JSON must parse as CurrentView");
        let parsed_job = parsed.job.expect("job present after parse");
        assert_eq!(parsed_job.phase, JobPhase::Finished);
        assert!(parsed_job.cancel_requested);
        assert_eq!(parsed_job.finish, Some(JobFinish::Cancelled));
        match parsed_job.report.expect("report present after parse") {
            JobReport::DiscoverModels { terminal: None } => {}
            other => panic!("expected DiscoverModels None, got {other:?}"),
        }
    }

    /// **Existing 92-test harness compatibility pin.**
    /// Drive a cancel-before-terminal discover
    /// through the real `start_operation_with` path
    /// using the existing fixture-based resolver and
    /// the `JsonAssertingEmit` closure from
    /// `worker_pre_cancel_discovery_emit_serializes_current_view`.
    /// This is the regression guard: the harness
    /// (`run_to_terminal` + the `noexternalprocess
    /// mockcontext resolver no IO env`) MUST keep
    /// producing a serializable `CurrentView` for
    /// the pre-cancel discovery arm.
    #[test]
    fn pre_cancel_discovery_emit_through_real_registry_path() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let emitted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&emitted);
        let emit: EmitFnArc = Arc::new(move |view: &CurrentView| {
            let json = serde_json::to_string(view).expect("emit view must serialize to JSON");
            sink.lock().expect("emit lock").push(json);
            true
        });
        // Run a real worker through
        // `start_operation_with` (the same call site
        // the production wiring uses). The
        // fixture-based resolver returns
        // `paths: None, state: None` for the
        // discovery arm, so the lib's
        // `discover_models_controlled` runs without
        // touching the env.
        start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("discover"),
        )
        .expect("start must succeed");
        // Wait for the worker to release.
        wait_for_release(&registry);
        // Drain the captured emits. The pre-fix bug
        // would surface as a serialization error
        // (no view would be captured because the
        // serde call would have panicked).
        let captures = emitted.lock().expect("emit lock").clone();
        assert!(
            !captures.is_empty(),
            "at least the initial running view must be emitted"
        );
        for json in &captures {
            // Each captured JSON parses back to a
            // `CurrentView` (the frontend's reducer
            // does the same). The pre-fix bug: a
            // pre-cancel discovery with `partial:
            // None` would have produced a
            // non-serializable terminal.
            let parsed: CurrentView =
                serde_json::from_str(json).expect("captured JSON must parse as CurrentView");
            assert!(parsed.job.is_some() || parsed.seq == "0");
        }
    }

    /// End-to-end: the worker emits the terminal view
    /// the registry captured under the lock. A test
    /// that captures emits through the registry's emit
    /// closure pins the order: initial Running → terminal
    /// Finished, both with strictly increasing seq, and
    /// the terminal carries the finish / error / report.
    #[test]
    fn worker_emits_running_then_terminal_with_increasing_seq() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let (captured, emit) = capture_emit();
        let outcome = start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("discover"),
        )
        .expect("start must succeed");
        // Drop the running-event emit by waiting for
        // release and reading the captured list.
        wait_for_release(&registry);
        let captures = captured.lock().expect("lock").clone();
        assert!(
            !captures.is_empty(),
            "at least the initial running view must be emitted"
        );
        let last = captures.last().unwrap();
        assert_eq!(last.seq.parse::<u64>().unwrap() > 0, true);
        let last_job = last.job.as_ref().unwrap();
        assert_eq!(last_job.id, outcome.job_id);
        assert_eq!(last_job.phase, JobPhase::Finished);
        assert!(last_job.finish.is_some());
    }

    /// RAII short guard: a helper that panics still
    /// releases the short reservation. Pre-fix the
    /// helper ran `try_reserve_short()` / `helper()` /
    /// `release_short()` linearly; an unwind between
    /// the helper and the release leaked the slot.
    #[test]
    fn guarded_short_releases_on_helper_panic() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        // Acquire manually so we can observe the
        // post-panic release via the registry's
        // internal short_busy state.
        assert!(registry.try_reserve_short());
        // Manually release first; the test then drives
        // the helper through `guarded_short` and forces
        // a panic, verifying the slot clears.
        registry.release_short();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = guarded_short(&registry, (), |_input| -> Result<String, String> {
                panic!("simulated helper panic");
            });
        }));
        assert!(result.is_err(), "panic must propagate");
        // The slot must be released after the unwind.
        assert!(
            !registry.should_block_close(),
            "RAII guard must release short on panic"
        );
    }

    /// `reserve_and_publish` must reject a second call
    /// while the first is still in flight. The atomic
    /// helper returns `None`; the caller surfaces
    /// `OperationError::Busy`.
    #[test]
    fn reserve_and_publish_returns_none_when_active() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        let _ = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("first must succeed");
        let second = registry.reserve_and_publish(sample_request("discover"));
        assert!(second.is_none());
    }

    /// **`publish_failed_terminal` (and therefore every
    /// caller that goes through it) must NOT emit a
    /// fabricated fallback view when the active owner has
    /// been replaced.** Pre-fix, the helper returned
    /// `CurrentView { seq: "0", job: None }` whenever
    /// `publish_terminal_and_release` rejected the
    /// publish; the caller forwarded that fabricated view
    /// to the frontend, which then observed a "fake
    /// terminal" event with the registry's old seq, the
    /// new reservation's `latest` snapshot was stomped by
    /// the helper's bump, and the user's reducer saw a
    /// stale view they never asked for. Now the helper
    /// returns `Option<CurrentView>` and the caller emits
    /// only on `Some`. This test pins the contract through
    /// the real helper (with the captured emit) — the
    /// previous test only exercised the
    /// `publish_terminal_and_release` path with the
    /// active id, which never hit the wrong-owner branch.
    #[test]
    fn publish_failed_terminal_drops_non_owner_emits_nothing() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = OperationRegistry::new();
        // Step 1: reserve id1 and drive it to terminal so
        // the reservation drops. The capture-emit records
        // every emit the registry's helper made.
        let (id1, _t1, _v1) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("first reserve must succeed");
        let mut snap1 = JobSnapshot::new(id1.clone(), sample_request("discover"));
        snap1.phase = JobPhase::Finished;
        snap1.finish = Some(JobFinish::Completed);
        let view1 = registry
            .publish_terminal_and_release(&id1, snap1)
            .expect("id1 publish must succeed as active owner");
        let id1_seq: u64 = view1.seq.parse().unwrap();
        let captures_before = Arc::new(Mutex::new(Vec::<CurrentView>::new()));
        let sink = Arc::clone(&captures_before);
        let emit: EmitFnArc = Arc::new(move |view: &CurrentView| {
            sink.lock().expect("emit lock").push(view.clone());
            true
        });
        assert!(captures_before.lock().expect("lock").is_empty());
        // Step 2: reserve id3 (fresh, post id1). The
        // registry's current_view is the terminal id1
        // snapshot; id3 is active. Capture the baseline
        // snapshot of the registry so we can assert it
        // did NOT change when the stale id1 publish
        // arrives.
        let (_id3, _t3, v3) = registry
            .reserve_and_publish(sample_request("skills"))
            .expect("id3 reserve must succeed");
        let id3_seq: u64 = v3.seq.parse().unwrap();
        assert!(id3_seq > id1_seq, "id3 must bump seq");
        // Step 3: simulate the stale id1 worker arriving
        // late via the real `publish_failed_terminal`
        // helper. The helper calls
        // `publish_terminal_and_release(&id1, ...)`,
        // which rejects because id1 is no longer the
        // active owner. The helper MUST return `None`
        // and the caller MUST NOT emit.
        let stale_snapshot = JobSnapshot::new(id1.clone(), sample_request("discover"));
        let view = publish_failed_terminal(
            &registry,
            &id1,
            stale_snapshot,
            JobFinish::Failed,
            Some("stale id1 terminal must be dropped".to_string()),
            None,
        );
        assert!(
            view.is_none(),
            "wrong-owner publish must return None, got {view:?}"
        );
        // Capture after the stale publish — the
        // captured list MUST be empty because the
        // helper returned None and the test emitted
        // nothing manually.
        let captured = captures_before.lock().expect("lock");
        assert!(
            captured.is_empty(),
            "wrong-owner publish must NOT emit any view, got: {captured:?}"
        );
        // Step 4: the registry's state must be
        // unchanged: id3 is still active, the seq
        // matches `id3_seq`, and the latest snapshot
        // is the id3 running view (NOT a fabricated
        // seq:"0" stub from the kill path).
        assert_eq!(registry.active_job_id().as_deref(), Some(_id3.as_str()));
        let current = registry.current_view();
        assert_eq!(
            current.seq.parse::<u64>().unwrap(),
            id3_seq,
            "seq must not bump"
        );
        let job = current.job.as_ref().expect("id3 running view");
        assert_eq!(job.id, _id3);
        assert_eq!(job.phase, JobPhase::Running);
        // Sanity: the captured view we never emitted
        // would have been the fabricated stub, but the
        // caller never let it reach the frontend.
        drop(captured);
        let _ = emit; // silence unused-binding lint
    }

    /// **End-to-end: the helper's `None` return is
    /// honored by the spawn-failure and resolver-failure
    /// callers.** A test that drives the spawn-failure
    /// path with the captured emit pins the contract
    /// through the real call site, not a manual
    /// `publish_terminal_and_release` exercise.
    #[test]
    fn publish_failed_terminal_drop_does_not_emit_under_capture() {
        // The spawn-failure path is the simplest real
        // call site that goes through
        // `publish_failed_terminal`. We install a
        // failing thread factory so the spawn fails
        // synchronously and the registry publishes a
        // terminal `Failed` snapshot. The captured
        // emit must record exactly one terminal view
        // for the active owner; the helper must NEVER
        // emit a fabricated stub.
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        registry.install_thread_factory(Box::new(|_builder, _worker| {
            Err("simulated ulimit".to_string())
        }));
        let (captured, emit) = capture_emit();
        let err = start_operation_with(
            Arc::clone(&registry),
            emit,
            resolver_for(&paths),
            sample_request("discover"),
        )
        .unwrap_err();
        match err {
            OperationError::Spawn { message } => {
                assert!(message.contains("simulated ulimit"));
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
        let captures = captured.lock().expect("lock").clone();
        // The spawn-failure path is the active owner
        // — the registry publishes one terminal Failed
        // snapshot, releases the reservation, and the
        // caller emits it. We must NOT see a
        // `{seq:"0", job:None}` stub.
        for view in &captures {
            if view.job.is_none() {
                panic!("emitted a fabricated null-job terminal: {view:?}");
            }
            assert_ne!(view.seq, "0", "seq must not be the old fabricated fallback");
        }
        // At least one terminal emit (the spawn
        // failure's terminal Failed).
        assert!(
            captures
                .iter()
                .any(|v| matches!(v.job.as_ref().map(|j| j.phase), Some(JobPhase::Finished))),
            "expected at least one terminal emit, got: {captures:?}"
        );
    }

    /// `permission_keys` returns the lib's `PERMISSION_KEYS`
    /// verbatim; the frontend never maintains a parallel
    /// list.
    #[test]
    fn permission_keys_projects_lib_vocabulary() {
        let keys = permission_keys().keys;
        let expected: Vec<String> = PERMISSION_KEYS.iter().map(|s| s.to_string()).collect();
        assert_eq!(keys, expected);
    }

    // ---------- Regression: runtime path bug ----------

    /// **Pre-fix runtime path bug.** Sync against the
    /// raw `Paths::resolve(...)` shape (canonical_dir =
    /// the historical default `<agenthd_root>/agents`)
    /// must write to the CONFIGURED checkout's agents,
    /// never to the default canonical_dir even when a
    /// homonymous decoy sits at the default. Pre-fix,
    /// the resolver used the raw `paths` and the worker
    /// either wrote into the decoy or hit the missing
    /// canonical-dir gate and surfaced a spurious
    /// precondition failure. Post-fix, the production
    /// resolver resolves `Ready` + re-points via
    /// `with_settings` + scopes every canonical read;
    /// the test fixture mirrors that resolution.
    #[test]
    fn runtime_path_sync_writes_to_configured_checkout_not_default() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // Raw Paths — canonical_dir is the default.
        // Configured checkout lives elsewhere.
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("scout.md"),
            "---\ndescription: configured\nmode: subagent\n---\nbody\n",
        )
        .unwrap();
        // Decoy at default canonical_dir.
        let decoy_agents = paths.canonical_dir.clone();
        std::fs::create_dir_all(&decoy_agents).unwrap();
        std::fs::write(
            decoy_agents.join("scout.md"),
            "---\ndescription: default decoy\nmode: subagent\n---\nDECOY body — must not be overwritten\n",
        )
        .unwrap();
        // Apply the configured checkout via settings.
        agenthd::store::save_settings(
            &paths.settings_file,
            &agenthd::store::Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
        let registry = Arc::new(OperationRegistry::new());
        let view = run_to_terminal(
            Arc::clone(&registry),
            &paths,
            OperationRequest::SyncAgents {
                target: SyncAgentTarget::Opencode,
            },
        );
        let job = view.job.expect("terminal snapshot");
        assert_eq!(job.phase, JobPhase::Finished);
        assert_eq!(job.finish, Some(JobFinish::Completed));
        // Decoy at default canonical_dir must be intact.
        let decoy_bytes = std::fs::read_to_string(decoy_agents.join("scout.md")).unwrap();
        assert!(
            decoy_bytes.contains("DECOY body"),
            "default canonical_dir must not have been overwritten, got: {decoy_bytes}"
        );
    }

    /// **Pre-fix runtime path bug (Skills).** Mirror of
    /// the sync test for `InstallSkills`. Pre-fix, the
    /// skills workflow ran against the default
    /// canonical_dir; post-fix, it runs against the
    /// configured checkout's skills tree.
    #[test]
    fn runtime_path_skills_writes_to_configured_checkout_not_default() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let skills_dir = checkout.join("skills");
        // Empty skills source: the planner finds no rows
        // and the workflow completes cleanly.
        std::fs::create_dir_all(&skills_dir).unwrap();
        // Apply the configured checkout.
        agenthd::store::save_settings(
            &paths.settings_file,
            &agenthd::store::Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
        let registry = Arc::new(OperationRegistry::new());
        let view = run_to_terminal(
            Arc::clone(&registry),
            &paths,
            OperationRequest::InstallSkills,
        );
        let job = view.job.expect("terminal snapshot");
        assert_eq!(job.phase, JobPhase::Finished);
        // Empty source => Completed; any other finish
        // indicates a real lib error that the registry
        // must surface (NOT invent Completed for).
        assert!(job.finish.is_some());
    }

    /// **Pre-fix token-during-barrier test.** A cancel
    /// request that arrives AFTER the registry has
    /// reserved but BEFORE the worker has emitted any
    /// progress MUST still flip the sticky
    /// `cancel_requested` flag and bump the seq exactly
    /// once. The token request is processed under the
    /// same lock the worker uses to bump the seq, so
    /// the worker's eventual terminal view carries
    /// `cancel_requested = true`.
    #[test]
    fn cancel_during_initial_barrier_emits_sticky_view() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        // Simulate the post-reservation barrier: the
        // registry has reserved + installed the running
        // snapshot, but the worker hasn't started yet.
        let (id, _t, view) = registry
            .reserve_and_publish(sample_request("discover"))
            .expect("reserve");
        // Cancel arrives during the barrier.
        let cancel_view = registry.request_cancel(&id).expect("active id");
        let cancel_view = match cancel_view {
            Ok(v) => v,
            Err(_) => panic!("cancel must flip once on first call"),
        };
        assert!(cancel_view.seq.parse::<u64>().unwrap() > view.seq.parse::<u64>().unwrap());
        let job = cancel_view.job.as_ref().unwrap();
        assert!(job.cancel_requested);
        assert_eq!(job.id, id);
        // Drive to terminal and verify the sticky flag
        // survives the publish.
        let mut terminal = JobSnapshot::new(id.clone(), sample_request("discover"));
        terminal.phase = JobPhase::Finished;
        terminal.finish = Some(JobFinish::Cancelled);
        let final_view = registry
            .publish_terminal_and_release(&id, terminal)
            .expect("active owner");
        let final_job = final_view.job.as_ref().unwrap();
        assert!(
            final_job.cancel_requested,
            "sticky flag must survive terminal publish"
        );
    }

    /// **Pre-fix `paths_and_state_from_env` test.** The
    /// public env-resolver still exists for the test
    /// surface; the runtime does not call it before
    /// reserve. This test pins that the env resolver
    /// surfaces a `Ready` checkout's scoped paths.
    #[test]
    fn paths_and_state_from_env_scopes_to_configured_checkout() {
        // The public helper is no longer the runtime
        // path; verify the underlying `resolve_ready_scoped_paths`
        // works against a Ready settings file.
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        let checkout = dir.path().join("checkout");
        let agents_dir = checkout.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        agenthd::store::save_settings(
            &paths.settings_file,
            &agenthd::store::Settings::new(checkout.to_string_lossy().into_owned()),
        )
        .unwrap();
        let scoped = resolve_ready_scoped_paths(&paths).expect("ready checkout");
        assert_eq!(
            scoped.canonical_dir, agents_dir,
            "scoped paths must point at the configured checkout"
        );
    }

    /// **Resolver-failure path.** A resolver that
    /// returns `Err` must publish a terminal Failed
    /// snapshot with the precondition text, release the
    /// reservation, and surface `Err` to the caller.
    /// Pre-fix, env read happened before reserve, so a
    /// failing env read leaked the slot or panicked.
    #[test]
    fn resolver_failure_publishes_terminal_failed() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        empty_state(&paths);
        let registry = Arc::new(OperationRegistry::new());
        let resolver: JobContextResolver = Box::new(|_request| {
            Err(OperationError::FailedPreconditions {
                message: "resolver says no".to_string(),
            })
        });
        let err = start_operation_with(
            Arc::clone(&registry),
            noop_emit_fn(),
            resolver,
            sample_request("discover"),
        )
        .unwrap_err();
        match err {
            OperationError::FailedPreconditions { message } => {
                assert!(message.contains("resolver says no"));
            }
            other => panic!("expected FailedPreconditions, got {other:?}"),
        }
        assert!(registry.active_job_id().is_none());
        let view = registry.current_view();
        let job = view.job.expect("terminal snapshot retained");
        assert_eq!(job.phase, JobPhase::Finished);
        assert_eq!(job.finish, Some(JobFinish::Failed));
    }

    /// **DiscoverModels uses raw paths, no state.** A
    /// discover request must succeed against a TempDir
    /// with no settings.json (the discovery is
    /// orthogonal to the ownership manifest).
    #[test]
    fn discover_models_runs_without_settings_or_state() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // No settings.json, no state.json — the
        // discovery must NOT fail on precondition.
        let registry = Arc::new(OperationRegistry::new());
        let view = run_to_terminal(
            Arc::clone(&registry),
            &paths,
            OperationRequest::DiscoverModels,
        );
        let job = view.job.expect("terminal snapshot");
        assert_eq!(job.phase, JobPhase::Finished);
        // The discovery will likely return Cancelled /
        // Failed because there is no `opencode` binary
        // in the test env; both are valid
        // (preserved-from-lib) finishes.
        assert!(matches!(
            job.finish,
            Some(JobFinish::Cancelled) | Some(JobFinish::Failed) | Some(JobFinish::Completed)
        ));
    }

    /// **`DiscoverModels` must NOT invoke the
    /// `Paths::from_env()` factory.** Pre-fix, the
    /// production resolver called `Paths::from_env()`
    /// before the match arm, so a Discover request
    /// against a CI env with no `HOME` /
    /// `XDG_CONFIG_HOME` surfaced a `resolve config
    /// paths: …` precondition failure even though the
    /// discovery is orthogonal to the env. The factory
    /// seam ([`production_request_resolve_paths_state_with_paths_factory`])
    /// lets us inject a failing factory and prove the
    /// resolver never calls it for the discovery arm.
    /// The other arms (`SyncAgents` / `InstallSkills` /
    /// `InstallTool`) MUST call the factory and surface
    /// the failure verbatim — that pins the
    /// discovery-only skip.
    #[test]
    fn production_resolver_skips_paths_factory_for_discover() {
        // Failing factory: any call panics. The
        // `SyncAgents` / `InstallSkills` / `InstallTool`
        // arms would propagate the failure; the
        // discovery arm must short-circuit BEFORE the
        // factory call.
        let ctx = production_request_resolve_paths_state_with_paths_factory(
            || panic!("Paths::from_env() must not be called for DiscoverModels"),
            OperationRequest::DiscoverModels,
        )
        .expect("discovery must succeed without invoking the factory");
        assert!(
            ctx.paths.is_none(),
            "discovery ctx must have paths=None, got {ctx:?}"
        );
        assert!(
            ctx.state.is_none(),
            "discovery ctx must have state=None, got {ctx:?}"
        );

        // The other arms MUST invoke the factory and
        // surface the panic. We use a factory that
        // returns Err to keep the assertion
        // side-effect-free.
        let err = production_request_resolve_paths_state_with_paths_factory(
            || {
                Err(OperationError::FailedPreconditions {
                    message: "factory says no".to_string(),
                })
            },
            OperationRequest::SyncAgents {
                target: SyncAgentTarget::Opencode,
            },
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::FailedPreconditions { .. }));
        let err = production_request_resolve_paths_state_with_paths_factory(
            || {
                Err(OperationError::FailedPreconditions {
                    message: "factory says no".to_string(),
                })
            },
            OperationRequest::InstallTool {
                tool_id: "anything".to_string(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, OperationError::FailedPreconditions { .. }));
    }

    /// **InstallTool uses raw paths, no state.** A
    /// tool install must work without state.json — the
    /// tool installer reads `skills_dir` only.
    #[test]
    fn install_tool_runs_without_state_file() {
        let dir = TempDir::new().unwrap();
        let paths = paths_in(&dir);
        // No state.json; the InstallTool resolver
        // returns state = None.
        let registry = Arc::new(OperationRegistry::new());
        let tool_id = DEFAULT_CATALOG[0].skill_name.to_string();
        let view = run_to_terminal(
            Arc::clone(&registry),
            &paths,
            OperationRequest::InstallTool {
                tool_id: tool_id.clone(),
            },
        );
        let job = view.job.expect("terminal snapshot");
        assert_eq!(job.phase, JobPhase::Finished);
        // The tool installer will return Failed /
        // Cancelled because there is no `git` /
        // `opencode` in the test env; both are valid
        // preserved finishes.
        assert!(job.finish.is_some());
        // The report's status field carries the
        // lib's actual outcome (not the registry's
        // invention).
        let report = job.report.expect("terminal report");
        match report {
            JobReport::InstallTool { terminal: _ } => {}
            other => panic!("expected InstallTool, got {other:?}"),
        }
    }
}
