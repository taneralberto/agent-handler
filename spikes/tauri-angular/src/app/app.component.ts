import { Component, OnInit, signal } from "@angular/core";
import { invoke } from "@tauri-apps/api/core";

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

@Component({
  selector: "app-root",
  templateUrl: "./app.component.html",
  styleUrl: "./app.component.css",
})
export class AppComponent implements OnInit {
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

  // Serialization flag for every public action (refresh + save).
  // Public so the template can bind `[disabled]` directly. The
  // flag prevents overlap of public actions while one is in
  // flight; it is not a global consistency guarantee against
  // external writers. `loadSettings` / `loadAgents` are private
  // and bypass it.
  readonly busy = signal(false);

  /** True once the draft buffer has been seeded or the user typed. */
  private draftSeeded = false;

  ngOnInit(): void {
    void this.refreshAll();
  }

  /** Refresh Settings via the read-only `settings_status` command. */
  async refreshSettings(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.loadSettingsIntoState();
    } finally {
      this.busy.set(false);
    }
  }

  /** Refresh Agents via the read-only `list_agents` command. */
  async refreshAgents(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      await this.loadAgentsIntoState();
    } finally {
      this.busy.set(false);
    }
  }

  /** Refresh both panels under a single busy window. */
  async refreshAll(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      // Sequential so the busy window is identical regardless
      // of whether the loaders share state.
      await this.loadSettingsIntoState();
      await this.loadAgentsIntoState();
    } finally {
      this.busy.set(false);
    }
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
   */
  async saveCheckout(): Promise<void> {
    if (this.busy()) return;
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
      this.saveError.set(this.toMessage(e));
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
      }
    }
  }

  /** Native input binding (no FormsModule). */
  onCheckoutPathInput(value: string): void {
    this.draftSeeded = true;
    this.checkoutPath.set(value);
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

  /** Fetch Settings, update signals, seed the draft if first time. */
  private async loadSettingsIntoState(): Promise<void> {
    this.settingsLoading.set(true);
    this.settingsError.set(null);
    try {
      const response = await this.loadSettings();
      this.settings.set(response);
      this.seedDraftFrom(response);
    } catch (e) {
      this.settingsError.set(this.toMessage(e));
      this.settings.set(null);
    } finally {
      this.settingsLoading.set(false);
    }
  }

  /** Fetch Agents, update signals. */
  private async loadAgentsIntoState(): Promise<void> {
    this.agentsLoading.set(true);
    this.agentsError.set(null);
    try {
      const list = await this.loadAgents();
      this.agents.set(list.agents);
      this.agentsError.set(list.error);
    } catch (e) {
      this.agentsError.set(this.toMessage(e));
      this.agents.set([]);
    } finally {
      this.agentsLoading.set(false);
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

  /** Normalise a Tauri rejection into a stable string. */
  private toMessage(e: unknown): string {
    if (typeof e === "string") return e;
    if (e instanceof Error) return e.message;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }
}
