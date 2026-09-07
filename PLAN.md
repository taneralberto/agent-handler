# Implementation Plan

## Goal

Build a minimal Rust TUI binary named `agenthd` that:

- Stores canonical OpenCode agent definitions in the user config directory.
- Seeds editable `scout`, `reviewer`, and `worker` starter agents.
- Manages additional user-created agents.
- Edits name, description, mode, model, prompt, and non-deprecated permissions.
- Synchronizes owned definitions safely to `~/.config/opencode/agents/`.
- Preserves unowned or externally modified target files by default.
- Shows whether each agent needs installation, update, conflict resolution, or removal.

The main menu is exactly: **Agents**, **Install/Update**, **Exit**.

## Context

- `/home/lukateric/dev/agent-handler` is currently empty; there is no existing application architecture or test convention to preserve.
- Local OpenCode config exists at `/home/lukateric/.config/opencode/opencode.jsonc` and currently contains only the OpenCode schema reference.
- `/home/lukateric/.config/opencode/agents/` does not currently exist.
- The locally installed OpenCode SDK and logs identify OpenCode version `1.17.4`.
- Local SDK evidence at `/home/lukateric/.config/opencode/node_modules/@opencode-ai/sdk/dist/v2/gen/types.gen.d.ts` confirms:
  - Agent modes are `subagent`, `primary`, and `all`.
  - Models use `provider/model` identifiers.
  - The supported permission actions are `allow`, `ask`, and `deny`.
  - Agent configuration uses singular `permission`; legacy `tools` exists but must not be emitted.
  - Current permission names include `read`, `edit`, `glob`, `grep`, `list`, `bash`, `task`, `external_directory`, `todowrite`, `question`, `webfetch`, `websearch`, `lsp`, `doom_loop`, and `skill`.
- Local OpenCode state contains recent models in `provider/model` form, corroborating the required model representation.
- No database, daemon, agent runner, project-local synchronization, plugin interface, remote catalog, or agent-version model is needed.

### Preventive KISS design

- Canonical agents are ordinary OpenCode-compatible Markdown files, not records duplicated into another storage format.
- A single `Agent` type is the source of truth for parsing, editing, validation, and rendering.
- One static permission-name list drives the permission editor.
- One sync planner computes all install states; rendering and application consume that same plan.
- Adding another starter agent requires one entry in the starter list. No per-agent screen, loader, installer, or subtype is introduced.
- Generic behavior: all agent CRUD, frontmatter handling, status calculation, installation, and rendering.
- Specific behavior: only the three starter definitions differ by constants.
- Do not create repositories, managers, service traits, factories, plugins, databases, or a generic form framework.

## Decisions and Assumptions

### Paths

Honor the standard Linux config root:

1. Use `$XDG_CONFIG_HOME` when it is non-empty.
2. Otherwise require `$HOME` and use `$HOME/.config`.

Derived paths:

- Canonical definitions: `<config-root>/agenthd/agents/*.md`
- Ownership state: `<config-root>/agenthd/state.json`
- OpenCode target: `<config-root>/opencode/agents/*.md`

This produces the requested paths under `~/.config` in the normal environment while permitting isolated validation with a temporary `XDG_CONFIG_HOME`.

### Canonical format

Each canonical file is already valid OpenCode Markdown and is copied byte-for-byte during synchronization:

```markdown
---
description: Reviews a completed implementation for correctness and regressions.
mode: subagent
model: openai/gpt-5.4
permission:
  bash: ask
  edit: deny
  webfetch: allow
---

Review completed changes for correctness...
```

Rules:

- The agent name comes from the `.md` filename and is not duplicated in frontmatter.
- Frontmatter contains `description`, `mode`, optional `model`, and optional `permission`.
- The body is the prompt.
- Never emit `tools`.
- YAML serialization handles quoting and multiline-safe scalar values.
- Unknown frontmatter fields are rejected rather than silently discarded on a later edit.
- Nested/pattern-specific permission objects are outside this MVP; permissions are flat tool-to-action entries.
- Synchronization copies the canonical bytes rather than re-rendering them, avoiding formatting churn after manual canonical edits.

### Data model

In `src/agent.rs`:

```rust
struct Agent {
    name: String,
    description: String,
    mode: Mode,
    model: Option<String>,
    prompt: String,
    permissions: BTreeMap<String, PermissionAction>,
}

enum Mode {
    Subagent,
    Primary,
    All,
}

enum PermissionAction {
    Allow,
    Ask,
    Deny,
}
```

The serialized frontmatter has a `permission` field mapped from `Agent.permissions`.

Validation:

- Name must be lowercase kebab-case: ASCII lowercase letters or digits separated by single hyphens.
- Name cannot be empty, begin/end with `-`, contain `..`, separators, whitespace, or control characters.
- Description and prompt must be nonblank.
- Model is either absent, meaning “inherit OpenCode default,” or a non-whitespace `provider/model` identifier with non-empty provider and model portions.
- Permission names are selected from the one supported permission list.
- A rename cannot overwrite an existing canonical filename.
- Canonical files must be regular UTF-8 `.md` files; symlinks and special files are refused.

### Starter definitions

Seed these only once, without overwriting an existing same-named canonical file. After seeding they are normal editable/deletable agents.

- **scout**
  - Description: `Quickly inspects a codebase and reports focused findings for another agent.`
  - Mode: `subagent`
  - Model: inherit
  - Prompt: `You are a read-only codebase scout. Inspect only what is needed, trace the relevant flow, and report concrete findings with file paths, line ranges, dependencies, and risks. Do not edit files.`
  - Permissions: `read=allow`, `glob=allow`, `grep=allow`, `list=allow`, `bash=ask`, `edit=deny`, `task=deny`, `external_directory=ask`, `webfetch=allow`.

- **reviewer**
  - Description: `Reviews completed changes for correctness, regressions, security, and missing validation.`
  - Mode: `subagent`
  - Model: inherit
  - Prompt: `Review completed changes for correctness, regressions, security, and missing tests. Report findings in severity order with file and line references. Do not edit files; say explicitly when no issues are found.`
  - Permissions: `read=allow`, `glob=allow`, `grep=allow`, `list=allow`, `bash=ask`, `edit=deny`, `task=deny`, `external_directory=ask`, `webfetch=allow`.

- **worker**
  - Description: `Implements an assigned change and validates the result.`
  - Mode: `subagent`
  - Model: inherit
  - Prompt: `Implement the assigned change directly in the current workspace. Follow the approved plan and existing conventions, keep the diff small, run focused validation, and report changed files, tests, and remaining risks.`
  - Permissions: `read=allow`, `glob=allow`, `grep=allow`, `list=allow`, `edit=allow`, `bash=ask`, `task=deny`, `external_directory=ask`, `webfetch=allow`.

A newly created user agent defaults to `subagent`, inherited model, and `edit=ask`, `bash=ask`, `external_directory=ask`.

### Ownership and update state

`state.json` is a small safety manifest, not a database or agent-versioning system:

```json
{
  "starters_seeded": true,
  "installed": {
    "scout.md": "<sha256-of-last-bytes-written>"
  }
}
```

For each canonical/target/manifest combination, compute:

- **Not installed**: source exists, target absent.
- **Up to date**: source and target bytes match.
- **Update available**: source differs, but target hash equals agenthd’s last-installed hash.
- **Conflict**: source differs and target is unowned or has changed since agenthd installed it.
- **Remove**: source was deleted and the target still equals agenthd’s last-installed bytes.
- **Preserve modified**: source was deleted but the formerly owned target was externally modified.
- **Unowned**: target-only file has no manifest ownership record.

Safe Install/Update:

- Creates **Not installed** files.
- Records/adopts identical **Up to date** files without rewriting them.
- Replaces **Update available** files.
- Removes **Remove** files.
- Leaves **Conflict** and **Unowned** files untouched.
- Leaves **Preserve modified** files untouched and releases their stale ownership entry.
- Cleans manifest entries whose source and target are both absent.

A selected active conflict may be overwritten only after a path-specific confirmation. Modified orphan targets are never deleted by the MVP; users must resolve those manually.

### Atomicity

Use a single standard-library helper for canonical files, target files, and `state.json`:

1. Create a uniquely named temporary sibling using `create_new`.
2. Write all bytes.
3. Flush and `sync_all`.
4. Rename within the same directory.
5. Remove the temporary file on failure.

The process ID plus a counter is sufficient for temporary names. Same-directory rename supplies atomic visibility on the requested Linux target. Source deletion uses `remove_file`, which atomically removes the directory entry.

### Model discovery

- Spawn `opencode models` directly, without a shell.
- Set `NO_COLOR=1`.
- Read successful stdout as one candidate per nonblank line.
- Accept candidate lines only when they are valid `provider/model` identifiers.
- Sort and deduplicate.
- Show stderr or launch failure in the picker’s status area.
- Manual entry remains available whether discovery succeeds, returns no candidates, or fails.
- Run discovery when opening the picker; `r` retries. No async process framework is needed for the MVP.

## Implementation

1. **Materialize the approved design and scaffold the binary**
   - Files: `PLAN.md`, `Cargo.toml`, `Cargo.lock`, `.gitignore`, `README.md`
   - Change:
     - Put this approved plan in `PLAN.md`.
     - Create a Rust binary package named `agenthd`.
     - Use a small dependency set:
       - `anyhow`
       - `crossterm`
       - `ratatui`
       - `serde` with derive
       - `serde_json`
       - `serde_yaml`
       - `sha2`
       - `tui-textarea`
       - `tempfile` only as a dev dependency
     - Do not add `clap`; the MVP has no command-line options.
     - Ignore `/target`.
     - Document `cargo install --path .`, config paths, keybindings, sync safeguards, and MVP exclusions.
   - Reason: Establishes the executable and records the agreed scope without unnecessary architecture.
   - Validation:
     - `cargo metadata --no-deps` identifies one binary named `agenthd`.
     - README paths and behavior match the implementation.
     - `Cargo.lock` is generated and committed for the application.

2. **Implement the agent format and starter source of truth**
   - Files: `src/agent.rs`
   - Change:
     - Add `Agent`, `Mode`, `PermissionAction`, strict frontmatter DTO, `PERMISSION_KEYS`, and the three starter entries.
     - Implement filename/name validation, model validation, Markdown parsing, and deterministic Markdown rendering.
     - Parse only a leading `---` frontmatter block with a standalone closing `---`.
     - Serialize singular `permission` and never serialize `tools`.
     - Preserve prompt contents when parsing; renderer emits a stable final newline.
     - Reject malformed YAML, unsupported fields, invalid modes/actions, invalid filenames, missing frontmatter, and empty required fields with file-specific errors.
   - Reason: Keeps canonical representation, validation, and UI data in one place.
   - Validation:
     - Exact-output test asserts OpenCode Markdown structure and absence of `tools:`.
     - Parse/render round-trip test covers all three modes, optional model, YAML-sensitive description text, multiline prompt, and every permission action.
     - Invalid-name/model/frontmatter tests fail with actionable messages.
     - Adding a fourth starter would require only one starter-list entry.

3. **Implement canonical storage, first-run seeding, and atomic writes**
   - Files: `src/store.rs`
   - Change:
     - Add `Paths`, `State`, path resolution, canonical loading, `save_agent`, `delete_agent`, `seed_starters`, state loading, and the atomic sibling-write helper.
     - Resolve paths from explicit environment values passed into a pure path helper, keeping path tests independent of process-global environment mutation.
     - Seed missing starter files only while `starters_seeded` is false; never replace any existing path.
     - Mark seeding complete only after every starter is either created or already present as a regular file.
     - Save same-name edits atomically.
     - For rename, validate destination absence, atomically write the new file, then remove the old file; if removal fails, attempt to remove the new file and report the failure.
     - Deleting from Agents removes only the canonical file. Target removal waits for Install/Update.
     - Refuse symlinks, directories, special files, and invalid UTF-8 canonical definitions.
     - Treat malformed `state.json` as a fail-closed install error while still allowing canonical agents to be viewed.
   - Reason: Makes agenthd the canonical source without risking pre-existing files.
   - Validation:
     - Temporary-directory tests cover first-run seeding, repeated idempotent seeding, preservation of a pre-existing `scout.md`, CRUD, rename collision, and atomic replacement.
     - Verify failed validation does not change the previous canonical bytes.

4. **Implement one sync planner and guarded installer**
   - Files: `src/store.rs`
   - Change:
     - Define `SyncItem` and `SyncStatus` for the seven states above.
     - Hash raw bytes with SHA-256.
     - Scan canonical and target `.md` files plus manifest entries into one deterministic filename-sorted plan.
     - Ignore non-`.md` target files and preserve them.
     - Block all installation if any canonical agent cannot be parsed; do not silently perform a partial deployment from an invalid canonical set.
     - Implement:
       - `apply_safe(plan)` for non-conflicting actions.
       - `force_install(name)` only for a selected source-present conflict after UI confirmation.
     - Refuse to replace any non-regular target, even through force install.
     - Update `state.json` atomically after applied operations. If a target operation fails, retain its prior manifest entry, continue only where safe, and return a per-file summary.
     - If the state write fails after target writes, report the error prominently; the next plan can safely recover identical source/target files as adoptable up-to-date entries.
   - Reason: Centralizes ownership and conflict behavior, preserving unowned agents by default.
   - Validation:
     - A table-driven temporary-directory test covers:
       - fresh install,
       - no-op/up-to-date,
       - safe update,
       - unowned collision,
       - externally modified owned collision,
       - safe removal,
       - modified orphan preservation and ownership release,
       - unowned target-only preservation,
       - stale manifest cleanup,
       - state recovery when source and target already match.
     - Assert conflict tests leave target bytes unchanged.
     - Assert safe update/removal occurs only when the target hash matches the recorded installed hash.

5. **Implement model discovery with manual fallback**
   - Files: `src/models.rs`
   - Change:
     - Add `discover_models()` and a separately testable `parse_models(stdout)`.
     - Execute `opencode models` directly with `NO_COLOR=1`.
     - Return sorted, deduplicated valid identifiers.
     - Preserve launch, nonzero-exit, stderr, and empty-result information for display.
     - Keep manual model validation in `agent.rs` so discovered and manually entered values follow one rule.
   - Reason: Meets model selection requirements without an SDK integration or background worker.
   - Validation:
     - Parser tests cover duplicate IDs, blank lines, malformed output, warning-like lines, and provider/model IDs containing punctuation.
     - Manual entry remains reachable in all discovery-result states.

6. **Implement the TUI and terminal safety**
   - Files: `src/app.rs`, `src/main.rs`
   - Change:
     - `main.rs` resolves paths, seeds starters, loads the application, enters raw/alternate-screen mode, and runs the event loop.
     - Add an RAII terminal guard whose `Drop` disables raw mode, leaves alternate screen, and restores the cursor on normal return or error.
     - Keep application state and rendering together in `app.rs`; do not introduce a UI framework layer.

     Main menu interactions:
     - `Up/Down` or `j/k`: move.
     - `Enter`: open selected item.
     - `q` or `Esc`: exit.

     Agents screen:
     - Sorted agent list with description, mode, and model/inherited-model summary.
     - `n`: create.
     - `Enter` or `e`: edit selected.
     - `d`: confirmation modal, then delete canonical definition only.
     - `Esc`: back.

     Agent editor:
     - Fields: Name, Description, Mode, Model, Prompt, Permissions.
     - `Tab`/`BackTab`: move fields.
     - `Left/Right` on Mode: cycle `subagent`, `primary`, `all`.
     - `Enter` on Model: open model picker.
     - Prompt uses `tui-textarea` for multiline editing.
     - Permissions show every `PERMISSION_KEYS` entry; `Space` cycles `inherit → allow → ask → deny → inherit`.
     - `Ctrl+S`: validate and save.
     - `Esc`: return immediately if clean, otherwise confirm discard.
     - Validation and I/O failures leave editor contents intact.

     Model picker:
     - `Up/Down`: select candidate.
     - `Enter`: apply candidate.
     - `m`: manual-entry modal; blank means inherit.
     - `r`: rerun discovery.
     - `Esc`: cancel.
     - Discovery errors are visible but do not block manual entry.

     Install/Update screen:
     - Show filename, status, and concise reason/path.
     - Include preserved target-only files so users can see that they are intentionally untouched.
     - `i`: apply all safe actions, then recompute statuses and show counts.
     - `o`: overwrite only the selected source-present conflict after a confirmation naming the exact target.
     - `r`: refresh from disk.
     - `Esc`: back.
     - No key deletes a modified orphan or unowned target.

     Global error behavior:
     - Display recoverable errors in a status bar/modal.
     - Attach path and operation context to filesystem errors.
     - Do not panic for malformed config, missing `opencode`, empty model output, terminal resize, or an agent changed externally while the TUI is open.
     - Reload/recompute immediately before save, delete, safe install, or force overwrite where stale state could cause data loss.
   - Reason: Provides the requested workflow while keeping the state machine direct and auditable.
   - Validation:
     - Unit-test pure navigation/status transitions where practical.
     - Manually verify every key path, dirty-editor confirmation, terminal resize, and terminal restoration after both normal exit and injected load error.

7. **Complete focused compatibility and quality validation**
   - Files: all implementation files and documentation
   - Change:
     - Resolve compiler/clippy findings without adding abstraction.
     - Verify generated definitions against the installed OpenCode CLI in an isolated config root.
     - Ensure documentation reflects actual keys and safeguards.
   - Reason: The sync and file-format boundaries need runnable evidence before real user configuration is touched.
   - Validation:
     - Run:
       - `cargo fmt --check`
       - `cargo clippy --all-targets --all-features -- -D warnings`
       - `cargo test`
       - `cargo build --release`
     - In a temporary directory:
       1. Set `XDG_CONFIG_HOME` to the temporary path.
       2. Run `cargo run`.
       3. Inspect/edit seeded agents.
       4. Apply Install/Update.
       5. Verify generated files exist under the temporary `opencode/agents`.
       6. Run `opencode debug config` and confirm the generated agents load with expected modes, models, prompts, and permissions.
       7. Add an unrelated target agent and verify another sync leaves it byte-identical.
       8. Modify an owned target and verify it becomes Conflict and safe sync leaves it byte-identical.
       9. Update a canonical agent with an unchanged owned target and verify safe update succeeds.
     - Run `opencode models` in the real environment read-only and verify at least representative output is parsed; separately verify manual fallback with an unavailable command.

## Files to Modify

The repository is empty, so there are no existing files to modify.

## New Files

- `PLAN.md` — approved implementation plan and scope boundary.
- `Cargo.toml` — binary package and minimal dependencies.
- `Cargo.lock` — reproducible application dependency resolution.
- `.gitignore` — ignores Rust build output.
- `README.md` — installation, paths, keybindings, safeguards, and exclusions.
- `src/main.rs` — process startup, config paths, terminal lifecycle, and event loop.
- `src/agent.rs` — agent model, validation, frontmatter parsing/rendering, permissions, and starters.
- `src/store.rs` — canonical storage, manifest, atomic writes, sync planning, and installation.
- `src/models.rs` — `opencode models` discovery and parsing.
- `src/app.rs` — TUI state, interactions, modals, and rendering.

Tests should remain as focused `#[cfg(test)]` modules beside the tested code; do not create a separate test hierarchy unless implementation constraints require it.

## Validation

Required automated checks:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
- `cargo build --release`

Required behavior coverage:

- Exact OpenCode Markdown output using `permission`, never `tools`.
- Strict agent/name/model validation.
- Seed idempotence and preservation of existing canonical files.
- Atomic canonical and target writes.
- Every ownership/status transition.
- No overwrite of unowned or externally modified targets during safe sync.
- Safe removal only for unchanged agenthd-owned targets.
- Preservation and ownership release for modified orphan targets.
- Model output parsing and unconditional manual fallback.
- Dirty edit/delete/overwrite confirmations.
- Terminal restoration on normal and error exits.
- Isolated OpenCode compatibility check through `opencode debug config`.

## Risks

- **High — external format compatibility:** The local SDK confirms the fields and permission vocabulary, but Markdown loading must still be tested against the installed OpenCode CLI. Do not write to the real target until isolated `opencode debug config` validation succeeds.
- **High — overwrite safety:** A corrupt ownership manifest makes prior ownership unknowable. Installation must fail closed; the user may move/delete the bad manifest, after which collisions are treated as unowned and require explicit overwrite.
- **Medium — CLI output drift:** `opencode models` may change its output format. Invalid lines must be ignored and manual entry must remain usable.
- **Medium — filesystem races:** Another process can change a target between status display and installation. Re-read and re-hash immediately before every mutation; turn a mismatch into Conflict.
- **Medium — symlinks/special files:** Replacing these could affect unexpected locations. Refuse them and require manual resolution.
- **Low — interrupted multi-file sync:** Individual files and the manifest are atomic, but the whole batch is not transactional. Recomputed hashes and adoptable identical targets make reruns safe.
- **Low — blocking model discovery:** `opencode models` runs synchronously and may briefly pause the picker. Accept this for MVP; add process timeout/background handling only if observed latency is materially disruptive.

### Stop or escalation conditions

Stop the affected implementation and escalate rather than guessing if:

- The installed OpenCode CLI rejects the planned Markdown `permission` structure or does not load user agents from `<config-root>/opencode/agents`.
- `opencode models` does not provide line-oriented `provider/model` identifiers and no stable documented output mode is available.
- A supported OpenCode version requires ordered or nested permission rules to express basic tool actions; that would materially change the data model and editor.
- Atomic same-directory rename is unavailable on the actual supported target platform.
- Product requirements expand to importing/editing unowned target files, project-local agents, preserving unknown frontmatter, nested command permission patterns, batch force-overwrite, or modified-orphan deletion.
- A real target path is a symlink, directory, or special file. The MVP must report and preserve it, not attempt a clever replacement.

## Out of Scope

- Running agents or prompts.
- Importing OpenCode agents into agenthd.
- Agent history, revisions, rollback, or version comparison beyond the last-installed safety hash.
- Database or daemon.
- Background file watching.
- Remote agent catalog.
- Project-local targets.
- Plugin or provider framework.
- Nested/path/command-pattern permission rules.
- Editing arbitrary unknown OpenCode frontmatter.
- Binary self-update or OpenCode installation.
- Backups after an explicitly confirmed conflict overwrite.
- Packaging beyond `cargo install --path .`.
- Mouse interaction, themes, localization, search, sorting controls, or elaborate form abstractions.