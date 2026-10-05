// spikes/tauri-angular/editor-dto.test.mjs
//
// Pure-function tests for the GUI editor's DTO helpers.
// Tests the wire-shape helpers in `src/app/editor-dto.ts`
// without booting Angular, Tauri IPC, or the DOM. The
// helpers are pure functions (deep copy + structural
// equality over a flat record), so a `node --test` harness
// is enough — no DOM, no IPC, no Angular runtime, no Tauri
// mock.
//
// Run via:
//   node --experimental-strip-types --test \
//     spikes/tauri-angular/editor-dto.test.mjs
//
// The harness imports the TS module directly. Node 22's
// `--experimental-strip-types` flag strips the type
// annotations and runs the file as ESM; this is the same
// `--experimental-strip-types` mode Node uses for its
// built-in TS support.

import { test } from "node:test";
import assert from "node:assert/strict";

import { deepCopyDto, sameDto } from "./src/app/editor-dto.ts";

// JSDoc-typed local alias so we get editor hints without a
// `type` import (the `.mjs` test file does not allow
// TypeScript syntax; the imported `.ts` module does).
/** @typedef {import("./src/app/editor-dto.ts").AgentEditDto} AgentEditDto */

/** @returns {AgentEditDto} */
function dto() {
  return {
    name: "scout",
    description: "original",
    mode: "subagent",
    model: "prov/x",
    prompt: "body",
    permissions: { bash: "ask", edit: "deny" },
  };
}

// ---------- deepCopyDto ----------

test("deepCopyDto: produces a structurally equal but independent copy", () => {
  const original = dto();
  const copy = deepCopyDto(original);
  assert.deepEqual(copy, original);
  // Mutating the copy must not touch the original.
  copy.name = "renamed";
  copy.permissions.bash = "allow";
  copy.permissions.new_key = "ask";
  assert.equal(original.name, "scout");
  assert.equal(original.permissions.bash, "ask");
  assert.equal(original.permissions.new_key, undefined);
});

test("deepCopyDto: null model stays null", () => {
  const original = dto();
  original.model = null;
  const copy = deepCopyDto(original);
  assert.equal(copy.model, null);
});

test("deepCopyDto: permissions map is a shallow-cloned dict (not aliased)", () => {
  const original = dto();
  const copy = deepCopyDto(original);
  assert.notEqual(copy.permissions, original.permissions);
});

// ---------- sameDto: identity ----------

test("sameDto: identical input equals itself", () => {
  const a = dto();
  const b = dto();
  assert.equal(sameDto(a, b), true);
});

test("sameDto: same instance equals itself", () => {
  const a = dto();
  assert.equal(sameDto(a, a), true);
});

// ---------- sameDto: field-level changes ----------

test("sameDto: name change is detected", () => {
  const a = dto();
  const b = dto();
  b.name = "renamed";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: description change is detected", () => {
  const a = dto();
  const b = dto();
  b.description = "edited";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: mode change is detected", () => {
  const a = dto();
  const b = dto();
  b.mode = "primary";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: prompt change is detected", () => {
  const a = dto();
  const b = dto();
  b.prompt = "edited body";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: model change is detected", () => {
  const a = dto();
  const b = dto();
  b.model = "prov/y";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: null vs empty-string model is treated as equal", () => {
  const a = dto();
  a.model = null;
  const b = dto();
  b.model = "";
  // The editor's onEditModel normalises the empty string to
  // null; sameDto preserves that contract so a freshly
  // cleared model field is not flagged as dirty.
  assert.equal(sameDto(a, b), true);
});

// ---------- sameDto: permissions ----------

test("sameDto: permissions map order does not affect equality", () => {
  const a = dto();
  a.permissions = { bash: "ask", edit: "deny", read: "allow" };
  const b = dto();
  b.permissions = { read: "allow", bash: "ask", edit: "deny" };
  assert.equal(sameDto(a, b), true);
});

test("sameDto: permission value change is detected", () => {
  const a = dto();
  const b = dto();
  b.permissions.bash = "allow";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: added permission key is detected", () => {
  const a = dto();
  const b = dto();
  b.permissions.new_key = "ask";
  assert.equal(sameDto(a, b), false);
});

test("sameDto: removed permission key is detected", () => {
  const a = dto();
  const b = dto();
  delete b.permissions.edit;
  assert.equal(sameDto(a, b), false);
});

test("sameDto: same key with same value but different casing is detected", () => {
  // The wire shape is string-typed, so the equality
  // comparison is exact-string. Surface that as a contract
  // pin: the editor MUST normalise casing before
  // round-tripping through the lib.
  const a = dto();
  const b = dto();
  b.permissions.bash = "Allow";
  assert.equal(sameDto(a, b), false);
});

// ---------- round-trip ----------

test("round-trip: sameDto(deepCopyDto(x), x) holds", () => {
  const a = dto();
  const b = deepCopyDto(a);
  assert.equal(sameDto(a, b), true);
});

test("round-trip: deepCopyDto preserves unknown permission keys", () => {
  const a = dto();
  a.permissions.unknown_key = "ask";
  const b = deepCopyDto(a);
  assert.equal(b.permissions.unknown_key, "ask");
  assert.equal(sameDto(a, b), true);
});

// ---------- semantic normalization pin ----------
//
// The editor normalises the model input via
// `onEditModel`: an empty / whitespace-only string becomes
// `null` (so the user typing nothing does not dirty the
// draft). The wire-shape round-trip therefore treats
// `model = null` and `model = ""` as equal — the lib
// does the same on parse. The tests below pin that
// semantic so a future refactor that compares models
// strictly (without the `?? ""` fallback) cannot silently
// flip the dirty state for the "user cleared the field"
// path.

test("semantic: null vs empty model is normalised to equal", () => {
  const a = dto();
  a.model = null;
  const b = dto();
  b.model = "";
  assert.equal(sameDto(a, b), true);
});

test("semantic: prompt whitespace is NOT normalised (strict)", () => {
  // The editor does NOT trim or normalise the prompt
  // field — every keystroke counts as a change. A user
  // who adds a trailing space is making an edit. The
  // backend's `Agent::validate` rejects whitespace-only
  // prompts at save time, but the dirty detection here
  // works on raw bytes.
  const a = dto();
  const b = dto();
  b.prompt = a.prompt + " ";
  assert.equal(sameDto(a, b), false);
});

test("semantic: model with surrounding whitespace is strict", () => {
  // The lib trims model on parse (`parse_trims_model_before_storing_it`),
  // but the editor's wire shape does NOT pre-trim. A user
  // who types `" prov/x "` (with spaces) sees a draft
  // different from `"prov/x"`. The save side re-trims on
  // render, so the lib's whitespace tolerance is the
  // backend's concern, not the frontend's dirty tracking.
  const a = dto();
  const b = dto();
  b.model = ` ${a.model} `;
  assert.equal(sameDto(a, b), false);
});