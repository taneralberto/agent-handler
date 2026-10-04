/**
 * Pure helpers for the GUI editor's DTO. Kept separate from
 * `app.component.ts` so they can be exercised under
 * `node --test` without booting Angular, Tauri IPC, or the
 * DOM. The component imports these as plain functions; the
 * test harness imports them by file path. There is no DOM or
 * IPC dependency here on purpose — the helpers operate on
 * the wire shape only.
 *
 * The wire shape mirrors `AgentEditDto` in
 * `spikes/tauri-angular/src-tauri/src/lib.rs`.
 */

export interface AgentEditDto {
  name: string;
  description: string;
  mode: string;
  model: string | null;
  prompt: string;
  permissions: Record<string, string>;
}

/** Both nullable fields are null for New, or both strings for an existing agent. */
export type AgentEditContext = { checkout_path: string } & (
  | { original_name: null; prior_hash: null }
  | { original_name: string; prior_hash: string }
);

/** Deep-copy a DTO so a snapshot signal stays independent of
 *  subsequent draft mutations. JSON round-trip is enough
 *  here — the DTO is a flat record of strings + a primitive
 *  map; no closures, no nested objects. */
export function deepCopyDto(dto: AgentEditDto): AgentEditDto {
  return {
    name: dto.name,
    description: dto.description,
    mode: dto.mode,
    model: dto.model === null ? null : dto.model,
    prompt: dto.prompt,
    permissions: { ...dto.permissions },
  };
}

/** Structural equality for the DTO + permissions. Used by
 *  the editor to detect dirty state without a round-trip
 *  through the backend. Pure function — does not depend on
 *  signal state. Permissions are compared as sorted
 *  key/value pairs so order in the map does not affect the
 *  result.
 *
 *  Semantic normalization pin: `model = null` and
 *  `model = ""` are treated as equal so the editor's
 *  `onEditModel` can normalize an empty input to `null`
 *  without dirtying the draft. All other fields (prompt,
 *  description, mode, permissions values) are strict —
 *  whitespace inside `prompt` is a real change. The lib
 *  trims model on parse, but the frontend's dirty
 *  tracking works on raw wire bytes; the backend
 *  re-normalises at save time. The test harness asserts
 *  both halves of this contract. */
export function sameDto(a: AgentEditDto, b: AgentEditDto): boolean {
  if (a.name !== b.name) return false;
  if (a.description !== b.description) return false;
  if (a.mode !== b.mode) return false;
  if ((a.model ?? "") !== (b.model ?? "")) return false;
  if (a.prompt !== b.prompt) return false;
  const ak = Object.keys(a.permissions).sort();
  const bk = Object.keys(b.permissions).sort();
  if (ak.length !== bk.length) return false;
  for (let i = 0; i < ak.length; i++) {
    if (ak[i] !== bk[i]) return false;
    if (a.permissions[ak[i]] !== b.permissions[bk[i]]) return false;
  }
  return true;
}
