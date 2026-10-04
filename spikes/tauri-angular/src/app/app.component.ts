import {
  afterRenderEffect,
  Component,
  ElementRef,
  OnDestroy,
  OnInit,
  signal,
  viewChild,
} from "@angular/core";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { AgentEditContext, AgentEditDto, deepCopyDto, sameDto } from "./editor-dto";

/** Sentinel option keys for the model searchable select.
 *  Real wire model IDs are never `__inherit__` or
 *  `__custom__`; the lib rejects empty / non-`provider/model`
 *  strings only at save time, so these two-letter prefix
 *  sentinels are guaranteed to never collide with a wire
 *  model id. The two sentinels are intentionally exposed as
 *  constants so the template and the helper that derives
 *  the option list both refer to the same strings. */
const MODEL_KEY_INHERIT = "__inherit__";
const MODEL_KEY_CUSTOM = "__custom__";

/** Result of picking an option from the model select. */
type ModelPick =
  | { kind: "inherit" }
  | { kind: "custom" }
  | { kind: "known"; value: string };

/**
 * Wire shapes returned by the Rust commands in
 * spikes/tauri-angular/src-tauri/src/lib.rs. Mirrors the
 * `#[serde(tag = "kind", rename_all = "snake_case")]`
 * discriminator on `SettingsStatus`.
 */
type SettingsStatus =
  | { kind: "empty" }
  | { kind: "ready"; checkout_path: string }
  | { kind: "stale"; banner: string; raw_path: string }
  | { kind: "error"; message: string };

interface SettingsResponse {
  settings_file: string;
  status: SettingsStatus;
}

interface AgentSummary {
  name: string;
  mode: string;
  model: string | null;
  description: string;
}

interface AgentsList {
  agents: AgentSummary[];
  error: string | null;
}

/** Editable view of an agent. Re-exported from `./editor-dto` so
 *  the wire shape lives in one place and `node --test` can
 *  exercise it without booting Angular. Permissions are a
 *  `string → string` map so the wire shape can carry unknown
 *  keys through to the backend (the lib rejects them at save
 *  time with the standard "unknown permission key" error). */
interface AgentEditLoadResponse {
  agent: AgentEditDto;
  context: AgentEditContext;
}

// ===========================================================================
// D3 long-job wire shapes (mirror `jobs.rs`).
// ===========================================================================

/** Outcome of `op_start`. The backend returns the
 *  job id AND the initial `CurrentView` the registry
 *  installed atomically with the reservation. The
 *  frontend feeds the `view` straight into the reducer
 *  (same envelope as `agenthd-operation` events) so the
 *  wire is consistent: every wire observation is a
 *  `CurrentView`, regardless of whether it arrived as an
 *  event or as a command reply. The job id is preserved
 *  as a wire-stable label for the cancel button. */
interface LongOutcome {
  job_id: string;
  view: CurrentView;
}

/** Sync target enum for the `SyncAgents` request. The
 *  backend serializes with `snake_case`; the frontend
 *  mirrors that so the JSON shape round-trips verbatim. */
type SyncAgentTarget = "opencode" | "pi";

/** Wire shape of `jobs::OperationRequest`. The
 *  `kind` discriminant + `deny_unknown_fields` keep a
 *  typo from the GUI a parse failure, not a silent
 *  default arm. */
type OperationRequest =
  | { kind: "sync_agents"; target: SyncAgentTarget }
  | { kind: "install_skills" }
  | { kind: "install_tool"; tool_id: string }
  | { kind: "discover_models" };

interface OperationPlan {
  kind: string;
  target?: SyncAgentTarget;
  checkout_path: string;
  rows: {
    name: string;
    status: string;
    source_path: string;
    target_path: string;
    source_hash: string | null;
    target_hash: string | null;
    owned_hash: string | null;
    reason: string | null;
  }[];
}

interface PlanView {
  plan: OperationPlan | null;
  error: string | null;
  loading: boolean;
}

const PLAN_REQUESTS = [
  { id: "opencode", label: "Agents → OpenCode", request: { kind: "sync_agents", target: "opencode" } },
  { id: "pi", label: "Agents → Pi", request: { kind: "sync_agents", target: "pi" } },
  { id: "skills", label: "Skills → OpenCode", request: { kind: "install_skills" } },
] as const;

// ===========================================================================
// Sidebar navigation.
//
// One nav entry per logical view. Sync and Plans are grouped
// under a single entry per the directive ("Group Sync and
// plan views logically"). `setView` is the only mutator and
// does not touch the editor draft, the agent list or any IPC
// state — navigation is local UI state only.
// ===========================================================================

export type ViewId =
  | "agents"
  | "sync"
  | "tools"
  | "models"
  | "settings"
  | "job";

interface ViewSpec {
  id: ViewId;
  label: string;
  description: string;
  /** Inline SVG path body — rendered with
   *  `<svg><path d="..."/></svg>`. */
  iconPath: string;
}

const VIEWS: readonly ViewSpec[] = [
  {
    id: "agents",
    label: "Agents",
    description: "Canonical agents, editor and list.",
    iconPath: "M12 12a4 4 0 1 0-4-4 4 4 0 0 0 4 4Zm0 2c-2.7 0-8 1.3-8 4v2h16v-2c0-2.7-5.3-4-8-4Z",
  },
  {
    id: "sync",
    label: "Sync & Plans",
    description: "Agent sync, skills install and inventory.",
    iconPath: "M4 12a8 8 0 0 1 14-5.3L20 9M20 4v5h-5M20 12a8 8 0 0 1-14 5.3L4 15M4 20v-5h5",
  },
  {
    id: "tools",
    label: "Tools",
    description: "Install and inventory of bundled tools.",
    iconPath: "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.8-3.8a6 6 0 0 1-7.9 7.9l-6.7 6.7a2.1 2.1 0 0 1-2.9-2.9l6.7-6.7a6 6 0 0 1 7.9-7.9l-3.8 3.8Z",
  },
  {
    id: "models",
    label: "Models",
    description: "Advisory model discovery for the editor.",
    iconPath: "M3 6h18M3 12h18M3 18h18",
  },
  {
    id: "settings",
    label: "Settings",
    description: "Checkout path and lib status.",
    iconPath: "M10.3 3.6 11 1.8l1 1.8 2.1.3-1.5 1.5.4 2.1-1.9-1-1.9 1 .4-2.1L8 3.9l2.1-.3ZM4 13l1 1.8-1.4 1.4 1.5.5.5 1.5 1.4-1.4L9 18l-1.8 1 1.5.5.5 1.5L11 19.6 13 21l-.3-2.1 1.5-1.5-2.1-.4 1-1.9-1.9 1-.5-1.5Z",
  },
  {
    id: "job",
    label: "Actividad",
    description: "Operación en curso, cancelar y estado.",
    iconPath: "M12 6v6l4 2M21 12a9 9 0 1 1-9-9 9 9 0 0 1 9 9Z",
  },
] as const;

/** One tick of progress. The `processed` / `total` are
 *  `u64` on the wire; the frontend compares them
 *  numerically (Number is safe below 2^53, and the
 *  lib's counters are bounded by the row count). */
interface JobProgress {
  stage: string;
  item: string | null;
  processed: number;
  total: number | null;
}

/** Wire projection of a `JobReport::SyncAgents` /
 *  `JobReport::InstallSkills` row. The lib's `ok` flag
 *  is preserved as-is so a `skipped` row stays
 *  `skipped` (no `ok = true` flattening). */
interface RowOutcome {
  name: string;
  action: string;
  detail: string;
  ok: boolean;
}

/** Wire projection of `JobReport::InstallTool`. The
 *  `residual` is the staging directory the cleaner
 *  could not remove; it is **not** a successful install
 *  path. */
interface ToolTerminal {
  status: string;
  detail: string;
  residual: string | null;
}

/** Wire projection of `JobReport::DiscoverModels`. The
 *  lib's `Discovery` enum is mapped to a tagged wire
 *  form so the GUI can render the Found / Empty / Failed
 *  arms without a parallel type ladder. */
type DiscoveryTerminal =
  | { kind: "found"; models: string[] }
  | { kind: "empty"; message: string }
  | { kind: "failed"; message: string };

/** Wire projection of the lib's `OperationReport`. The
 *  `serde(tag = "kind")` keeps the wire payload small
 *  and the variant arms distinct. The
 *  `observed_state` field is an opaque `unknown`
 *  JSON value the lib populated at terminal for
 *  `SyncAgents` / `InstallSkills`; the GUI renders
 *  only a brief partial-count label and never
 *  persists or destructures it. */
type JobReport =
  | { kind: "sync_agents"; outcomes: RowOutcome[]; observed_state?: unknown }
  | { kind: "install_skills"; outcomes: RowOutcome[]; observed_state?: unknown }
  | { kind: "install_tool"; terminal: ToolTerminal }
  | { kind: "discover_models"; terminal: DiscoveryTerminal | null };

/** `jobs::JobPhase` — running / finished. */
type JobPhase = "running" | "finished";

/** `jobs::JobFinish` — completed / cancelled / failed. */
type JobFinish = "completed" | "cancelled" | "failed";

/** One `JobSnapshot` the registry exposes. The
 *  `cancel_requested` flag is sticky: once `true` it
 *  stays `true` through the terminal transition so the
 *  UI can render the "cancelling…" hint while the
 *  worker still has a row in flight. */
interface JobSnapshot {
  id: string;
  request: OperationRequest;
  phase: JobPhase;
  cancel_requested: boolean;
  progress: JobProgress | null;
  report: JobReport | null;
  error: string | null;
  finish: JobFinish | null;
}

/** `jobs::CurrentView` — the full view the frontend
 *  receives on every state change. `seq` is the global
 *  monotonic sequence the backend stamps on every
 *  push; the frontend compares with `BigInt` and
 *  discards stale snapshots. */
interface CurrentView {
  seq: string;
  job: JobSnapshot | null;
}

/** `jobs::OperationError` — refused request. The
 *  frontend dispatches on `kind` to render each arm
 *  with a different hint. */
type OperationError =
  | { kind: "busy"; active_job_id: string }
  | { kind: "unknown_job"; job_id: string }
  | { kind: "failed_preconditions"; message: string }
  | { kind: "spawn"; message: string };

/** Wire projection of one entry in the backend's
 *  `tool_catalog_status` reply. The backend computes
 *  these rows from `agenthd::tools::DEFAULT_CATALOG` +
 *  `agenthd::tools::tool_status(&paths, entry)`. The
 *  frontend does NOT maintain a parallel catalog — the
 *  picker renders whatever the backend sent at the last
 *  `tool_catalog_status` call. */
interface ToolStatusRow {
  tool_id: string;
  display: string;
  status: string;
  detail: string;
  destination: string;
}

interface ToolStatusList {
  rows: ToolStatusRow[];
}

/** Wire projection of the lib's `PERMISSION_KEYS`. The
 *  frontend renders the editor's known-key dropdown from
 *  this list so a future change to the lib's vocabulary
 *  does not require a parallel frontend update. The
 *  dropdown order follows the lib's own order. */
interface PermissionKeysWire {
  keys: string[];
}

@Component({
  selector: "app-root",
  templateUrl: "./app.component.html",
  styleUrl: "./app.component.css",
})
export class AppComponent implements OnInit, OnDestroy {
  readonly planRequests = PLAN_REQUESTS;
  readonly views = VIEWS;
  /** Sidebar navigation. The default is `agents` per the
   *  directive. `setView` is the only mutator; navigation
   *  does NOT trigger IPC, does NOT prompt for a dirty draft,
   *  does NOT auto-discard an open editor — the editor draft
   *  is preserved across nav because the section is just
   *  `[hidden]`, not removed from the DOM. */
  readonly activeView = signal<ViewId>("agents");
  readonly selectedPlanTarget = signal<string>("opencode");
  readonly plans = signal<Record<string, PlanView>>(Object.fromEntries(
    PLAN_REQUESTS.map(({ id }) => [id, { plan: null, error: null, loading: false }]),
  ));

  /** Switch the active view. Pure UI state change — no
   *  side effects on the editor, agents list, IPC, job
   *  registry or any other persisted state. The hidden
   *  sections stay mounted (so a draft, open editor or
   *  in-flight job panel survives navigation). */
  setView(id: ViewId): void {
    this.activeView.set(id);
  }

  /** Active view title for the workspace header. */
  activeViewTitle(): string {
    return VIEWS.find((v) => v.id === this.activeView())?.label ?? "";
  }

  /** Active view description (one-line subtitle). */
  activeViewDescription(): string {
    return VIEWS.find((v) => v.id === this.activeView())?.description ?? "";
  }

  /** Per-view count shown in the header + sidebar badge. */
  activeViewCount(): number | null {
    switch (this.activeView()) {
      case "agents":
        return this.agents().length > 0 ? this.agents().length : null;
      case "sync": {
        const n = this.agents().length;
        return n > 0 ? n : null;
      }
      case "tools":
        return this.toolCatalog().length > 0 ? this.toolCatalog().length : null;
      case "models":
        return this.discoveredModels().length > 0
          ? this.discoveredModels().length
          : null;
      case "settings":
        return this.settings() ? 1 : null;
      case "job":
        return this.activeJob() ? 1 : null;
    }
  }

  /** Per-view badge for the workspace header — short label
   *  with semantic colouring. */
  activeViewBadge(): { label: string; tone: "accent" | "warning" | "danger" } | null {
    if (this.activeView() === "job") {
      const job = this.currentJob();
      if (!job) return null;
      if (job.phase === "running") return { label: "running", tone: "accent" };
      if (job.finish === "failed") return { label: "failed", tone: "danger" };
      if (job.finish === "cancelled") return { label: "cancelled", tone: "warning" };
      return { label: job.finish ?? "done", tone: "accent" };
    }
    if (this.activeView() === "settings") {
      const s = this.settings();
      if (!s) return null;
      switch (s.status.kind) {
        case "ready":
          return null;
        case "stale":
          return { label: "stale", tone: "warning" };
        case "error":
          return { label: "error", tone: "danger" };
        case "empty":
          return { label: "empty", tone: "warning" };
      }
    }
    if (this.activeView() === "agents") {
      if (this.agentsError()) return { label: "error", tone: "danger" };
      if (this.agentsLoading()) return { label: "loading", tone: "accent" };
    }
    return null;
  }

  /** Sidebar badge for a given nav entry — short count.
   *  Returns `null` if no count is meaningful. */
  viewBadgeCount(id: ViewId): number | null {
    switch (id) {
      case "agents":
        return this.agents().length > 0 ? this.agents().length : null;
      case "sync":
        return this.agents().length > 0 ? this.agents().length : null;
      case "tools":
        return this.toolCatalog().length > 0 ? this.toolCatalog().length : null;
      case "models":
        return this.discoveredModels().length > 0
          ? this.discoveredModels().length
          : null;
      case "settings":
        return this.settings() ? 1 : null;
      case "job":
        return this.activeJob() ? 1 : null;
    }
  }

  /** True when a stale / failed nav entry should be
   *  surfaced as a warning badge in the sidebar. Hidden
   *  settings / list errors show this global warning so
   *  failures are not lost across navigation. */
  viewBadgeWarning(id: ViewId): boolean {
    if (id === "settings") {
      const s = this.settings();
      return s?.status.kind === "stale" || s?.status.kind === "error";
    }
    if (id === "agents") {
      return Boolean(this.agentsError());
    }
    if (id === "job") {
      const job = this.currentJob();
      return job?.finish === "failed";
    }
    if (id === "tools") {
      return Boolean(this.toolCatalogError());
    }
    if (id === "models") {
      return Boolean(this.discoveryError());
    }
    return false;
  }

  async refreshPlans(): Promise<void> {
    if (!this.mutationsEnabled() || this.busy()) return;
    this.busy.set(true);
    try {
      await this.loadPlans();
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  private async loadPlans(): Promise<void> {
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    await Promise.all(PLAN_REQUESTS.map(async ({ id, request }) => {
      this.plans.update((views) => ({ ...views, [id]: { ...views[id], loading: true } }));
      try {
        const plan = await invoke<OperationPlan>("operation_plan", { request });
        if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
        this.plans.update((views) => ({ ...views, [id]: { plan, error: null, loading: false } }));
      } catch (e) {
        if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
        this.plans.update((views) => ({ ...views, [id]: { ...views[id], error: this.toMessage(e), loading: false } }));
      }
    }));
  }
  // Settings state.
  readonly settings = signal<SettingsResponse | null>(null);
  readonly settingsError = signal<string | null>(null);
  readonly settingsLoading = signal(false);

  // Agents state.
  readonly agents = signal<AgentSummary[]>([]);
  readonly agentsError = signal<string | null>(null);
  readonly agentsLoading = signal(false);

  // Settings mutation. `checkoutPath` seeds once from the first
  // Ready/Stale and refreshes never overwrite it while the user
  // has it open. `saving` is a UX signal; `saveError` carries
  // the backend's literal error text (same as the TUI).
  readonly checkoutPath = signal<string>("");
  readonly saving = signal(false);
  readonly saveError = signal<string | null>(null);

  // Editor state. The editor is **inline-per-agent** rather
  // than a single dedicated page: each `AgentSummary` row has
  // its own "Edit" button. While the editor is open it owns
  // the row; the rest of the list stays read-only. The
  // `editing` / `editDraft` / `editContext` / `editOriginal`
  // signals are **independent** of the agents list membership
  // so a refresh that removes the row from `agents()` does
  // not hide the editor — the P1 fix hoists the editor
  // rendering outside the agents/error/list membership gate.
  readonly editing = signal<string | null>(null);
  readonly editDraft = signal<AgentEditDto | null>(null);
  readonly editContext = signal<AgentEditContext | null>(null);
  readonly newAgentName = signal("new-agent");
  readonly editActionConfirm = signal<{ key: string; message: string; draft: AgentEditDto } | null>(null);
  readonly editLoading = signal(false);
  readonly editSaving = signal(false);
  readonly editError = signal<string | null>(null);
  /** Snapshot of the original draft (taken when the editor
   *  opens) so `isEditDirty` can compare field-by-field.
   *  Strings/nulls only — no closures, so it survives
   *  signal re-evaluation cleanly. */
  readonly editOriginal = signal<AgentEditDto | null>(null);

  /** Editor <dialog> reference. The template uses a single
   *  `<dialog #editorDialog>` that lives OUTSIDE the per-view
   *  blocks and OUTSIDE `.workspace-content` (see the static-
   *  template test in `app-component.test.mjs`). The component
   *  imperatively calls `showModal()` / `close()` via the
   *  ElementRef inside an `afterRenderEffect` that re-reads
   *  `editing()` / `editDraft()` and the dialog signal — so
   *  the dialog opens the moment a draft is loaded and closes
   *  the moment the editor is closed. The effect runs after
   *  Angular has flushed the rendered DOM, so the dialog
   *  element is guaranteed to exist when the callback fires.
   *  The body is idempotent and tracks the DOM's true
   *  `open` state (`editorDialogNativeOpen`) so `showModal()`
   *  is never called twice and `close()` is never called on
   *  an already-closed dialog — `showModal` throws when the
   *  dialog is already open in some browsers. */
  private editorDialog = viewChild<ElementRef<HTMLDialogElement>>("editorDialog");

  /** Browser focus on `close()` is not guaranteed to
   *  return to the opener (WebKit / Chromium behaviour
   *  varies when the opener element is hidden by a
   *  re-render). The component captures the trigger
   *  button synchronously on the click that opened the
   *  editor and re-focuses it on close so the keyboard
   *  flow stays inside the agents list. The field is
   *  cleared on close so a stale opener does not get
   *  re-focused after a navigation. No `document.activeElement`
   *  fallback: tests run without a DOM and the
   *  capture-currentTarget path is the deterministic
   *  contract. */
  private editorOpener: HTMLElement | null = null;

  /** Refreshed from native `open` on every sync; never use a stale cached
   *  value to decide whether showModal/close is needed. */
  private editorDialogNativeOpen = false;
  /** Blur only after showModal succeeds, never merely because a draft exists. */
  readonly editorModalOpen = signal(false);

  /** Reactive sync between the `editing()` /
   *  `editDraft()` signals and the editor <dialog>'s
   *  `showModal()` / `close()` state. The effect runs
   *  in Angular's `afterRenderEffect` (browser-only),
   *  re-runs whenever ANY of the three signals it
   *  reads (`editing()`, `editDraft()`, the dialog
   * viewChild) change, AND only after Angular has
   * flushed the rendered DOM — so the dialog element
   * is guaranteed to exist when the callback fires.
   * The previous design polled via
   * `ngAfterViewChecked`, which is a manual approach
   * that does NOT respond to Angular 22's signal-driven
   * render scheduling (a change to `editing()`/`editDraft()`
   * would only surface on the next tick of the
   * `Checked` hook, not on the signal-driven render
   * the template consumes). The signal query is the
   * strict Angular 22 pattern. The body is idempotent
   * and tracks the DOM's true open state — a stale
   * `editorDialogNativeOpen` cannot trigger duplicate
   * `showModal()` calls. Exceptions from `showModal`
   * / `close()` are logged via `editError` so the user
   * can see why the dialog did not open, and the
   * `editorDialogNativeOpen` flag is NOT flipped on
   * failure so the next sync re-tries. */
  private readonly _editorDialogSyncEffect = afterRenderEffect(() => {
    this.syncEditorDialog();
  });

  /** Editor model-search term. PURE UI state — typing
   *  here MUST NOT mutate the draft. The select's
   *  visible option list is filtered by this term; the
   *  draft `model` is only written when the user picks
   *  an option from the filtered list (or types into
   *  the Custom input). The contract is asserted by
   *  the `modelSearch-does-not-mutate-draft` test. */
  readonly modelSearchTerm = signal("");

  /** Local copy of the "Custom…" model input. Distinct
   *  from `modelSearchTerm` so a user typing a manual
   *  model does not have it appear in the search
   *  filter. The field is only revealed when the
   *  user picks the "Custom…" option from the
   *  select; otherwise the field is empty and the
   *  select reflects the draft model. The Custom
   *  input mutates the draft via `onEditModel` on
   *  every input — same wire-shape contract as the
   *  old single `<input>`. */
  readonly modelCustomInput = signal("");

  // Serialization flag for every public action (refresh + save).
  // Public so the template can bind `[disabled]` directly. The
  // flag prevents overlap of public actions while one is in
  // flight; it is not a global consistency guarantee against
  // external writers. `loadSettings` / `loadAgents` are private
  // and bypass it.
  readonly busy = signal(false);

  /** True once the draft buffer has been seeded or the user typed. */
  private draftSeeded = false;

  // ===========================================================================
  // D3 long-job state
  //
  // The `currentView` signal is the single source of truth
  // for the right-hand progress / report panel. The reducer
  // (`applyCurrentView`) is the only writer; it ignores
  // snapshots whose `seq` is older than the last observed
  // `seq` so a stale event cannot overwrite a newer
  // terminal.
  //
  // The `job` signal is a typed view of `currentView().job`
  // so the template can read it without dereferencing
  // `currentView()?.job` on every render.
  // ===========================================================================

  /** Latest `CurrentView` the registry has pushed. The
   *  `BigInt` `lastSeq` is the wire-level uniqueness
   *  guarantee; the reducer compares incoming `seq` against
   *  this value and discards stale snapshots. */
  readonly currentView = signal<CurrentView | null>(null);

  /** True once the `agenthd-operation` listener has been
   *  registered. While `false` the `applyCurrentView`
   *  reducer treats any incoming event as a no-op (the
   *  listener is not the wire source of truth — `current`
   *  is). After a failed `subscribe`, mutations are
   *  disabled (the `gate` flag below) until the user
   *  hits Retry. */
  readonly subscribed = signal(false);

  /** True while the user can mutate state. The flag is
   *  `false` until BOTH the listener is registered AND
   *  the first `op_current` has seeded the reducer; it
   *  flips to `true` only when `subscribe` resolves
   *  successfully (the listener is the wire source of
   *  truth; without it the reducer cannot keep visible
   *  state in sync). It flips back to `false` when
   *  `subscribe` or the initial `current` call fails,
   *  and the only way to re-enable it is the
   *  `retryConnection` action. A window without a
   *  working listener would silently desync from the
   *  registry; the only safe posture is fail-closed
   *  until the wire is up. */
  readonly mutationsEnabled = signal(false);

  /** Connection error for the listener. Surfaced as
   *  `connectionError` in the right-hand panel; the user
   *  can hit Retry to re-register. */
  readonly connectionError = signal<string | null>(null);

  /** Last observed `seq` as a BigInt. The reducer compares
   *  incoming snapshots against this value and discards
   *  stale ones. `null` until the very first event lands
   *  (the very first `current` call also goes through the
   *  reducer, so the field is `null` only on a freshly-
   *  booted backend that has not yet pushed a snapshot). */
  private lastSeq: bigint | null = null;

  /** Generation counter used by `OnDestroy` to identify
   *  late listener resolutions. When the component
   *  destroys, the counter is bumped and any pending
   *  `listen()` promise that resolves after destruction
   *  sees a stale generation and unsubscribes immediately.
   *  Prevents the "listener stays alive after
   *  destruction" leak. */
  private generation = 0;

  /** Per-connection generation counter, incremented on
   *  every fresh `subscribe()` call (initial bootstrap
   *  AND each `retryConnection`). The listener's event
   *  handler captures this value at listen-time; a
   *  stale captured value means the listener was
   *  orphaned (e.g. the user hit Retry and the new
   *  listener replaced it) and the event should be
   *  dropped. Distinct from `generation` above, which
   *  only changes on destroy. */
  private connectionGeneration = 0;
  private recoveryPending = 0;
  private recoveryWasReady = false;

  /** The unlisten handle returned by `listen()`. The
   *  component calls it from `OnDestroy` (and from the
   *  late-cleanup path) so the listener never outlives
   *  the component. */
  private currentUnlisten: UnlistenFn | null = null;

  /** True while a long job is in flight, derived from
   *  the latest `currentView().job.phase`. The cancel
   *  button is only enabled while this is `true`. The
   *  value is recomputed by the reducer on every
   *  accepted snapshot — there is NO separate
   *  `jobPending` signal that lives between the
   *  `op_start` resolve and the first event landing;
   *  the reducer's view of `currentView` IS the
   *  authoritative state from the moment `op_start`
   *  resolves. */
  activeJob(): boolean {
    return this.currentView()?.job?.phase === "running";
  }

  /** Job id of the currently-running or just-finished
   *  job, derived from `currentView().job.id`. Used by
   *  the cancel button and the ambiguous-start /
   *  cancel reconcile path. */
  activeJobId(): string | null {
    return this.currentView()?.job?.id ?? null;
  }

  /** Inline two-step confirmation for the dirty-draft
   *  guard. When `native confirm()` is unavailable (e.g.
   *  in a `node --test` harness without a DOM), the
   *  fallback is a two-button "are you sure?" arm: the
   *  user must click the same Discard button twice. The
   *  flag clears after a 5-second window so a stray
   *  click does not get stuck. The Discard confirmation
   *  is INDEPENDENT from the Sync / Skills / Tool
   *  confirmations below — each action captures its
   *  own immutable request so a stale click on
   *  another button cannot accidentally confirm a
   *  different action. */
  readonly dirtyConfirmArmed = signal(false);
  private dirtyConfirmTimer: ReturnType<typeof setTimeout> | null = null;
  /** Separate timer for the action-specific
   *  confirmations (Sync / Skills / Tool). The
   *  previous design shared the Discard timer with
   *  the action confirmations, which meant a
   *  straddle — a Discard click could disarm an
   *  action confirmation mid-arm, or vice-versa,
   *  and the tests could not assert independent
   *  lifetime. Each timer owns one flag group. */
  private actionConfirmTimer: ReturnType<typeof setTimeout> | null = null;

  /** Action-specific inline confirmations. The previous
   *  design re-used `dirtyConfirmArmed` for the Install
   *  Tool button; that was a cross-action bug — a
   *  single shared flag meant a stray click on Discard
   *  could accidentally confirm an install. Each action
   *  now owns its own flag AND its own captured
   *  request (kind + target / id). The arm only
   *  matches if the second click carries the SAME
   *  captured request: any other action in between
   *  disarms the prior arm. The 5-second timer is
   *  shared across all confirmations for consistency
   *  with the Discard flow. */
  readonly syncConfirmArmed = signal<SyncAgentTarget | null>(null);
  readonly skillsConfirmArmed = signal<boolean>(false);
  readonly toolConfirmArmed = signal<string | null>(null);

  /** True while an `op_start` call is in flight. The
   *  flag flips to `true` BEFORE the `invoke()` so a
   *  second mutation (refresh / sync / etc.) cannot
   *  slip in between the resolve and the first event;
   *  it clears once `applyCurrentView` accepts the
   *  initial view OR the `invoke` itself rejects.
   *  Distinct from `busy`: the cancel / status panel
   *  needs to keep working while a start is in flight
   *  (a delayed `op_start` reply must NOT block the
   *  user from hitting Refresh status or Cancel after
   *  the running event has landed). One consistent
   *  representation: a public `signal` the template
   *  reads via `startPending()` and the in-method
   *  guards read via `startPending()` too. The
   *  previous design had BOTH a private field AND a
   *  method with the same name, which the harness's
   *  transpile step rejected as ambiguous. */
  readonly startPending = signal(false);

  /** Cancel / status inflight flags. Each guards ONE
   *  wire call so the two flows cannot overlap
   *  themselves, but neither blocks the other AND
   *  neither blocks short mutations / the start /
   *  the listener. The flags are intentionally NOT
   *  gated on `busy()` so a Refresh status issued
   *  between the `op_start` invoke and its reply
   *  reaches the registry; the user can recover
   *  from a delayed start by hitting Cancel /
   *  Refresh status as soon as the running event
   *  lands (or even before, if they want to see
   *  the registry's latest snapshot). */
  readonly cancelInflight = signal(false);
  readonly statusInflight = signal(false);

  /** Job panel error stream. Cancel / Refresh status
   *  / start action failures route here so they
   *  surface next to the job panel rather than on
   *  the editor's error (cancel / status are
   *  registry-global actions, not editor-scoped). */
  readonly jobError = signal<string | null>(null);

  /** Backend-driven tool catalog. The backend resolves the
   *  catalog from `agenthd::tools::DEFAULT_CATALOG` plus
   *  the per-entry `tool_status`; the frontend does not
   *  maintain a parallel list. Empty until the first
   * `tool_catalog_status` resolve lands; the picker is
   *  disabled while the list is empty. On a
   * `tool_catalog_status` failure the previous rows
   *  are kept AND `toolCatalogError` surfaces the
   *  backend's literal text; the picker keeps the
   *  stale list rather than rendering a fake "not
   *  installed" state. */
  readonly toolCatalog = signal<ToolStatusRow[]>([]);
  readonly toolCatalogError = signal<string | null>(null);
  /** Picker default. Empty until the catalog lands;
   *  flips to the first row's `tool_id` once it does.
   *  An empty picker cannot install anything — the
   *  `Install` button is disabled until this resolves
   *  to a non-empty value. */
  readonly selectedToolId = signal<string>("");

  /** Backend-driven known permission keys. Empty until
   *  the first `permission_keys` resolve lands; the
   *  editor's known-key dropdown is empty until this
   *  is populated. The frontend never maintains a
   *  parallel list. On a `permission_keys` failure
   *  the previous list is kept AND
   *  `knownPermissionKeysError` surfaces the
   *  backend's literal text so custom-permission
   *  editing remains safe against the existing
   *  vocabulary. */
  readonly knownPermissionKeys = signal<string[]>([]);
  readonly knownPermissionKeysError = signal<string | null>(null);

  /** Discovered models list (advisory only; never overwrites
   *  the editor's typed `model`). */
  readonly discoveredModels = signal<string[]>([]);
  readonly discoveryError = signal<string | null>(null);

  ngOnInit(): void {
    // The editor dialog is synced by an
    // `afterRenderEffect` registered at construction
    // time (see the field initialiser block below).
    // The effect re-reads `editing()` / `editDraft()`
    // AND the dialog `viewChild` signal so the dialog
    // opens the moment a draft is loaded AND closes
    // the moment the editor is closed — regardless of
    // which Angular render cycle owned the change.
    void this.bootstrap();
  }

  ngOnDestroy(): void {
    this.editorModalOpen.set(false);
    // Bump the generation so any pending `listen()`
    // resolution sees a stale generation and unsubscribes
    // immediately. The current `currentUnlisten` is
    // called too so the listener never outlives the
    // component. The terminal-drain timer is cancelled
    // so a scheduled drain cannot fire against a
    // destroyed component.
    this.generation += 1;
    if (this.currentUnlisten) {
      try {
        this.currentUnlisten();
      } catch {
        // The Tauri runtime might already have torn down
        // the listener; ignore.
      }
      this.currentUnlisten = null;
    }
    if (this.dirtyConfirmTimer !== null) {
      clearTimeout(this.dirtyConfirmTimer);
      this.dirtyConfirmTimer = null;
    }
    if (this.actionConfirmTimer !== null) {
      clearTimeout(this.actionConfirmTimer);
      this.actionConfirmTimer = null;
    }
    if (this.terminalRefreshTimer !== null) {
      clearTimeout(this.terminalRefreshTimer);
      this.terminalRefreshTimer = null;
    }
    this.pendingTerminalIds.clear();
    this.disarmAllActionConfirms();
  }

  // ===========================================================================
  // Bootstrap: subscribe BEFORE the first `current` call.
  //
  // The contract from the directive is "LISTENER-before-
  // current/bootstrap/start". The race we are closing: an
  // event that fires between `current` and `listen` would
  // be lost (the reducer cannot observe an event whose
  // listener is not yet registered). The new bootstrap
  // registers the listener FIRST so any event that fires
  // during the initial `current` call is captured; the
  // first `current` resolution then catches up via the
  // registry's retained latest snapshot. From then on
  // every incoming event has a strictly-greater `seq`
  // and the reducer applies it. Mutations are disabled
  // until BOTH the listener is registered AND the first
  // `current` has resolved — a window without a working
  // listener would silently desync from the registry, so
  // the only safe posture is fail-closed.
  // ===========================================================================

  private async bootstrap(): Promise<void> {
    // Capture the generation BEFORE any await so the
    // post-await guard compares a frozen snapshot
    // against the live counter (the previous code
    // compared `this.generation !== this.generation`
    // — a tautology that always evaluated to `false`
    // and short-circuited the bootstrap to the
    // `ReadyTrue` path even when `subscribe()` had
    // rejected). A destroy / Retry that bumps
    // `generation` invalidates this snapshot.
    const myGen = this.generation;

    // 1) Register the listener FIRST. The reducer's
    // first event (whatever `seq` it lands on) seeds
    // `lastSeq`. Mutations are NOT enabled yet — the
    // user must not fire-and-forget a `startOperation`
    // before the first `current` has had a chance to
    // seed the reducer. `subscribe()` is responsible
    // for failing closed (calls
    // `handleConnectionFailure`, never flips
    // `subscribed` / `mutationsEnabled` on its error
    // arm).
    await this.subscribe();
    if (myGen !== this.generation) return;
    if (!this.subscribed()) {
      // `subscribe()` rejected AND `handleConnectionFailure`
      // already disabled mutations. The bootstrap
      // stops here; the user must hit Retry to
      // recover. Returning without flipping
      // `mutationsEnabled` to `true` is the
      // fail-closed posture.
      return;
    }
    const myConn = this.connectionGeneration;

    // 2) Issue `op_current`. The reducer accepts the
    // returned view as the first wire observation; the
    // listener is the second wire observation. Either
    // path can win the race; both go through the same
    // monotonic `seq` comparison.
    let currentResult: "accepted" | "stale" | "invalid" | "suppressed";
    try {
      const view = await invoke<CurrentView>("op_current");
      if (myGen !== this.generation) return;
      currentResult = this.applyCurrentView(view, myGen, myConn);
    } catch (e) {
      if (myGen !== this.generation) return;
      this.handleConnectionFailure(this.toMessage(e));
      return;
    }

    // 3) Re-enable mutations only after BOTH
    // subscriptions are healthy AND the initial
    // `op_current` reply was at least a valid
    // wire observation. The `applyCurrentView`
    // outcome is the single source of truth:
    //
    // - `accepted`: a strictly-newer `seq` was
    //   applied; the visible state is in sync.
    // - `stale`: a non-strictly-greater `seq`
    //   (e.g. the listener already pushed the
    //   same snapshot the registry retains);
    //   the visible state is still in sync
    //   through the listener.
    // - `invalid`: the wire reply was malformed;
    //   `handleConnectionFailure` has already
    //   flipped mutations off. Do NOT
    //   unconditionally re-enable.
    // - `suppressed`: a Retry / destroy raced
    //   the call. Stop the bootstrap; the next
    //   call (Retry or fresh bootstrap) owns
    //   this connection.
    //
    // The previous design enabled mutations
    // UNCONDITIONALLY here — that was the bug:
    // a malformed `op_current` would still
    // flip `mutationsEnabled` to `true` and
    // let the user fire-and-forget against a
    // wire that had just failed.
    if (myGen !== this.generation) return;
    if (currentResult === "invalid" || currentResult === "suppressed") {
      return;
    }

    this.mutationsEnabled.set(true);

    // 4) Refresh the read-only panels under a single
    // busy window. The refresh is the same as the
    // D1 slice's `refreshAll`; the new wiring just
    // runs it from the bootstrap path so a freshly-
    // booted window does not need a separate
    // "Refresh both" click. Metadata (tool catalog +
    // permission keys) is awaited in the SAME cycle
    // so the picker / dropdown never render with the
    // previous process's stale rows.
    await this.refreshAll();
    if (myGen !== this.generation) return;

    // 5) Drain any terminal-side refresh markers
    // that the bootstrap's own wire observations
    // might have parked (a starting job that already
    // finished between the `op_current` call and the
    // `refreshAll` resolution — rare but possible on
    // a long-lived registry that retained a snapshot
    // from an earlier session). Drain once now so
    // the post-bootstrap metadata is fresh.
    this.drainTerminalRefresh();
  }

  /** Register the `agenthd-operation` listener. The
   *  reducer (`applyCurrentView`) is the only writer of
   *  the right-hand panel. The listener is set up with
   *  a generation guard so a late resolution after
   *  destroy unsubscribes immediately, AND a per-
   *  connection generation guard so a stale event from
   *  an orphaned listener (replaced by Retry) is
   *  dropped without touching state. The connection
   *  generation is bumped on every fresh subscribe
   *  call. The promise rejects if `listen()` itself
   *  rejects — the caller (`bootstrap` /
   *  `retryConnection`) gates its success path on the
   *  `subscribed()` signal, never on this promise's
   *  resolution, so a failing `listen()` does not
   *  silently flip `mutationsEnabled` to `true`. */
  private async subscribe(): Promise<void> {
    const myGen = this.generation;
    // Bump the connection generation BEFORE the
    // `listen()` call so a previous listener's events
    // are dropped as soon as the new subscribe begins.
    // `subscribe()` is the SINGLE place that bumps
    // the value (the previous design also bumped it
    // in `retryConnection`, which double-counted).
    this.connectionGeneration += 1;
    this.recoveryPending = 0;
    this.recoveryWasReady = false;
    const myConn = this.connectionGeneration;
    try {
      const unlisten = await listen<CurrentView>(
        "agenthd-operation",
        (event) => {
          if (myGen !== this.generation) {
            // The component has been destroyed since
            // this event was queued; the unlisten handle
            // is the cleanup path. Drop the event
            // without touching state.
            return;
          }
          if (myConn !== this.connectionGeneration) {
            // The listener was orphaned (a newer
            // subscribe ran while this one was
            // registering). Drop the event without
            // touching state.
            return;
          }
          this.applyCurrentView(event.payload, myGen, myConn);
        },
      );
      if (myGen !== this.generation) {
        // The component was destroyed while the
        // listener was being registered; unsubscribe
        // immediately so the listener does not outlive
        // the component.
        try {
          unlisten();
        } catch {
          // The Tauri runtime might already have torn
          // down the listener; ignore.
        }
        return;
      }
      if (myConn !== this.connectionGeneration) {
        // A newer subscribe ran while this one was
        // registering; the older listener is orphaned.
        // Unsubscribe so it does not outlive the
        // current connection generation.
        try {
          unlisten();
        } catch {
          // ignore
        }
        return;
      }
      this.currentUnlisten = unlisten;
      this.subscribed.set(true);
      this.connectionError.set(null);
    } catch (e) {
      if (myGen !== this.generation) return;
      // `listen()` itself rejected. Surface the error
      // and fail closed: do NOT flip `subscribed` or
      // `mutationsEnabled` to true. The caller sees
      // `subscribed() === false` and stops the
      // bootstrap / retry flow.
      this.handleConnectionFailure(this.toMessage(e));
    }
  }

  /** User-initiated Retry hook. Re-issues both
   *  `subscribe` and `op_current`. While a Retry is
   *  in flight the existing busy window stays open so
   *  a second click cannot race it. The Retry
   *  captures the post-bump `connectionGeneration`
   *  and threads it through `subscribe()` +
   *  `op_current` + `applyCurrentView` so a stale
   *  reply from the previous connection can never
   *  overwrite the new connection's state. */
  async retryConnection(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      // Drop the current unlisten (if any) so the
      // retry can register a fresh one.
      if (this.currentUnlisten) {
        try {
          this.currentUnlisten();
        } catch {
          // ignore
        }
        this.currentUnlisten = null;
      }
      // New-connection boundary: reset old
      // in-flight flags ONCE. A previous
      // request's caller (startRequest /
      // cancelActiveJob / refreshJobStatus)
      // captured the old (gen, conn); when
      // its reply settles, the suppressed
      // path does nothing (including no
      // flag cleanup), so a stale request
      // cannot hold up the new flow. The
      // flag reset here is the only place
      // that touches the old request's
      // bookkeeping.
      this.startPending.set(false);
      this.cancelInflight.set(false);
      this.statusInflight.set(false);
      this.subscribed.set(false);
      this.mutationsEnabled.set(false);
      const myGen = this.generation;
      // Subscribe FIRST (it owns the
      // `connectionGeneration` bump — see the comment
      // in `subscribe`). The captured `myConn` is
      // read AFTER the subscribe succeeds so a
      // subscribe-time bump cannot be missed.
      await this.subscribe();
      if (myGen !== this.generation) return;
      if (!this.subscribed()) {
        // `subscribe()` rejected. Stop the retry
        // flow; mutations stay disabled until the
        // next Retry succeeds.
        return;
      }
      const myConn = this.connectionGeneration;
      let currentResult: "accepted" | "stale" | "invalid" | "suppressed";
      try {
        const view = await invoke<CurrentView>("op_current");
        if (myGen !== this.generation) return;
        currentResult = this.applyCurrentView(view, myGen, myConn);
      } catch (e) {
        if (myGen !== this.generation) return;
        this.handleConnectionFailure(this.toMessage(e));
        return;
      }
      if (myGen !== this.generation) return;
      if (currentResult === "invalid" || currentResult === "suppressed") {
        // Same posture as bootstrap: do NOT
        // unconditionally re-enable mutations. The
        // apply outcome is the source of truth.
        return;
      }
      this.mutationsEnabled.set(true);
      // The freshly-reconnected registry
      // already retained the latest
      // snapshot. The Retry's `finally`
      // runs `drainTerminalRefresh` (under
      // the existing busy window) so the
      // parked terminal-side markers are
      // consumed in ONE cycle. The
      // metadata calls happen EXACTLY
      // ONCE per Retry.
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Common failure path for both `subscribe` and the
   *  initial `op_current` call. The flag flips
   *  `mutationsEnabled` to `false` so the user cannot
   *  fire-and-forget a `start_operation` while the wire
   *  is down. */
  private handleConnectionFailure(message: string): void {
    this.connectionError.set(message);
    this.mutationsEnabled.set(false);
  }

  /** Outcome of one `applyCurrentView` call. The
   *  reducer returns a small discriminated union
   *  so callers (bootstrap / retry / `startRequest`
   *  / `cancelActiveJob` / `refreshJobStatus`) can
   *  decide whether to enable mutations, drop a
   *  stale snapshot, or report a wire failure. The
   *  previous reducer returned `void`; the only
   *  signal was the boolean "is `mutationsEnabled`
   *  still on?" — which the bootstrap / retry then
   *  flipped on UNCONDITIONALLY after a malformed
   *  `op_current` reply. The new shape makes that
   *  decision explicit and authoritative:
   *
   *  - `accepted`: a strictly-newer `seq` was
   *    applied. The visible state is now in sync
   *    with the registry.
   *  - `stale`: the reducer saw a non-strictly-
   *    greater `seq` (equal or older). The
   *    snapshot was already known; the visible
   *    state is in sync; the call is a successful
   *    no-op.
   *  - `invalid`: the wire reply was malformed
   *    (null, bad envelope, malformed `seq`). The
   *    reducer has already flipped
   *    `mutationsEnabled` to `false` and surfaced
   *    `connectionError`. The visible state is NOT
   *    in sync and the caller must treat the
   *    connection as broken.
   *  - `suppressed`: the generation / connection-
   *    generation guard saw a stale call (Retry
   *    or destroy happened while the call was in
   *    flight). The reply is from a previous
   *    connection and is silently discarded. The
   *    visible state is unchanged. */
  private applyCurrentView(
    view: CurrentView | null,
    guardGen?: number,
    guardConn?: number,
  ): "accepted" | "stale" | "invalid" | "suppressed" {
    if (view === null || view === undefined) {
      // `op_current` / `op_status` always returns
      // a non-null `CurrentView` on the wire today
      // (`CurrentView::initial()` on a freshly-
      // constructed registry); a `null` here is a
      // strict wire-shape failure. Refuse rather
      // than risk desync.
      this.handleConnectionFailure(
        "wire returned a null CurrentView (desync)",
      );
      return "invalid";
    }
    if (guardGen !== undefined && guardGen !== this.generation) {
      // The component has been destroyed or
      // retried since this wire call was issued;
      // the reply belongs to an older connection.
      return "suppressed";
    }
    if (
      guardConn !== undefined &&
      guardConn !== this.connectionGeneration
    ) {
      // A newer subscribe bumped the connection
      // generation since this call was issued.
      return "suppressed";
    }
    if (!this.envelopeShapeValid(view)) {
      // `seq` not a string OR `job` not null + not
      // an object: the lib's contract is
      // `{seq: String, job: Option<JobSnapshot>}`.
      // Anything else is a strict wire-shape failure
      // and is refused the same way a malformed
      // `seq` is.
      this.handleConnectionFailure(
        "wire returned an invalid CurrentView (desync)",
      );
      return "invalid";
    }
    const incoming = this.parseSeq(view.seq);
    if (incoming === null) {
      // Malformed `seq` from the backend — refuse
      // rather than risk a non-monotonic comparison.
      // The fail-closed posture is the same as a
      // stale seq: the wire is suspect, the registry
      // stays authoritative.
      this.handleConnectionFailure(
        "wire returned a malformed seq (desync)",
      );
      return "invalid";
    }
    if (this.lastSeq !== null && incoming <= this.lastSeq) {
      // Stale snapshot. The wire is best-effort, the
      // registry is monotonic; trust the registry and
      // discard. The same seq re-emitted from `op_current`
      // / `op_cancel` (the registry did NOT publish a
      // new event) is the common case here — the
      // reducer drops it cleanly. This is VALID
      // recovery, not a failure: do NOT touch
      // `mutationsEnabled` / `connectionError` here.
      return "stale";
    }
    this.lastSeq = incoming;
    this.currentView.set(view);
    this.updateDiscoveredModels(view);
    return "accepted";
  }

  /** Strict envelope shape check. `seq` must be a
   *  string (the lib's `serde(u64)` is always a base-10
   *  string). `job` must be either `null` OR an object
   *  with the minimum structural shape the reducer
   *  relies on (`id` + `request.kind`). Anything else
   *  is a wire-shape failure and triggers
   *  fail-closed. The check is intentionally light:
   *  `seq` parsing catches the wire's numeric
   *  envelope later, and `JobSnapshot` field access
   *  will surface any structural mismatch in the
   *  template. The purpose here is to reject the
   *  obvious shape failures (primitive seq, scalar
   *  job, missing fields) BEFORE they corrupt
   *  `lastSeq` / `currentView`. */
  private envelopeShapeValid(view: unknown): view is CurrentView {
    if (!view || typeof view !== "object") return false;
    const v = view as { seq?: unknown; job?: unknown };
    if (typeof v.seq !== "string") return false;
    if (v.job === null || v.job === undefined) return true;
    if (typeof v.job !== "object") return false;
    const j = v.job as {
      id?: unknown;
      request?: { kind?: unknown };
      phase?: unknown;
      cancel_requested?: unknown;
      progress?: unknown;
      report?: unknown;
      error?: unknown;
      finish?: unknown;
    };
    if (typeof j.id !== "string") return false;
    if (!j.request || typeof j.request !== "object") return false;
    if (typeof j.request.kind !== "string") return false;
    // The lib's `JobPhase` is `running` or `finished`.
    // Anything else is a wire-shape failure: an
    // unrecognised phase would let `activeJob()`
    // (derived from `phase === "running"`) return
    // `false` even mid-job, silently disabling the
    // cancel button. The reducer rejects unknown
    // phases here rather than letting malformed
    // snapshots enable writes against a stale view.
    if (j.phase !== "running" && j.phase !== "finished") return false;
    if (typeof j.cancel_requested !== "boolean") return false;
    if (j.progress !== null && j.progress !== undefined) {
      if (typeof j.progress !== "object") return false;
      const p = j.progress as {
        stage?: unknown;
        item?: unknown;
        processed?: unknown;
        total?: unknown;
      };
      if (typeof p.stage !== "string") return false;
      if (p.item !== null && p.item !== undefined && typeof p.item !== "string") return false;
      if (typeof p.processed !== "number") return false;
      if (p.total !== null && p.total !== undefined && typeof p.total !== "number") return false;
    }
    if (j.report !== null && j.report !== undefined) {
      if (typeof j.report !== "object") return false;
      const r = j.report as { kind?: unknown };
      if (typeof r.kind !== "string") return false;
    }
    if (j.error !== null && j.error !== undefined && typeof j.error !== "string") return false;
    if (
      j.finish !== null &&
      j.finish !== undefined &&
      j.finish !== "completed" &&
      j.finish !== "cancelled" &&
      j.finish !== "failed"
    ) return false;
    return true;
  }

  /** Parse the wire's `seq: String` as `BigInt`. The
   *  cast is the lossless form on every host (the wire
   *  is always a base-10 string). The regex is a
   *  strict decimal — empty input, leading `+`,
   *  leading zeros, or any non-digit character
   *  produces `null`, which the reducer treats as a
   *  malformed snapshot and ignores. The lib emits
   *  `u64::to_string()`, which is always base-10 with
   *  no leading `+`. */
  private parseSeq(raw: string): bigint | null {
    if (typeof raw !== "string" || raw.length === 0) return null;
    if (!/^[0-9]+$/.test(raw)) return null;
    try {
      const value = BigInt(raw);
      if (value < 0n) return null;
      return value;
    } catch {
      return null;
    }
  }

  /** Refresh Settings via the read-only `settings_status` command. */
  async refreshSettings(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.loadSettingsIntoState();
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Refresh Agents via the read-only `list_agents` command. */
  async refreshAgents(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.loadAgentsIntoState();
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Refresh both panels under a single busy window.
   *  The directive is "no implicit discard": the refresh
   *  does NOT prompt the user to close a dirty editor
   *  draft, and the `did-discard` flag is set ONLY when
   *  the user explicitly invoked `discardEditor`. A
   *  refresh that lands external bytes into the live
   *  list state while the editor is open does not
   *  affect the draft — `editDraft` / `editContext` are
   *  independent signals so the user can finish editing
   *  against the open bytes even when the on-disk file
   *  has drifted. The list state behind the editor
   *  refreshes from `list_agents` on the next call.
   *
   *  Metadata (tool catalog + known permission keys)
   *  is refreshed in the SAME cycle via
   *  `refreshToolsMetadata` so a manual "Refresh both"
   *  click re-tries the metadata fetches the previous
   *  fire-and-forget path silently skipped. Each
   *  sub-loader surfaces its own error; `refreshAll`
   *  does not aggregate them. */
  async refreshAll(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    this.busy.set(true);
    try {
      // `refreshToolsMetadata` itself checks `busy()`
      // and early-returns; under `refreshAll` the
      // busy window is ALREADY held by the outer
      // try/finally, so the gated variant would
      // silently no-op. Use the unlocked loader
      // here so the metadata is part of the same
      // single cycle as Settings + Agents.
      await this.loadSettingsIntoState();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      await this.loadAgentsIntoState();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      await this.refreshToolsMetadataUnlocked();
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Refresh both panels without prompting; used after a
   *  successful save where the post-save refresh is
   *  intentional (the user explicitly saved). The draft is
   *  already closed by then. */
  private async refreshAllAfterSave(): Promise<void> {
    await this.loadSettingsIntoState();
    await this.loadAgentsIntoState();
  }

  /**
   * Save the typed checkout path. The raw buffer is forwarded
   * verbatim; trim / empty-rejection / absolute check / validator
   * messages all live in `workflows::apply_checkout` so TUI and
   * GUI surface identical text. Success updates the draft to the
   * persisted (trimmed) form; failure preserves the draft and
   * surfaces `ApplyError::message()` verbatim. The post-write
   * revalidate runs inside the same `busy` window so refresh
   * errors cannot mask the save result.
   *
   * If the user has an editor draft open OR a long job is
   * running OR a long-job start is in flight, saveCheckout
   * refuses to run — the template's `[disabled]` gate is
   * NOT sufficient (the template is best-effort, the in-method
   * gate is the source of truth):
   *
   * - editor open: a new checkout would orphan the draft.
   * - active long job: the save races the registry's worker.
   * - start pending: a delayed `op_start` reply could land
   *   while the save is in flight and the reducer would have
   *   to drop the reply or replay it post-save.
   *
   * The user must close the editor (save / discard) AND wait
   * for any in-flight job to settle before saving a new
   * checkout path.
   */
  async saveCheckout(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    if (this.editing()) {
      this.saveError.set(
        "close the editor (save or discard) before saving a new checkout path",
      );
      return;
    }
    this.busy.set(true);
    this.saving.set(true);
    this.saveError.set(null);
    const draft = this.checkoutPath();
    try {
      const persisted = await invoke<string>("apply_checkout", {
        checkoutPath: draft,
      });
      this.draftSeeded = true;
      this.checkoutPath.set(persisted);
    } catch (e) {
      this.saveError.set(this.toOperationMessage(e));
    } finally {
      // Always revalidate inside the same busy window before
      // clearing saving/busy so a second save cannot slip in
      // mid-refresh.
      try {
        await this.loadSettingsIntoState();
        await this.loadAgentsIntoState();
      } finally {
        this.saving.set(false);
        this.busy.set(false);
        this.drainTerminalRefresh();
      }
    }
  }

  /** Native input binding (no FormsModule). */
  onCheckoutPathInput(value: string): void {
    this.draftSeeded = true;
    this.checkoutPath.set(value);
  }

  /** Open the inline editor for a specific agent by name.
   *  Calls the read-only `load_agent_for_edit` command;
   *  failure surfaces the backend's literal text and leaves
   *  the list read-only. While the editor is open the row
   *  it belongs to is treated as in-progress and the
   *  `busy` lock blocks overlapping public actions. The
   *  in-method gate ALSO checks `activeJob()` and
   *  `startPending()` (the template's `[disabled]` is
   *  best-effort only — a long-running registry could
   *  flip the visible state between the click and the
   *  invoke, so the source of truth is the in-method
   *  check). Discovery stays allowed (the editor save
   *  gate blocks until any in-flight discovery reaches
   *  a terminal). */
  async openEditor(name: string, isNew = false): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    if (this.editing()) {
      // Already editing another agent — refuse rather than
      // silently swap drafts. The TUI editor mirrors this:
      // it does not open two agents at once.
      return;
    }
    this.busy.set(true);
    this.editLoading.set(true);
    this.editError.set(null);
    try {
      const response = await invoke<AgentEditLoadResponse>(
        isNew ? "load_new_agent_for_edit" : "load_agent_for_edit",
        { name },
      );
      this.editing.set(response.agent.name);
      this.editDraft.set(response.agent);
      this.editOriginal.set(deepCopyDto(response.agent));
      this.editContext.set(response.context);
    } catch (e) {
      this.editError.set(this.toMessage(e));
    } finally {
      this.editLoading.set(false);
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  async newAgent(): Promise<void> {
    await this.openEditor(this.newAgentName(), true);
  }

  /** UI entry point for the Agents-card "New agent"
   *  button. The Agents card no longer renders the
   *  inline name input that the previous design
   *  used as a fixture for `newAgentName` — the
   *  only path to create a new agent is this
   *  button. The handler resets `newAgentName` to
   *  the root default before delegating to the
   *  existing public `newAgent()`. Two reasons:
   *
   *  1. Avoid stale `newAgentName` from a previous
   *     create / cancel cycle leaking into the next
   *     create — the default is always `"new-agent"`.
   *  2. Do not change the public `newAgent()` /
   *     `newAgentName` contract that the test suite
   *     already pins. The fixture in
   *     `app-component.test.mjs` sets
   *     `component.newAgentName.set("fresh")` and
   *     calls `await component.newAgent()` directly;
   *     that path is preserved. */
  async beginNewAgent(): Promise<void> {
    this.newAgentName.set("new-agent");
    await this.newAgent();
  }

  onEditName(value: string): void {
    if (!this.editorMutationAllowed()) return;
    const draft = this.editDraft();
    if (!draft) return;
    this.editActionConfirm.set(null);
    this.editDraft.set({ ...draft, name: value });
  }

  private editorMutationAllowed(): boolean {
    return this.mutationsEnabled() && !this.busy() && !this.activeJob() && !this.startPending();
  }

  private confirmEditorAction(action: "rename" | "delete", context: AgentEditContext, draft: AgentEditDto): boolean {
    const name = draft.name;
    const key = JSON.stringify([action, context, name]);
    const confirmation = this.editActionConfirm();
    if (confirmation?.key === key && sameDto(confirmation.draft, draft)) {
      this.editActionConfirm.set(null);
      return true;
    }
    const message = action === "rename"
      ? `Rename ${context.original_name} to ${name}? Canonical only; installed copies are unchanged. Rename is non-atomic: a failed save may leave the file renamed. No rollback. Click Save again to confirm.`
      : `Delete canonical ${context.original_name}? Installed copies and state are unchanged. Unsaved draft changes will be discarded only if deletion succeeds. Click Delete again to confirm.`;
    this.editActionConfirm.set({ key, message, draft: deepCopyDto(draft) });
    return false;
  }

  cancelEditorAction(): void {
    if (!this.editorMutationAllowed()) return;
    this.editActionConfirm.set(null);
  }

  async deleteEditor(): Promise<void> {
    if (!this.editorMutationAllowed()) return;
    const context = this.editContext();
    const draft = this.editDraft();
    if (!context || !draft || context.original_name === null) return;
    if (!this.confirmEditorAction("delete", context, draft)) return;
    this.busy.set(true);
    this.editSaving.set(true);
    this.editError.set(null);
    try {
      await invoke<void>("delete_agent_edit", { context });
      this.closeEditor();
      try {
        await this.refreshAllAfterSave();
      } catch (e) {
        this.agentsError.set(this.toMessage(e));
      }
    } catch (e) {
      this.editError.set(this.toOperationMessage(e));
    } finally {
      this.editSaving.set(false);
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Save the current editor draft through
   *  `save_agent_edit`. Failure preserves the draft +
   *  context (so the user can edit and retry); success
   *  closes the editor BEFORE the post-save refresh so
   *  the user never sees a "save succeeded" message that
   *  turns into a "refresh failed" banner for the same
   *  logical action. The in-method gate ALSO checks
   *  `activeJob()` and `startPending()` — the template
   *  binding alone is insufficient (a long-running
   *  registry could flip the visible state between the
   *  click and the invoke, so the source of truth is
   *  the in-method check). */
  async saveEditor(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    const draft = this.editDraft();
    const context = this.editContext();
    if (!draft || !context) return;
    if (context.original_name !== null && draft.name !== context.original_name
      && !this.confirmEditorAction("rename", context, draft)) return;
    this.busy.set(true);
    this.editSaving.set(true);
    this.editError.set(null);
    try {
      await invoke<string>("save_agent_edit", {
        context,
        agent: draft,
      });
      // Close the draft BEFORE refreshing: the user committed
      // to the save, so a refresh failure cannot un-commit
      // them. The refresh below happens with no editor open,
      // so the list re-renders against the post-save bytes
      // without our draft re-entering the picture.
      this.closeEditor();
      try {
        await this.refreshAllAfterSave();
      } catch (e) {
        // Save succeeded; refresh failure is non-fatal — we
        // surface the refresh error so the user can manually
        // retry, but we do not pretend the save failed.
        this.agentsError.set(this.toMessage(e));
      }
    } catch (e) {
      // Failure path: the draft + context are preserved. The
      // user can edit and retry.
      this.editError.set(this.toOperationMessage(e));
    } finally {
      this.editSaving.set(false);
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Discard the editor draft. If the draft is dirty, a
   *  two-step confirmation is required. The native
   *  `confirm()` is the first preference (matches the TUI
   *  editor's "are you sure" semantics); when the native
   *  dialog is unavailable (e.g. `node --test` without a
   *  DOM, or a future Tauri build that strips the WebView
   *  dialog), the fallback is an inline two-step "click
   *  Discard again" arm that auto-clears after 5 seconds.
   *  The guard mirrors the TUI editor's "are you sure"
   *  semantics so a silent data-loss path is not possible. */
  async discardEditor(): Promise<void> {
    if (!this.editorMutationAllowed()) return;
    if (!this.editing()) return;
    if (this.isEditDirty()) {
      const ok = await this.confirmDiscard(
        "Discard the unsaved editor changes?",
      );
      if (!ok) return;
    }
    this.closeEditor();
  }

  /** Returns true while the editor is open and at least one
   *  field diverges from the open-time copy. The comparison
   *  is a deep structural equality over DTO + permissions. */
  isEditDirty(): boolean {
    const draft = this.editDraft();
    const original = this.editOriginal();
    if (!draft || !original) return false;
    return !sameDto(draft, original);
  }

  /** True if the editor is currently editing `name`. The
   *  template uses this to render one row in editor mode
   *  and the rest read-only. */
  isEditing(name: string): boolean {
    return this.editing() === name;
  }

  /** Native input binding for the description field. The
   *  setter mutates the `editDraft` signal in place so
   *  consumers see the live value. */
  onEditDescription(value: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    this.editDraft.set({ ...d, description: value });
  }

  onEditModel(value: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    const trimmed = value.trim();
    this.editDraft.set({
      ...d,
      model: trimmed === "" ? null : trimmed,
    });
  }

  /** Search-input handler for the model field. PURE UI
   *  state: typing in the search input MUST NOT mutate
   *  the draft. The select's option list is filtered by
   *  this term; the draft `model` is only written when
   *  the user picks an option from the filtered list
   *  (via `onModelSelectChange`) or types into the
   *  Custom input (via `onEditModel`). The test
   *  `modelSearch-does-not-mutate-draft` pins this
   *  contract: typing arbitrary text into the search
   *  box leaves `editDraft().model` unchanged. */
  onModelSearchInput(value: string): void {
    this.modelSearchTerm.set(value);
  }

  /** Clear the search input on blur. The clear is
   *  UI-only — it does NOT write to the draft. The
   *  reason: a user who scrolls the list with the
   *  filter active and then leaves the input should
   *  not see the filtered view stick. The native
   *  `<select size>` keeps its own value, so a re-
   *  open of the modal starts from the unfiltered
   *  list again. */
  onModelSearchBlur(): void {
    this.modelSearchTerm.set("");
  }

  /** Explicit "Inherit" handler. Resets the
   *  sticky `modelCustomActive` flag (so the
   *  Custom input is hidden again) and clears
   *  the draft model via the same single source
   *  of truth (`onEditModel`) the select-change
   *  handler uses. Exposed as a named method so
   *  tests and future programmatic callers have
   *  one canonical mutator per choice, instead of
   *  having to know the `__inherit__` sentinel
   *  string. */
  chooseInherit(): void {
    this.modelCustomActive = false;
    this.onEditModel("");
  }

  /** Explicit "known model" handler. Resets the
   *  sticky `modelCustomActive` flag (so the
   *  Custom input is hidden again) and writes
   *  the chosen model id through the single
   *  source of truth (`onEditModel`). Exposed
   *  as a named method so tests and future
   *  programmatic callers have one canonical
   *  mutator per choice, instead of having to
   *  know whether a wire id is in the option
   *  list. */
  chooseKnown(modelId: string): void {
    this.modelCustomActive = false;
    this.onEditModel(modelId);
  }

  /** Select-change handler for the model field. The
   *  select's `value` is one of the sentinel keys
   *  (inherit / custom) or a real wire model id
   *  (the `optionKeyForModel` mapping). Picking a
   *  sentinel / id routes through `onEditModel` so
   *  the dirty-flag and confirmation logic see
   *  exactly one source of truth for the model
   *  field. The Custom flag is sticky: once the
   *  user picks Custom, the Custom input stays
   *  revealed until they pick a different option.
   *  The explicit `chooseInherit` / `chooseKnown`
   *  helpers are the canonical mutators; this
   *  method is the single `change`-event entry
   *  point the template binds to. */
  onModelSelectChange(value: string): void {
    if (value === MODEL_KEY_INHERIT) {
      this.chooseInherit();
      return;
    }
    if (value === MODEL_KEY_CUSTOM) {
      // Reveal the custom input. The current custom
      // text is whatever the user typed last; if it
      // is empty, seed it from the draft so the
      // user sees the current value.
      const draft = this.editDraft();
      this.modelCustomInput.set(draft?.model ?? "");
      this.modelCustomActive = true;
      this.onEditModel(this.modelCustomInput());
      return;
    }
    // Real model id.
    this.chooseKnown(value);
  }

  /** Custom-input handler. The Custom input is a
   *  free-text field; every keystroke writes
   *  through `onEditModel` so the same wire-shape
   *  contract as the old single `<input>` holds. */
  onModelCustomInput(value: string): void {
    this.modelCustomInput.set(value);
    this.onEditModel(value);
  }

  /** Map the draft's `model` field to the select's
   *  option key. The select binds via `value`; the
   *  template uses this helper to compute the
   *  currently selected key on every render. The
   *  helper centralises the sentinel-vs-real
   *  distinction so the template never reaches
   *  into the private sentinel constants.
   *
   *  The Custom flag is checked FIRST: once the
   *  user explicitly picks the Custom option, the
   *  select must keep the `__custom__` sentinel
   *  selected (so the Custom input stays revealed
   *  via `isModelCustomActive()`) even if the
   *  current draft model happens to coincide with
   *  a known option. Without this guard, picking
   *  Custom and then accepting the seeded draft
   *  value would silently flip the select back to
   *  the known model id and hide the Custom input.
   *  The known / inherit arms are still the
   *  canonical source of truth for the inherited
   *  / known cases. */
  modelOptionKey(): string {
    if (this.modelCustomActive) return MODEL_KEY_CUSTOM;
    const draft = this.editDraft();
    if (!draft) return MODEL_KEY_INHERIT;
    const m = draft.model;
    if (m === null || m === undefined || m === "") return MODEL_KEY_INHERIT;
    if (this.isInOptionList(m)) return m;
    // The draft carries a model that is NOT in the
    // option list (a manual entry the user typed
    // before a discovery refresh). We surface it via
    // the Custom input rather than the select so
    // the user can keep editing it without the
    // search filter dropping it.
    return MODEL_KEY_CUSTOM;
  }

  /** True when `m` is in the option list (i.e. it
   *  is a real wire model id the select knows
   *  about). The helper reads the dedup-sorted
   *  union of the cache + draft + agents' models.
   *  Pure function over the public state — no
   *  side effects. */
  private isInOptionList(m: string): boolean {
    if (!m) return false;
    const list = this.modelOptions();
    return list.includes(m);
  }

  /** True when the user has explicitly picked the
   *  "Custom…" option from the model select. The
   *  flag is sticky: once the user picks Custom,
   *  the Custom input stays revealed until the
   *  user picks a different option (a real model
   *  id, Inherit, or any other value). The flag
   *  is a counter on the model-select-control
   *  surface — it does NOT depend on whether the
   *  current draft model is in the option list
   *  (the draft model is always added to the
   *  option list so the selected option is
   *  never silently dropped from the visible
   *  list, per the directive's "preserve
   *  current selected option" contract). The
   *  template uses the flag to reveal the
   *  Custom <input>; the input is bound to
   *  `modelCustomInput` and writes through
   *  `onEditModel` on every keystroke. */
  isModelCustomActive(): boolean {
    return this.modelCustomActive;
  }

  /** Set by `onModelSelectChange` when the user
   *  picks the Custom option. Cleared when the
   *  user picks a real model id, Inherit, or
   *  anything else. The flag is a private field
   *  (not a signal) because the public surface
   *  for the template is the read-only
   *  `isModelCustomActive()` helper. */
  private modelCustomActive = false;

  /** Dedup-sorted union of every model id the UI
   *  can suggest. The list is derived from three
   *  sources, in priority order:
   *
   *  1. `discoveredModels()` — the registry's
   *     last-cached discovery result.
   *  2. `agents().map(a => a.model)` — every
   *     model id any loaded canonical agent uses.
   *     Catches the case where the user has not
   *     run discovery yet but already has agents
   *     with known model ids.
   *  3. The current draft's `model` — even if it
   *     is unknown to the discovery cache / the
   *     other agents, the user's draft is always
   *     selectable so they do not lose the value
   *     they typed.
   *
   *  The result is sorted case-sensitively (the
   *  wire model id is the canonical string; the
   *  search input is case-insensitive but the
   *  rendered list is sorted canonically). UNKNOWN
   *  model ids the user typed are preserved
   *  verbatim — the lib rejects invalid model
   *  strings only at save time. */
  modelOptions(): string[] {
    const draft = this.editDraft();
    const seen = new Set<string>();
    const out: string[] = [];
    const push = (m: string | null | undefined) => {
      if (!m) return;
      if (seen.has(m)) return;
      seen.add(m);
      out.push(m);
    };
    for (const m of this.discoveredModels()) push(m);
    for (const a of this.agents()) push(a.model);
    if (draft) push(draft.model);
    out.sort();
    return out;
  }

  /** Filtered view of `modelOptions()` against the
   *  current search term. Case-insensitive
   *  substring match on the model id. The current
   *  draft's model is ALWAYS included even if the
   *  filter would exclude it, so the selected
   *  option is never silently dropped from the
   *  visible list. A "(selected)" suffix is
   *  appended to the option label so the user can
   *  see at a glance which entry is the current
   *  value (the browser would otherwise mark it
   *  selected but the label would still help
   *  visual scanning). */
  filteredModelOptions(): string[] {
    const list = this.modelOptions();
    const term = this.modelSearchTerm().trim().toLowerCase();
    if (!term) return list;
    const matches = list.filter((m) => m.toLowerCase().includes(term));
    // Always include the current draft model so
    // the selected option is never silently
    // dropped.
    const current = this.editDraft()?.model;
    if (current && !matches.includes(current)) {
      matches.push(current);
      matches.sort();
    }
    return matches;
  }

  /** Display label for a model option. Real model
   *  ids render verbatim; the current draft model
   *  (when the filter would otherwise drop it)
   *  gets a "(selected)" suffix so the user can
   *  see the selection even when it is not in
   *  the filtered list. */
  modelOptionLabel(m: string): string {
    const current = this.editDraft()?.model;
    if (m === current) return `${m} (selected)`;
    return m;
  }

  /** True when the filtered list is empty. The
   *  template renders a "no results" hint next to
   *  the select when this is `true`. The select
   *  itself is left enabled (the user can still
   *  pick Inherit / Custom). */
  modelOptionsEmpty(): boolean {
    return this.filteredModelOptions().length === 0;
  }

  /** Editor title. The "New agent" arm fires when
   *  the editor was opened via `beginNewAgent` /
   *  `load_new_agent_for_edit` (the context's
   *  `original_name` is `null`). The "Edit agent"
   *  arm fires for any other open. The "Agent"
   *  label is intentional — the user is editing
   *  the metadata, the title is the lifecycle
   *  name, not the agent's name. */
  editorTitle(): string {
    const ctx = this.editContext();
    if (!ctx) return "Agent";
    return ctx.original_name === null ? "New agent" : "Edit agent";
  }

  /** Header sub-line. Carries the agent's name
   *  (editable) and the dirty marker. Pulled out
   *  of the template so the static-template test
   *  can pin the contract (title comes from
   *  `editorTitle`, name comes from the draft,
   *  dirty marker is a separate `<span>`). */
  editorDirtyLabel(): string {
    return this.isEditDirty() ? "● unsaved" : "";
  }

  onEditPrompt(value: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    this.editDraft.set({ ...d, prompt: value });
  }

  onEditMode(value: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    this.editDraft.set({ ...d, mode: value });
  }

  onEditPermissionChange(key: string, value: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    const next = { ...d.permissions };
    if (value === "(unset)") {
      delete next[key];
    } else {
      next[key] = value;
    }
    this.editDraft.set({ ...d, permissions: next });
  }

  /** Add a free-form permission row. The lib will reject
   *  unknown keys at save time with its standard
   *  "unknown permission key" error, which the backend
   *  surfaces verbatim into `editError`. The frontend
   *  preserves the draft so the user can fix the key. */
  addPermissionKey(key: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    const trimmed = key.trim();
    if (!trimmed) return;
    if (d.permissions[trimmed] !== undefined) return;
    const next = { ...d.permissions, [trimmed]: "ask" };
    this.editDraft.set({ ...d, permissions: next });
  }

  removePermissionKey(key: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    const next = { ...d.permissions };
    delete next[key];
    this.editDraft.set({ ...d, permissions: next });
  }

  /** Returns the merged permission keys for the editor:
   *  known keys first (so the dropdown renders in a stable
   *  order), then any extra keys the loaded draft carries
   *  but are not in the known list. The known-key list
   *  itself is the lib's `PERMISSION_KEYS` projection the
   *  backend returns on every `permission_keys` call; the
   *  frontend does not maintain a parallel list. */
  editPermissionKeys(): string[] {
    const d = this.editDraft();
    if (!d) return [];
    const known = this.knownPermissionKeys();
    const seen = new Set<string>();
    const ordered: string[] = [];
    for (const k of known) {
      if (d.permissions[k] !== undefined) {
        ordered.push(k);
        seen.add(k);
      }
    }
    for (const k of Object.keys(d.permissions)) {
      if (!seen.has(k)) ordered.push(k);
    }
    return ordered;
  }

  /** True if the known-key set already contains this key —
   *  used to hide the `+` button for keys that are
   *  already rendered. */
  isKnownPermission(key: string): boolean {
    return this.knownPermissionKeys().includes(key);
  }

  /** Values the lib accepts for a permission key.
   *  Closed set: `allow` / `ask` / `deny`. Mirrors
   *  `agenthd::agent::PermissionAction`. */
  permissionActions(): string[] {
    return ["allow", "ask", "deny"];
  }

  /** Known permission keys the editor can add via the
   *  dropdown (excludes the keys already present so the
   *  user never adds the same key twice). */
  availableKnownPermissionKeys(): string[] {
    const d = this.editDraft();
    if (!d) return this.knownPermissionKeys().slice();
    return this.knownPermissionKeys().filter(
      (k) => d.permissions[k] === undefined,
    );
  }

  /** Two-step confirmation helper. First preference is
   *  the browser / WebView's native `confirm()` — it is
   *  available in the runtime Tauri uses today and matches
   *  the TUI editor's "are you sure" semantics. When the
   *  native dialog is unavailable (e.g. in a `node --test`
   *  harness without a DOM), the helper falls back to an
   *  inline two-step "click Discard again" arm. The
   *  fallback auto-clears after 5 seconds so a stray
   *  click does not get stuck. The directive specifically
   *  asked for an inline fallback rather than a dialog
   *  plugin so the GUI stays plug-in free. */
  private async confirmDiscard(message: string): Promise<boolean> {
    try {
      if (typeof globalThis.confirm === "function") {
        // `confirm()` is available in the WebView2 /
        // WebKitGTK runtime Tauri uses; keep it on a
        // single line so a test can monkey-patch it
        // cleanly if needed.
        return globalThis.confirm(message);
      }
    } catch {
      // `confirm()` may throw in a non-DOM context;
      // fall through to the inline two-step arm.
    }
    return this.armInlineConfirm();
  }

  /** Inline two-step Discard confirmation. The
   *  first click on Discard arms the confirm; the
   *  second click within 5 seconds confirms; the
   *  timer auto-disarms so a stray click cannot get
   *  stuck. Arming Discard does NOT disarm a
   *  pending action confirmation — the action
   *  confirmation has its own `actionConfirmTimer`. */
  private armInlineConfirm(): Promise<boolean> {
    if (this.dirtyConfirmArmed()) {
      // Second click within the arm window: confirm.
      this.disarmInlineConfirm();
      return Promise.resolve(true);
    }
    this.dirtyConfirmArmed.set(true);
    this.armDiscardTimer();
    return Promise.resolve(false);
  }

  /** Clear the Discard confirmation arm. */
  private disarmInlineConfirm(): void {
    this.dirtyConfirmArmed.set(false);
    if (this.dirtyConfirmTimer !== null) {
      clearTimeout(this.dirtyConfirmTimer);
      this.dirtyConfirmTimer = null;
    }
  }

  /** (Re-)arm the Discard auto-disarm timer.
   *  The timer is `unref()`-ed so a 5-second wait
   *  does NOT keep a `node --test` loop alive past
   *  the test's actual completion. In the WebView2
   *  runtime the timer fires normally; the
   *  `unref()` call is a no-op there. */
  private armDiscardTimer(): void {
    if (this.dirtyConfirmTimer !== null) {
      clearTimeout(this.dirtyConfirmTimer);
    }
    this.dirtyConfirmTimer = setTimeout(() => {
      this.dirtyConfirmArmed.set(false);
      this.dirtyConfirmTimer = null;
    }, 5000);
    // `unref()` is a Node.js extension that keeps
    // the timer from holding the loop open. The
    // browser's `setTimeout` returns a `number`,
    // not a `NodeJS.Timeout`, so the cast needs
    // to go through `unknown` to satisfy the
    // production build's strict type check.
    if (this.dirtyConfirmTimer !== null) {
      const t = this.dirtyConfirmTimer as unknown as { unref?: () => void };
      if (typeof t.unref === "function") t.unref();
    }
  }

  /** Drop the editor state without prompting. Used after a
   *  successful save and from `discardEditor` after the user
   *  has confirmed. */
  private closeEditor(): void {
    this.editActionConfirm.set(null);
    this.editing.set(null);
    this.editDraft.set(null);
    this.editContext.set(null);
    this.editOriginal.set(null);
    this.editError.set(null);
    this.modelSearchTerm.set("");
    this.modelCustomInput.set("");
    this.modelCustomActive = false;
  }

  /** Sync the editor <dialog> open/close state with the
   *  `editing()` / `editDraft()` signals. Idempotent:
   *  the body reads the dialog's actual `open` state
   *  into `editorDialogNativeOpen` so `showModal()` is
   *  NEVER called twice in a row and `close()` is NEVER
   *  called on an already-closed dialog. The hook is
   *  driven by an `afterRenderEffect` that re-runs on
   *  any change to `editing()` / `editDraft()` /
   *  the dialog viewChild signal — Angular's signal-
   *  based reactivity guarantees the effect runs AFTER
   *  Angular has flushed the rendered DOM, so the
   *  dialog element is guaranteed to exist when the
   *  callback fires (an absent dialog short-circuits
   *  cleanly; the next effect run re-tries).
   *
   *  Exceptions from `showModal` are surfaced via the
   *  `editError` signal so the user can see why the
   *  dialog did not open; the `editorDialogNativeOpen`
   *  flag is NOT flipped on failure so the next sync
   *  re-tries. `close()` failures are non-fatal (a
   *  dialog that already closed itself through the
   *  native cancel path leaves the flag out of sync;
   *  the next sync corrects it).
   *
   *  On open, the first meaningful input is the name
   *  field — focus moves there after a microtask so the
   *  dialog is fully rendered before the focus call.
   *  On close, the captured opener is re-focused (if any)
   *  so the keyboard flow returns to the agents list.
   *  The opener capture is a synchronous handler on the
   *  trigger button; no `document.activeElement` fallback
   *  is used because the test harness runs without a DOM
   *  and the contract is purely the captured-currentTarget
   *  path. */
  private syncEditorDialog(): void {
    const ref = this.editorDialog();
    const dialog = ref?.nativeElement;
    if (!dialog) return;
    // Native Escape can close before the next Angular render. Consult the DOM,
    // not the cached flag, so an immediate reopen still calls showModal.
    this.editorDialogNativeOpen = dialog.open;
    const shouldBeOpen = this.editing() !== null && this.editDraft() !== null;
    if (shouldBeOpen && !this.editorDialogNativeOpen) {
      try {
        dialog.showModal();
      } catch (e) {
        this.editError.set(`failed to open the editor: ${this.toMessage(e)}`);
        return;
      }
      this.editorDialogNativeOpen = true;
      this.editorModalOpen.set(true);
      // Move focus to the name input. Microtask is
      // enough — the dialog is already in the top
      // layer by the time the promise resolves.
      queueMicrotask(() => {
        const nameInput = dialog.querySelector<HTMLInputElement>("#edit-agent-name");
        if (nameInput && document.activeElement !== nameInput) {
          try {
            nameInput.focus();
          } catch {
            // ignore
          }
        }
      });
    } else if (!shouldBeOpen && this.editorDialogNativeOpen) {
      try {
        dialog.close();
      } catch {
        // ignore
      }
      this.editorDialogNativeOpen = false;
      this.editorModalOpen.set(false);
      // Return focus to the opener so the user does
      // not lose context. The opener is the button
      // the user clicked to open the editor; if it
      // is no longer in the DOM (e.g. the agents
      // list refreshed) the focus call is skipped.
      const opener = this.editorOpener;
      this.editorOpener = null;
      if (opener && document.body.contains(opener)) {
        try {
          opener.focus();
        } catch {
          // ignore
        }
      }
    }
    this.editorModalOpen.set(dialog.open);
  }

  onDialogClose(): void {
    // A queued close event from the previous cycle must not clear a reopened modal.
    if (this.editorDialog()?.nativeElement.open) return;
    this.editorDialogNativeOpen = false;
    this.editorModalOpen.set(false);
  }

  /** Capture the opener button when the user clicks
   *  Edit or New. The handler is wired on the
   *  trigger button in the template as
   *  `(click)="captureOpener($event)"` so the
   *  handler runs BEFORE the `openEditor` /
   *  `beginNewAgent` call. The captured element is
   *  the only path that re-focuses on close; the
   *  test harness reads the handler name to verify
   *  the wiring. */
  captureOpener(event: Event): void {
    const target = event.target as HTMLElement | null;
    if (!target) return;
    // The button itself is the most common case; if
    // the user clicked an SVG / text child, walk up
    // to the enclosing button. The template binds
    // the click on the `<button>` element, so this
    // guard is defensive — the click event's
    // currentTarget (if we used it) would be the
    // button too, but we use `target` to keep the
    // harness simple.
    const button = target.closest("button") ?? target;
    this.editorOpener = button instanceof HTMLElement ? button : null;
  }

  /** Native <dialog> `cancel` event — fired when the
   *  user presses Escape, which would otherwise
   *  close the dialog bypassing the dirty-draft
   *  guard. We ALWAYS intercept and route through
   *  `discardEditor` so the same dirty-confirm
   *  flow used for the explicit Discard button is
   *  applied. The `cancel` event is `cancelable`;
   *  `preventDefault()` keeps the dialog open while
   *  the confirmation is pending. After a successful
   *  `discardEditor`, the editor signals flip, the
   *  `afterRenderEffect` sync closes the dialog.
   *  The `cancel` event does NOT bubble as a `click`,
   *  so the backdrop click handler and the cancel
   *  handler never fire on the same gesture. */
  onDialogCancel(event: Event): void {
    event.preventDefault();
    void this.discardEditor();
  }

  /** Backdrop click handler. A native <dialog>'s `click`
   *  bubbles when the user clicks anywhere inside or on
   *  the dialog itself (padding, borders, empty
   *  regions). To distinguish "inside the dialog"
   *  from "outside the dialog (backdrop)", the handler
   *  requires BOTH:
   *
   *    1. `event.target === dialog` (a click on the
   *       dialog element itself, e.g. its padding or
   *       borders — clicks on children target the
   *       child, NOT the dialog, so child clicks NEVER
   *       close the dialog).
   *
   *    2. The pointer coordinates
   *       (`event.clientX` / `event.clientY`) are
   *       OUTSIDE the dialog's `getBoundingClientRect()`
   *       (a click on the dialog's padding sits INSIDE
   *       the rect — that is a padding click, not a
   *       backdrop click, and the dialog must NOT
   *       close).
   *
   *  When both conditions hold, the click is treated
   *  as a backdrop click. The click routes through
   *  `discardEditor()` so the same dirty-draft guard
   *  used by the explicit Discard button and the
   *  Escape key is enforced. A click while
   *  `busy`/`editSaving`/`startPending`/`activeJob`
   *  is in flight is ignored — those gates block the
   *  close buttons in the footer, so they must also
   *  block the backdrop close (a partial cancel /
   *  save in flight must NOT be undone by a stray
   *  click on the backdrop).
   *
   *  The native `<dialog>` `cancel` event (Escape key)
   *  is a SEPARATE handler that ALWAYS intercepts and
   *  routes through `discardEditor()`; the `cancel`
   *  event does not bubble as a `click`, so the two
   *  paths never fire on the same gesture. */
  onDialogBackdropClick(event: MouseEvent): void {
    const ref = this.editorDialog();
    const dialog = ref?.nativeElement;
    if (!dialog) return;
    if (event.target !== dialog) return;
    const rect = dialog.getBoundingClientRect();
    const insideRect =
      event.clientX >= rect.left &&
      event.clientX <= rect.right &&
      event.clientY >= rect.top &&
      event.clientY <= rect.bottom;
    if (insideRect) return;
    // Backdrop click: route through the dirty-draft
    // guard. `discardEditor` returns silently when
    // the editor is busy / saving / a job is running.
    if (this.busy() || this.editSaving() || this.startPending() || this.activeJob()) {
      return;
    }
    void this.discardEditor();
  }

  /**
   * Seed the draft once from the first Settings load.
   * Ready and Stale prefill; Empty and Error leave the buffer
   * alone. Never runs again — the user might be mid-edit.
   */
  private seedDraftFrom(response: SettingsResponse): void {
    if (this.draftSeeded) return;
    const s = response.status;
    if (s.kind === "ready") {
      this.checkoutPath.set(s.checkout_path);
      this.draftSeeded = true;
    } else if (s.kind === "stale") {
      this.checkoutPath.set(s.raw_path);
      this.draftSeeded = true;
    }
  }

  /** Private read-only command call. No lock here. */
  private loadSettings(): Promise<SettingsResponse> {
    return invoke<SettingsResponse>("settings_status");
  }

  /** Private read-only command call. No lock here. */
  private loadAgents(): Promise<AgentsList> {
    return invoke<AgentsList>("list_agents");
  }

  /** Fetch Settings, update signals, seed the draft if first time.
   *  Never runs while the editor is open (the editor owns its
   *  own context — a settings refresh here would not affect
   *  the editor's draft but could mask a `checkout_path`
   *  drift). */
  private async loadSettingsIntoState(): Promise<void> {
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    this.settingsLoading.set(true);
    this.settingsError.set(null);
    try {
      const response = await this.loadSettings();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.settings.set(response);
      this.seedDraftFrom(response);
    } catch (e) {
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.settingsError.set(this.toMessage(e));
      this.settings.set(null);
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) this.settingsLoading.set(false);
    }
  }

  /** Fetch Agents, update signals. The list lives in the
   *  background even while the editor is open: the user
   *  sees the row they are editing with the editor, and
   *  the rest of the list as read-only summaries. The
   *  editor's own draft is kept separate, so a refresh
   *  cannot overwrite it. */
  private async loadAgentsIntoState(): Promise<void> {
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    this.agentsLoading.set(true);
    this.agentsError.set(null);
    try {
      const list = await this.loadAgents();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.agents.set(list.agents);
      this.agentsError.set(list.error);
    } catch (e) {
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.agentsError.set(this.toMessage(e));
      this.agents.set([]);
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) this.agentsLoading.set(false);
    }
  }

  /** Settings status display label. Mirrors the four arms the Rust command returns. */
  settingsLabel(): string {
    const s = this.settings();
    if (!s) return "(not loaded)";
    switch (s.status.kind) {
      case "empty":
        return "Empty — no settings.json yet";
      case "ready":
        return `Ready — ${s.status.checkout_path}`;
      case "stale":
        return `Stale — ${s.status.raw_path}`;
      case "error":
        return `Error — ${s.status.message}`;
    }
  }

  /** Per-status details (key/value pairs) for the Settings panel. */
  settingsDetails(): Array<[string, string]> {
    const s = this.settings();
    if (!s) return [];
    const rows: Array<[string, string]> = [["settings_file", s.settings_file]];
    switch (s.status.kind) {
      case "empty":
        rows.push(["kind", "empty"]);
        break;
      case "ready":
        rows.push(["kind", "ready"]);
        rows.push(["checkout_path", s.status.checkout_path]);
        break;
      case "stale":
        rows.push(["kind", "stale"]);
        rows.push(["raw_path", s.status.raw_path]);
        rows.push(["banner", s.status.banner]);
        break;
      case "error":
        rows.push(["kind", "error"]);
        rows.push(["message", s.status.message]);
        break;
    }
    return rows;
  }

  // ===========================================================================
  // D3 long-job commands.
  // ===========================================================================

  /** Start a `SyncAgents` job. The action refuses when
   *  the editor is open (a sync could land external
   *  bytes that would drift the editor's `prior_hash`)
   *  AND refuses when a job is already running.
   *  The two-step inline confirmation is action-specific
   *  (captured `target`); a stray click on a different
   *  Sync button disarms the prior arm so the second
   *  click cannot accidentally confirm a different
   *  target. The cancel / status panel stays usable
   *  while the start is in flight because the action
   *  flips `startPending` (not the global `busy`)
   *  before the `invoke` call. */
  async startSyncAgents(target: SyncAgentTarget): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    if (this.editing()) {
      this.editError.set(
        "close the editor (save or discard) before running a sync",
      );
      return;
    }
    // First click arms, second click within 5s
    // confirms with the SAME target.
    this.selectedPlanTarget.set(target);
    if (this.syncConfirmArmed() !== target) {
      this.disarmAllActionConfirms();
      this.syncConfirmArmed.set(target);
      this.armActionConfirmTimer();
      return;
    }
    this.disarmAllActionConfirms();
    await this.startRequest(
      { kind: "sync_agents", target } as OperationRequest,
    );
  }

  /** Start an `InstallSkills` job. Same rule as
   *  `startSyncAgents`; the confirmation captures the
   *  action kind rather than a target id. */
  async startInstallSkills(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    if (this.editing()) {
      this.editError.set(
        "close the editor (save or discard) before installing skills",
      );
      return;
    }
    if (!this.skillsConfirmArmed()) {
      this.disarmAllActionConfirms();
      this.skillsConfirmArmed.set(true);
      this.armActionConfirmTimer();
      return;
    }
    this.disarmAllActionConfirms();
    await this.startRequest(
      { kind: "install_skills" } as OperationRequest,
    );
  }

  /** Start an `InstallTool` job for the picker-selected
   *  tool id. The action refuses when the editor is
   *  open. The confirmation captures the picked
   *  `tool_id`: changing the picker mid-arm disarms
   *  the prior arm so the second click on the SAME
   *  tool_id confirms. */
  async startInstallTool(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    if (this.editing()) {
      this.editError.set(
        "close the editor (save or discard) before installing a tool",
      );
      return;
    }
    const toolId = this.selectedToolId();
    if (!toolId) return;
    if (this.toolConfirmArmed() !== toolId) {
      this.disarmAllActionConfirms();
      this.toolConfirmArmed.set(toolId);
      this.armActionConfirmTimer();
      return;
    }
    this.disarmAllActionConfirms();
    await this.startRequest({
      kind: "install_tool",
      tool_id: toolId,
    } as OperationRequest);
  }

  /** Picker handler. The catalog list is backend-driven
   *  (see `toolCatalog` signal). The picker is bound to
   *  the selected `tool_id`; the action uses the same
   *  id when calling `op_start`. Changing the picker
   *  disarms the Tool install confirmation so the user
   *  cannot accidentally confirm a now-stale target.
   *  The disarm only touches the tool-specific flag
   *  — the shared `actionConfirmTimer` is left
   *  running so a Sync / Skills arm that was set
   *  just before the picker change is not
   *  accidentally cleared. (The previous design
   *  cancelled the shared timer, which would kill
   *  any pending Sync / Skills auto-disarm.) */
  onToolSelected(value: string): void {
    this.selectedToolId.set(value);
    if (this.toolConfirmArmed() !== null) {
      this.toolConfirmArmed.set(null);
    }
  }

  /** Start a `DiscoverModels` job. Discovery is allowed
   *  while the editor is open (the result is advisory);
   *  the editor's save gate blocks until any in-flight
   *  discovery reaches a terminal snapshot. Discovery
   *  has NO destructive confirmation — it does not
   *  write to the checkout, it only reads. */
  async startDiscoverModels(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    if (this.startPending()) return;
    if (this.activeJob()) return;
    this.discoveryError.set(null);
    await this.startRequest(
      { kind: "discover_models" } as OperationRequest,
    );
  }

  /** Cancel the active long job. The action is a
   *  no-op when no job is in flight OR when a start
   *  is still pending (no `jobId` is known yet — the
   *  start's running event will land through the same
   *  listener path). The cancel button is gated by
   *  `activeJob()` / `activeJobId()` (the job id the
   *  reducer installed, not a separately-tracked
   *  pending id). Public: usable while `busy()` is
   *  `true` IF `startPending` is `false` (a Refresh /
   *  Cancel issued between `op_start` and its reply
   *  must reach the registry so the panel can recover
   *  the latest snapshot). Cancel is also reachable
   *  while `mutationsEnabled` is `false` if a job is
   *  currently known to be running — the user must
   *  be able to stop a job even when the connection
   *  is otherwise unhealthy.
   *
   *  `activeJobId()` returns the latest view's job
   *  id, which can be a retained finished-job id
   *  if the user clicked cancel after the
   *  terminal landed. The in-method check uses
   *  `isJobRunning()` (derived from the latest
   *  view's `phase === "running"`) to refuse the
   *  call when the job is not actually running —
   *  the registry would reject the cancel as
   *  `unknown_job` and surface a redundant
   *  job-error. The directive: the cancel
   *  button is disabled when the job is not
   *  running, so the in-method check is a
   *  belt-and-braces guard for the disabled
   *  binding (a stale snapshot between a
   *  click and the in-method check would
   *  otherwise slip through). */
  async cancelActiveJob(): Promise<void> {
    if (this.cancelInflight()) return;
    if (!this.isJobRunning()) return;
    const jobId = this.activeJobId();
    if (!jobId) return;
    this.cancelInflight.set(true);
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    try {
      const view = await invoke<CurrentView>("op_cancel", { jobId });
      if (myGen !== this.generation) return;
      const result = this.applyCurrentView(view, myGen, myConn);
      if (result === "invalid") {
        // The wire reply was malformed; the reducer
        // already flipped `mutationsEnabled` to
        // `false` and surfaced `connectionError`.
        // The current job's state is on the listener
        // path (or on the pre-invoke snapshot) —
        // either way, the user must hit Retry to
        // recover. The cancel `invoke` itself
        // succeeded; only the response was bad, so
        // the cancel is treated as "best-effort
        // accepted" — we do NOT surface a redundant
        // job error.
      }
    } catch (e) {
      if (myGen !== this.generation) return;
      if (myConn !== this.connectionGeneration) return;
      // Surface the cancel rejection on the Job
      // panel's error stream (not `editError` —
      // cancel is a registry-global action that
      // happens regardless of editor state). The
      // helper below normalises the wire-shape to a
      // stable string.
      this.jobError.set(this.toOperationMessage(e));
      await this.recoverFromOperationReject(myGen, myConn);
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) {
        this.cancelInflight.set(false);
        this.drainTerminalRefresh();
      }
    }
  }

  /** Refresh the right-hand panel by re-issuing
   *  `op_current`. The registry's latest snapshot is the
   *  recovery fallback for any missed event; the user
   *  can hit this button explicitly when a panel looks
   *  stale. Public: usable while a job is running
   *  (status is always available) AND during a
   *  pending start (the latest snapshot can settle
   *  an uncertain `op_start` reply). Reachable while
   *  `mutationsEnabled` is `false` (status is a
   *  read-only operation that the user can fire any
   *  time the listener is healthy); but the user
   *  MUST NOT have mutations enabled by a status
   *  reply alone — the listener is the wire source
   *  of truth and the only place that re-enables
   *  mutations is a successful `subscribe()` +
   *  valid `op_current` (Retry path). */
  async refreshJobStatus(): Promise<void> {
    if (this.statusInflight()) return;
    this.statusInflight.set(true);
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    try {
      const view = await invoke<CurrentView>("op_current");
      if (myGen !== this.generation) return;
      this.applyCurrentView(view, myGen, myConn);
    } catch (e) {
      if (myGen !== this.generation) return;
      if (myConn !== this.connectionGeneration) return;
      this.jobError.set(this.toOperationMessage(e));
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) {
        this.statusInflight.set(false);
        this.drainTerminalRefresh();
      }
    }
  }

  /** Shared `op_start` flow used by every Start
   *  action above. The method:
   *
   *  1. Sets `startPending = true` BEFORE the
   *     `invoke` so a second mutation cannot slip in
   *     between the reply and the first event landing.
   *     The flag stays `true` until the reply settles
   *     (accepted, stale, OR rejected) — the
   *     previous design cleared it in `finally`
   *     regardless of which path won, which let a
   *     user click a Start button again while the
   *     registry was still resolving the previous
   *     invoke; a fast double-click could issue two
   *     `op_start` calls and confuse the registry.
   *  2. Issues `invoke("op_start", request)` with the
   *     captured generation + connection-generation
   *     so a late reply (after Retry / destroy) cannot
   *     overwrite the new connection's state.
   *  3. Threads the reply through `applyCurrentView`,
   *     which has its own generation + connection-
   *     generation guards. The `applyCurrentView`
   *     outcome is the source of truth for whether
   *     the registry returned a known state: an
   *     `accepted` or `stale` result means the
   *     reply is a real wire observation; an
   *     `invalid` result means the wire failed
   *     closed.
   *  4. On reject: surfaces the error AND
   *     optionally issues a recovery `op_current`
   *     so the visible state is reconciled before
   *     the user can fire a new start. The
   *     recovery uses the same generation +
   *     connection-generation guards so a Retry
   *     that races the recovery suppresses the
   *     stale reply.
   *  5. Recovery temporarily disables mutations and restores only previously
   *     healthy readiness after a valid reply on the same connection. The
   *     caller releases its flag only while it still owns that connection.
   *
   *  The method does NOT flip the global `busy`: the
   *  cancel / status panel needs to stay reachable
   *  while a start is in flight, and the registry is
   *  single-threaded so two starts cannot overlap. */
  private async startRequest(request: OperationRequest): Promise<void> {
    if (this.startPending()) return;
    this.startPending.set(true);
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    try {
      const outcome = await invoke<LongOutcome>("op_start", { request });
      if (myGen !== this.generation) return;
      const result = this.applyCurrentView(outcome.view, myGen, myConn);
      if (result === "invalid") {
        // The start reply itself was malformed.
        // The reducer already flipped
        // `mutationsEnabled` to `false` and
        // surfaced `connectionError`. The user
        // must hit Retry to recover. We do NOT
        // also surface a redundant job error —
        // the wire-shape error is more useful.
        return;
      }
    } catch (e) {
      if (myGen !== this.generation) return;
      if (myConn !== this.connectionGeneration) return;
      // Surface the start rejection on the Job
      // panel. The editor's `editError` is reserved
      // for editor-scoped failures; the registry
      // refuses starts with `OperationError::Busy`
      // / `FailedPreconditions` / `Spawn` that are
      // not editor concerns.
      this.jobError.set(this.toOperationMessage(e));
      // A failed `op_start` may have left the
      // registry in an unknown state. The
      // previous design cleared `startPending`
      // in `finally` and let the user fire a new
      // start against a possibly-stale visible
      // state — a fast double-click could issue
      // two `op_start` calls in parallel. The
      // new design keeps `startPending` true
      // until the recovery `op_current` settles
      // (or the recovery itself fails), so a
      // second click during recovery refuses
      // to re-issue `op_start`. The recovery
      // is best-effort: a Retry that races it
      // (bumps `connectionGeneration`) will
      // suppress the recovery reply, and the
      // Retry's own `subscribe()` + `op_current`
      // path will re-enable mutations only if
      // its own reply is valid. A failed
      // recovery is itself a connection
      // failure — `handleConnectionFailure` is
      // the only place that flips the flag.
      await this.recoverFromOperationReject(myGen, myConn);
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) {
        this.startPending.set(false);
        this.drainTerminalRefresh();
      }
    }
  }

  /** Reconcile an uncertain Start / Cancel before releasing its caller's flag.
   *  A valid reply can restore readiness, but cannot heal a failed connection. */
  private async recoverFromOperationReject(
    myGen: number,
    myConn: number,
  ): Promise<void> {
    if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
    if (this.recoveryPending === 0) this.recoveryWasReady = this.mutationsEnabled();
    this.recoveryPending += 1;
    this.mutationsEnabled.set(false);
    try {
      if (!this.subscribed()) {
        this.handleConnectionFailure(
          this.connectionError() ?? "operation rejected; retry to recover",
        );
        return;
      }
      const view = await invoke<CurrentView>("op_current");
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.applyCurrentView(view, myGen, myConn);
    } catch (e) {
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      this.handleConnectionFailure(this.toMessage(e));
    } finally {
      if (myGen === this.generation && myConn === this.connectionGeneration) {
        this.recoveryPending -= 1;
        if (this.recoveryPending === 0 && this.recoveryWasReady &&
            this.subscribed() && this.connectionError() === null) {
          this.mutationsEnabled.set(true);
        }
      }
    }
  }

  /** Disarm every action-specific confirmation. Used
   *  before arming a new one and from the timer. The
   *  Discard confirmation has its own flag and is
   *  independent. */
  private disarmAllActionConfirms(): void {
    this.syncConfirmArmed.set(null);
    this.skillsConfirmArmed.set(false);
    this.toolConfirmArmed.set(null);
  }

  /** Cancel the action-confirm auto-disarm timer.
   *  The action confirmations have their own timer
   *  slot so the Discard confirmation's lifetime is
   *  independent — arming one does not disarm the
   *  other. */
  private cancelActionConfirmTimer(): void {
    if (this.actionConfirmTimer !== null) {
      clearTimeout(this.actionConfirmTimer);
      this.actionConfirmTimer = null;
    }
  }

  /** (Re-)arm the action-confirm auto-disarm timer.
   *  Mirrors the Discard confirmation's 5-second
   *  lifetime. The timer is `unref()`-ed so the
   *  `node --test` harness does not stay alive past
   *  a test that armed a confirmation but did not
   *  disarm it explicitly. */
  private armActionConfirmTimer(): void {
    this.cancelActionConfirmTimer();
    this.actionConfirmTimer = setTimeout(() => {
      this.disarmAllActionConfirms();
      this.actionConfirmTimer = null;
    }, 5000);
    // `unref()` is a Node.js extension. See the
    // note in `armDiscardTimer` for the cast.
    if (this.actionConfirmTimer !== null) {
      const t = this.actionConfirmTimer as unknown as { unref?: () => void };
      if (typeof t.unref === "function") t.unref();
    }
  }

  /** Refresh the backend-driven metadata: tool catalog
   *  (`tool_catalog_status`) and known permission keys
   *  (`permission_keys`). The frontend never maintains a
   *  parallel list — the catalog + dropdown render
   *  whatever the backend sent on the last call.
   *
   *  Both calls are awaited via `Promise.allSettled`
   *  so a failing one does not skip the other; the
   *  helper runs under a busy window and clears it in
   *  its own `finally` so the caller never sees a
   *  half-cleared busy. The two failure paths:
   *
   *  - `tool_catalog_status` rejection: keep the
   *    existing `toolCatalog` rows verbatim, surface
   *    the backend's literal text in
   *    `toolCatalogError`. The picker keeps the
   *    previous inventory rather than rendering a
   *    fake "not installed" state.
   *  - `permission_keys` rejection: keep the
   *    existing `knownPermissionKeys` list verbatim so
   *    the editor's custom-permission editing keeps
   *    its existing vocabulary intact; surface the
   *    error in `knownPermissionKeysError`.
   *
   *  Public: the caller (`refreshAll` /
   *  `drainTerminalRefresh` / bootstrap) awaits this
   *  so the picker / dropdown render the same rows
   *  that the post-installations `CurrentView` would
   *  have described. */
  private async refreshToolsMetadata(): Promise<void> {
    if (!this.mutationsEnabled()) return;
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.refreshToolsMetadataUnlocked();
    } finally {
      this.busy.set(false);
      this.drainTerminalRefresh();
    }
  }

  /** Apply a discovered model to the editor. The action
   *  only fires when the editor is open AND a discovery
   *  job is **terminal** AND the user picked an item
   *  from the discovered list. The wire `RowOutcome` /
   *  `ToolTerminal` / `DiscoveryTerminal` projection is
   *  hand-off to the `model` field of the draft; the
   *  editor's `onEditModel` normalises the input the
   *  same way a typed entry would. */
  applyDiscoveredModel(model: string): void {
    if (!this.editorMutationAllowed()) return;
    const d = this.editDraft();
    if (!d) return;
    this.editDraft.set({ ...d, model });
  }

  /** Discovered models list. Empty when no discovery job
   *  has run yet. The reducer does not overwrite the
   *  editor's typed `model`; the user must explicitly
   *  click an item to apply it. */
  discoveredModelList(): string[] {
    return this.discoveredModels();
  }

  /** View helpers for the template. */

  currentJob(): JobSnapshot | null {
    return this.currentView()?.job ?? null;
  }

  /** Returns true while a job is in flight (the registry
   *  has a `running` snapshot). The Cancel button is
   *  enabled when this is true; the Refresh-status
   *  button is always enabled (status is always
   *  available during a job). */
  isJobRunning(): boolean {
    const job = this.currentJob();
    return job?.phase === "running";
  }

  /** Render the active job's progress / terminal into a
   *  single string the template can show. The label is
   *  intentionally compact; the report list is the
   *  authoritative per-row view below it. The leading
   *  segment is the human-readable operation name
   *  (presentation only — the wire `request.kind`
   *  stays the lowercase identifier). */
  jobLabel(): string {
    const job = this.currentJob();
    if (!job) return "sin operación";
    const op = this.activityOperationLabel(job.request.kind);
    const stage = job.progress?.stage;
    const item = job.progress?.item ? ` · ${job.progress.item}` : "";
    if (stage) return `${op} · ${stage}${item}`;
    return `${op}${item}`;
  }

  /** Human-readable Spanish label for a wire
   *  `JobRequest::kind` value. The mapping is purely
   *  presentation: the wire stays lowercase so the
   *  reducer / DTO / backend contract are unchanged.
   *  Unknown kinds round-trip as their identifier
   *  (no lossy translation). */
  activityOperationLabel(kind: string): string {
    switch (kind) {
      case "sync_agents":
        return "Sincronizar agentes";
      case "install_skills":
        return "Instalar habilidades";
      case "install_tool":
        return "Instalar herramienta";
      case "discover_models":
        return "Descubrir modelos";
      default:
        return kind;
    }
  }

  /** Human-readable Spanish label for a wire
   *  `JobSnapshot::finish` value (or a phase string
   *  when no finish is present yet). Presentation
   *  only. Unknown values round-trip verbatim. */
  activityFinishLabel(value: string): string {
    switch (value) {
      case "completed":
        return "completada";
      case "failed":
        return "falló";
      case "cancelled":
        return "cancelada";
      case "running":
        return "en curso";
      case "starting":
        return "iniciando";
      case "pending":
        return "pendiente";
      default:
        return value;
    }
  }

  /** Per-row outcome list for the active job. Returns an
   *  empty array for job kinds that do not have rows
   *  (InstallTool / DiscoverModels) so the template
   *  renders an empty table without a type guard. */
  jobOutcomeRows(): RowOutcome[] {
    const job = this.currentJob();
    const report = job?.report;
    if (!report) return [];
    if (report.kind === "sync_agents") return report.outcomes;
    if (report.kind === "install_skills") return report.outcomes;
    return [];
  }

  /** Job-specific terminal detail for the panel. */
  jobToolTerminal(): ToolTerminal | null {
    const job = this.currentJob();
    if (job?.report?.kind === "install_tool") return job.report.terminal;
    return null;
  }

  jobDiscoveryTerminal(): DiscoveryTerminal | null {
    const job = this.currentJob();
    if (job?.report?.kind === "discover_models") return job.report.terminal;
    return null;
  }

  /** Render a brief label of the workflow's
   *  `observed_state` for `sync_agents` /
   *  `install_skills` jobs. The wire field is an
   *  opaque JSON value the lib populated at
   *  terminal; the GUI never depends on its shape
   *  AND never persists it. The label is concise
   *  (just the count of top-level keys for
   *  non-empty objects) so the user can see that
   *  the workflow reported observed state without
   *  a full dump (which could leak unrelated
   *  checkout contents). Returns `null` when the
   *  terminal report does not carry an
   *  `observed_state`. */
  jobObservedStatePartial(): string | null {
    const job = this.currentJob();
    const report = job?.report;
    if (!report) return null;
    if (report.kind === "sync_agents" || report.kind === "install_skills") {
      const observed = report.observed_state;
      if (!observed) return null;
      if (typeof observed !== "object") return "scalar";
      const keys = Object.keys(observed);
      if (keys.length === 0) return "empty object";
      return `${keys.length} field(s)`;
    }
    return null;
  }

  /** Reconcile the discovered-models cache from the
   *  registry's latest snapshot. Called by the reducer
   *  on every accepted `CurrentView`. The cache is
   *  advisory only; the editor's typed `model` is never
   *  overwritten by a discovery result. There is no
   *  separate `jobPending` signal to clear — the
   *  `activeJob()` derived from `currentView().job.
   *  phase` is the source of truth, and it flips to
   *  `false` the moment a terminal snapshot is
   *  accepted by the reducer.
   *
   *  Side effects on terminal (once per job): a
   *  Settings + Agents + Tools catalog + permission
   *  keys refresh is queued ONCE per terminal job id
   *  (fail / cancel / complete all qualify) via
   *  `pendingTerminalIds`. The actual refresh runs
   *  out of `drainTerminalRefresh()` — invoked from the
   *  end of every inflight action's `finally` AND on
   *  every connection retry AND from the bootstrap
   *  after the post-`op_current` refresh. The marker
   *  is NOT consumed unless the drain actually runs
   *  to completion: a `busy()` / `startPending()` /
   *  `!mutationsEnabled()` gate keeps the marker so a
   *  later drain (after the gate flips) picks it up.
   *  There is NO periodic poll — the marker is a
   *  one-shot scheduled event, not a recurring timer.
   *  No implicit discard of the editor draft; the
   *  user's open `editDraft` / `editContext` are
   *  preserved verbatim until an explicit save /
   *  discard. */
  private pendingTerminalIds = new Set<string>();
  private terminalRefreshAttempted = new Set<string>();
  private terminalRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  private updateDiscoveredModels(view: CurrentView): void {
    const job = view.job;
    if (job?.phase !== "finished") return;
    if (job.report?.kind === "discover_models") {
      const terminal = job.report.terminal;
      if (terminal?.kind === "found") {
        this.discoveredModels.set(terminal.models.slice());
        this.discoveryError.set(null);
      } else if (terminal?.kind === "empty") {
        this.discoveredModels.set([]);
        this.discoveryError.set(terminal.message);
      } else if (terminal?.kind === "failed") {
        this.discoveredModels.set([]);
        this.discoveryError.set(terminal.message);
      }
    }
    if (!this.pendingTerminalIds.has(job.id)) {
      this.pendingTerminalIds.add(job.id);
      // Only schedule the one-shot drain when
      // mutations are enabled. When the
      // connection is unhealthy, the drain
      // would reschedule itself in a loop (the
      // previous design) — keep the marker
      // silent and let the next Retry-driven
      // `drainTerminalRefresh` consume it.
      if (this.mutationsEnabled()) {
        this.scheduleTerminalDrain();
      }
    }
  }

  /** One-shot `setTimeout(0)` scheduler. The drain
   *  itself runs synchronously off the timer; if the
   *  drain defers (busy / startPending / disabled),
   *  the marker stays so a later drain picks it up.
   *  The timer is cancelled on destroy and re-armed
   *  on the next terminal so the post-destroyed
   *  listener cannot fire a stale drain. */
  private scheduleTerminalDrain(): void {
    if (this.terminalRefreshTimer !== null) return;
    this.terminalRefreshTimer = setTimeout(() => {
      this.terminalRefreshTimer = null;
      this.drainTerminalRefresh();
    }, 0);
  }

  /** Drain the pending-terminal-id set into a single
   *  Settings + Agents + Tools catalog + permission
   *  keys refresh. Invoked from:
   *
   *  - `scheduleTerminalDrain` (right after a terminal
   *    event lands);
   *  - `finally` of every inflight action
   *    (`saveCheckout`, `saveEditor`, `openEditor`,
   *    `refreshAll`, `refreshSettings`,
   *    `refreshAgents`, `retryConnection`,
   *    `refreshToolsMetadata`);
   *  - `bootstrap` after the post-`op_current`
   *    refresh.
   *
   *  The drain is gated on `!busy() && !startPending()
   *  && mutationsEnabled()`. If any of those is false
   *  the marker stays AND a fresh drain is scheduled
   *  when the gate flips. The drain itself runs the
   *  full Settings + Agents + Tools + PermissionKeys
   *  cycle in ONE flow so a terminal-side state
   *  change is never half-refreshed. No polling,
   *  no recurring timer — the drain is purely
   *  event-driven. */
  private async drainTerminalRefresh(): Promise<void> {
    if (!this.mutationsEnabled()) {
      // Connection is unhealthy. The previous
      // design scheduled another drain here, which
      // produced a recurring setTimeout(0) loop
      // while the connection was down — a polling
      // path the directive explicitly forbade.
      // The drain is event-driven: keep the
      // markers in `pendingTerminalIds` and
      // RETURN. The next terminal event re-fires
      // `scheduleTerminalDrain` from
      // `updateDiscoveredModels`. A successful
      // Retry flips `mutationsEnabled` to `true`
      // and the next `drainTerminalRefresh`
      // (invoked from the Retry's post-recovery
      // path) picks up the markers.
      return;
    }
    if (this.busy() || this.startPending()) {
      // The inflight action's `finally` will re-arm
      // the drain via `scheduleTerminalDrain`. Just
      // wait for the next event.
      return;
    }
    if (this.pendingTerminalIds.size === 0) return;
    // Clear the markers BEFORE the actual load: a
    // drain that re-queues (because of a load-time
    // gate flip) must re-trigger from a FRESH
    // terminal event, not from a stale marker the
    // previous drain half-loaded against.
    this.pendingTerminalIds.clear();
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    this.busy.set(true);
    try {
      // Single cycle: Settings, Agents, Tool catalog,
      // Permission keys. Each sub-loader surfaces its
      // own error; the drain does NOT aggregate.
      await this.loadSettingsIntoState();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      await this.loadAgentsIntoState();
      if (myGen !== this.generation || myConn !== this.connectionGeneration) return;
      await this.refreshToolsMetadataUnlocked();
    } finally {
      this.busy.set(false);
      if (myGen !== this.generation) {
        this.pendingTerminalIds.clear();
        return;
      }
      // A terminal that landed while the drain was
      // running parked into `pendingTerminalIds` (we
      // cleared it before the load) — schedule the
      // next drain so the markers are not lost.
      if (this.pendingTerminalIds.size > 0) {
        this.scheduleTerminalDrain();
      }
    }
  }

  /** Unlocked variant of `refreshToolsMetadata` used by
   *  the post-terminal drain. The drain holds `busy`
   *  itself; calling the public `refreshToolsMetadata`
   *  would early-return on `if (this.busy()) return;`
   *  and the metadata would never refresh. The drain
   *  reuses the same loader logic (awaited
   *  `Promise.allSettled`, retain-on-failure,
   *  per-source error signal). The helper captures
   *  the live generation + connection-generation at
   *  call-time so a Retry that races the fetch
   *  (bumps the connection-generation) does NOT
   *  publish stale rows from the previous
   *  connection: the writes are guarded against
   *  the captured snapshot. */
  private async refreshToolsMetadataUnlocked(): Promise<void> {
    const myGen = this.generation;
    const myConn = this.connectionGeneration;
    const results = await Promise.allSettled([
      invoke<ToolStatusList>("tool_catalog_status"),
      invoke<PermissionKeysWire>("permission_keys"),
      this.loadPlans(),
    ]);
    if (myGen !== this.generation) return;
    if (myConn !== this.connectionGeneration) return;
    const catalogResult = results[0];
    if (catalogResult.status === "fulfilled") {
      const list = catalogResult.value;
      this.toolCatalog.set(list.rows);
      this.toolCatalogError.set(null);
      if (!this.selectedToolId() && list.rows.length > 0) {
        this.selectedToolId.set(list.rows[0].tool_id);
      }
    } else {
      this.toolCatalogError.set(this.toMessage(catalogResult.reason));
    }
    if (myGen !== this.generation) return;
    if (myConn !== this.connectionGeneration) return;
    const keysResult = results[1];
    if (keysResult.status === "fulfilled") {
      this.knownPermissionKeys.set(keysResult.value.keys);
      this.knownPermissionKeysError.set(null);
    } else {
      this.knownPermissionKeysError.set(this.toMessage(keysResult.reason));
    }
  }

  /** Normalise a Tauri rejection into a stable
   *  string. The component runs in a `vm` context
   *  under `node --test`; the harness's deferred
   *  invoke handler is defined in the OUTER realm
   *  and rejects with an `Error` whose prototype
   *  chain does NOT include the vm's `Error`
   *  class — `e instanceof Error` is `false`. The
   *  duck-typed guard below catches both the
   *  production case (Tauri runs in the same realm
   *  as the GUI) and the test case (cross-realm
   *  `Error`). */
  private toMessage(e: unknown): string {
    if (typeof e === "string") return e;
    if (
      e &&
      typeof e === "object" &&
      typeof (e as { message?: unknown }).message === "string"
    ) {
      return (e as { message: string }).message;
    }
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }

  /** Normalise an `OperationError` rejection. The lib
   *  serialises the error as JSON; the JS side parses it
   *  to a tagged enum and renders each arm differently.
   *  A rejection that does not match the wire shape
   *  falls back to the literal text. The `cancelled`
   *  arm is no longer in `OperationError` — cancel is
   *  idempotent and never an error in this slice. */
  private toOperationMessage(e: unknown): string {
    if (e && typeof e === "object" && "kind" in e) {
      const op = e as OperationError;
      switch (op.kind) {
        case "busy":
          return `another job is active (${op.active_job_id}); wait for it to settle`;
        case "unknown_job":
          return `unknown or expired job ${op.job_id}`;
        case "failed_preconditions":
          return op.message;
        case "spawn":
          return op.message;
      }
    }
    return this.toMessage(e);
  }
}

// `activeJob()` is the user-facing "we are waiting on
// a job" signal. It is derived from
// `currentView().job.phase === "running"`, so the
// moment the registry's running snapshot lands in the
// reducer (either via the `op_start` reply envelope
// or via an `agenthd-operation` event), the cancel
// button is enabled; the moment a terminal snapshot
// lands, the cancel button is disabled. There is NO
// separate "pending id" signal that could desync from
// the reducer.
