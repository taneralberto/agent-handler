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

  /**
   * Load Settings + Agents on open so the prototype
   * window is not blank. Implemented via `ngOnInit`
   * firing `refreshAll()` once; the spike still does
   * not poll — refresh is also available manually via
   * the toolbar buttons, because the underlying
   * configuration lives in the TUI, which the user has
   * to switch back to.
   */
  ngOnInit(): void {
    void this.refreshAll();
  }

  /**
   * Refresh Settings by invoking the read-only
   * `settings_status` command. Manual — the spike
   * intentionally has no polling because the
   * underlying configuration lives in the TUI, which
   * the user has to switch back to. (Initial load on
   * open is fired once from `ngOnInit`.)
   */
  async refreshSettings(): Promise<void> {
    this.settingsLoading.set(true);
    this.settingsError.set(null);
    try {
      const response = await invoke<SettingsResponse>("settings_status");
      this.settings.set(response);
    } catch (e) {
      this.settingsError.set(this.toMessage(e));
      this.settings.set(null);
    } finally {
      this.settingsLoading.set(false);
    }
  }

  /**
   * Refresh Agents by invoking the read-only
   * `list_agents` command. Same manual-refresh policy
   * as Settings — the spike does not poll. (Initial
   * load on open is fired once from `ngOnInit`.)
   */
  async refreshAgents(): Promise<void> {
    this.agentsLoading.set(true);
    this.agentsError.set(null);
    try {
      const response = await invoke<AgentsList>("list_agents");
      this.agents.set(response.agents);
      this.agentsError.set(response.error);
    } catch (e) {
      this.agentsError.set(this.toMessage(e));
      this.agents.set([]);
    } finally {
      this.agentsLoading.set(false);
    }
  }

  /**
   * Convenience for the toolbar button: refresh both
   * screens in sequence. Each panel keeps its own
   * loading flag so a slow list_agents does not block
   * the settings view.
   */
  async refreshAll(): Promise<void> {
    await this.refreshSettings();
    await this.refreshAgents();
  }

  /**
   * Settings status display label. Mirrors the four
   * arms the Rust command returns; the rendering is
   * deliberately close to the TUI's Settings screen so
   * the spike is easy to compare side-by-side.
   */
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

  /**
   * Per-status details (key/value pairs) for the
   * Settings panel. Empty for "Empty" — the TUI
   * surfaces the same shape (a banner asking the user
   * to pick a checkout).
   */
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

  /**
   * Tauri rejects `null` rejections from JS by
   * surfacing an empty `Error`, which renders as
   * `[object Object]` here. Normalise to a stable
   * string so the panel does not show garbage.
   */
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
