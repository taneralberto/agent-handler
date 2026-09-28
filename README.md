# agenthd

`agenthd` is a small terminal UI that keeps your agent definitions under your
own control. The canonical sources live in a single user-configured checkout
on disk, and `Install/Update` safely synchronizes them into the directories
OpenCode and Pi read agents from.

## Source

There is exactly one canonical source of truth: the `agents/` directory
inside a checkout you point `agenthd` at. There is no Local mode and no
Compare mode — `agenthd` reads and writes agent definitions only inside the
configured checkout.

### First run

On first launch (no `settings.json`), the TUI opens a **gated** Settings
screen. The user must pick a checkout path before the rest of the menu
is reachable. The picker prefills the path field from a cwd-anchored
ancestor walk that locates the nearest directory with an `agents/` child
(only when no settings file is present). The prefill is a hint only: the
picker never auto-confirms, and the user can press Enter without typing
to accept it as-is. The first printable keystroke **replaces** the
prefill rather than appending to it (appending would produce an invalid
path the user would have to clear by hand); subsequent keystrokes
append normally. `Backspace` deletes the previous character and
`Ctrl+U` clears the buffer in one keystroke.

### Settings

Open **Settings** from the main menu to change the configured checkout
path at any time. The path is validated against the same contract used
at startup:

- the path must be absolute;
- it must exist and be a directory;
- it must contain an `agents/` directory (empty is allowed);
- it must not be a symlink.

A missing or moved checkout is an explicit error, never a silent
fallback to a different checkout. An invalid path is repairable through
the Settings screen or with `--repo <absolute>` on the next launch.

### `--repo` CLI override

```sh
agenthd --repo <abs-path>   # point at a checkout (validated up front)
```

`--repo` requires an absolute path. The path is validated up front so
the user sees a clear error if the checkout is unusable. When the path
is valid it is persisted to `settings.json` so subsequent launches
agree with the user.

### Empty checkout

An empty `agents/` directory is intentional: a freshly cloned checkout
with no `.md` files produces an empty Agents list, not an error and not
a seed step. The runtime surfaces that as "no agents" through the
normal empty-list path.

`agenthd` does **not** interact with Git on your behalf: no clone, no
fetch, no commit, no push. Bring your own checkout.

## Bundled starter roles

`agenthd` ships eight starter roles as tracked Markdown files at the
repository root under `agents/*.md`: seven subagents and one primary
coordinator. The .md files are the parseable source-of-truth; tests
include their bytes via `include_str!` so the fixtures and the repo's
tracked markdown cannot drift apart. The production binary carries
zero starter bytes — every starter you see at runtime comes from the
configured checkout, and `agents/*.md` is the only place starter
prompts live.

| Role | Purpose | Edits? |
| --- | --- | --- |
| `lukateric` | Primary coordinator; delegates bounded work and keeps user intent, decisions, and acceptance in one place. | Yes |
| `scout` | Read-only codebase recon; targeted findings and risks. | Yes |
| `oracle` | Read-only advisor; surface drift, contradictions, narrowest next move. | No |
| `planner` | Read-only implementation planner; concrete, ordered plans with validation and risk discipline. | No |
| `reviewer` | Read-only change review; severity-ordered findings with file/line evidence. | No |
| `researcher` | Read-only web research; concise, well-sourced brief. | No |
| `delegate` | Lightweight implementation agent that executes an assigned task directly. | Yes |
| `worker` | Default implementation agent with plan-aware validation. | Yes |

`lukateric` is `mode: primary`; the other seven are `mode: subagent`.
Each starter's frontmatter pins a model and per-permission actions;
Pi receives equivalent Pi subagent definitions with the appropriate
Pi tool allowlists.

The canonical files in the configured checkout are yours: edit them
freely. `Install/Update` updates every owned target safely — sync
actions for files that match an entry in the ownership manifest refresh
the destination in place, while files that already exist at the target
but are *not* owned by `agenthd` (or whose bytes no longer match the
stored hash) are presented as conflicts and require explicit `Y` to
overwrite. Untracked roles still get installed. The first-run / recovery
Settings screen never seeds the configured checkout: there is no clone,
fetch, or bootstrap step.

## Install

```sh
cargo install --path . --locked
```

Update with `cargo install --path . --locked --force`, remove with
`cargo uninstall agenthd`. The binary installs to `~/.cargo/bin`, which
is typically on `PATH` already via `~/.cargo/env`; add it to your
shell's `PATH` if it is not.

## Paths

| Purpose | Path |
| --- | --- |
| Settings (configured checkout path) | `$HOME/.agenthd/settings.json` |
| Ownership manifest | `$HOME/.agenthd/state.json` |
| Canonical agents | `<configured-checkout>/agents/*.md` |
| OpenCode agents | `$XDG_CONFIG_HOME/opencode/agents/*.md` (falls back to `$HOME/.config/opencode/agents/*.md`) |
| OpenCode skills | `$XDG_CONFIG_HOME/opencode/skills/<name>/` (falls back to `$HOME/.config/opencode/skills/<name>/`) |
| Pi agents | `$HOME/.pi/agent/agents/*.md` |

`agenthd`-owned state lives under `$HOME/.agenthd`, independent of
`XDG_CONFIG_HOME`. The OpenCode output directory follows the XDG layout;
Pi reads its global agents from `$HOME/.pi/agent/agents`. The
configured checkout lives wherever the user points `settings.json`;
`agenthd` reads from and writes to it without copy or staging.

## Source layout

The paths above are resolved and read/written by a small store module
shared across the app.

- `src/store/mod.rs` — path resolution, the ownership manifest,
  atomic temp+rename, parsing, and shared SHA-256 helper.
- `src/store/settings.rs` — per-machine settings (the configured
  checkout path), `validate_checkout_path`, and the cwd-anchored hint
  resolver used by the first-run picker.
- `src/store/canonical.rs` — load/save of the canonical agents (source
  of truth) under the configured checkout.
- `src/store/sync.rs` — plan/apply for the OpenCode and Pi targets
  driven by `Install/Update`.
- `src/store/tests.rs` — store unit tests.
- `src/agent/starter_fixture/` — `#[cfg(test)]` test-only fixture that
  parses `agents/*.md` for unit tests. The production binary does not
  link this module; the runtime reads starter bytes from the configured
  checkout.

## Keys

Main menu: `Up/Down` or `j/k`, `Enter`, `q` / `Esc` exit.

Settings (first-run; gated): `Up/Down` or `j/k`, `Enter` to open the
path editor. The path editor accepts printable characters, `Backspace`,
`Ctrl+U` to clear, `Enter` to validate + apply (and write
`settings.json`), `Esc` to cancel.

Settings (menu entry; non-gated): same path editor keys, plus `Esc` to
return to the Workspace menu.

Agents: `n` create, `Enter`/`e` edit, `d` delete (confirm), `Esc` back.

Editor (Vim-style): starts in `NORMAL`. The current mode is shown
above the fields as `[ NORMAL ]` or `[ INSERT ]`.

`NORMAL`:

- `Up`/`Down` or `j`/`k`: move between fields (including stepping
  through the permission list via the existing field helpers).
- `Left`/`Right` or `h`/`l`: on Mode, cycle `subagent → primary → all`;
  on the permission list, step between rows; on the other fields,
  no-op.
- `i`: enter `INSERT` on a text field (Name, Description, Prompt).
  No-op on Mode, Model, and the permission list.
- `Enter` on Model: open the model picker.
- `e` on Prompt: edit the whole prompt in `$VISUAL`/`$EDITOR`
  (Windows fallback: Notepad); save and close it to return the
  edited text.
- `Space` on a permission row: cycle `inherit → allow → ask → deny →
  inherit`.
- `w` or `Ctrl+S`: save using the existing validation path.
- `q` or `Esc`: return to Agents using the existing
  dirty-confirmation path (clean draft discards immediately; dirty
  draft arms the confirmation popup).

`INSERT` (text fields only):

- Every printable key, including `q`, `w`, `h`, `j`, `k`, `l`, is
  appended as literal text. Navigation, save, and quit keys are no-ops
  in `INSERT`.
- `Backspace`: delete the previous character.
- `Esc`: return to `NORMAL` without discarding the draft. A second
  `Esc` (now in `NORMAL`) runs the existing dirty-confirmation path.

Permissions: `Space` cycles `inherit → allow → ask → deny → inherit`.

Model picker: `Up/Down`, `Enter` apply, `m` manual (blank = inherit),
`r` rerun discovery, `Esc` cancel.

Install/Update: opens the **harness selector** on entry. `↑/↓` or
`j/k` pick OpenCode or Pi; `Enter` opens the per-file list scoped to
that harness only. On the list, `i` apply all safe actions for the
bound harness, `o` overwrite the selected conflict (confirm with `Y`,
cancel with `N` / `Esc`), `r` refresh the bound harness's plan. `Esc`
walks back through the sheets in order: armed popup → list → harness
selector → main menu. A session bound to one harness never reads the
other's directory or ownership map; `plan_for` and the per-target
`apply_safe` cleanup guarantee per-harness isolation.

Tools: `i` install the third-party OpenCode skill, `r` refresh, `Esc`
back.

## Tools (v1: install-only)

The Tools menu installs third-party OpenCode skills from a small
**bundled static catalog** baked into the binary
(`src/tools/mod.rs::DEFAULT_CATALOG`, which composes one entry from
`src/tools/pi_psql/mod.rs::ENTRY`). The first entry is `pi-psql`,
pinned to the verified tag `opencode-2026-09-23` (peeled commit
`0dba366061911f0ec389f4a78cc46fd6d6a19d41`).

### Module layout

The installer lives under `src/tools/` as a preventive split for future
per-tool UI:

- `src/tools/mod.rs` — shared types (`ToolCatalogEntry`, `ToolStatus`,
  `ToolItem`, `ToolOutcome`, `SpawnSpec`, `SpawnOutput`, `SpawnRunner`,
  `RenameRunner`), the default catalog, and the install flow
  (`tool_status`, `install_tool`, `install_tool_with`).
- `src/tools/pi_psql/mod.rs` — the bundled `pi_psql::ENTRY` constants.
- `src/tools/tests.rs` — the installer unit tests.

The generic Tools list UI (render / open / refresh / handle / install
and the pure helper) lives in `src/app/tools.rs` as a child module of
the TUI. `Screen::Tools`, the `MainItem::Tools` entry, dispatch, and
footer all stay in `src/app/mod.rs`.

### Install contract

v1 is **install-only**. It never updates, replaces, force-installs,
adopts unowned targets, or removes anything already at the
destination. Any existing target — regular directory, regular file, or
symlink — is a **conflict** that refuses to install. Add another tool
by appending one catalog entry; no other edit is required when the
default `SKILL.md` `name:` identity check is sufficient.

Each entry declares:

- `repo` — git remote URL (`https://github.com/taneralberto/pi-psql.git`
  for the bundled entry).
- `skill_name` — expected `SKILL.md` frontmatter `name:` value (e.g.
  `pi-psql`).
- `destination_subpath` — directory name under
  `<xdg|home>/.config/opencode/skills/`.
- `pin_tag` and `expected_sha` — verified immutable pin.
  `git rev-parse HEAD` in the staged tree must equal `expected_sha`,
  or install fails closed.
- `node_min` — full `(major, minor, patch)` tuple compared against
  `node --version` before any network call.

### Install flow (eight steps)

1. **Pre-flight** — `git --version`, `node --version` (parsed and
   compared to `node_min` full-tuple), `npm --version`. Missing tools
   or `node < node_min` yield `PrerequisitesMissing`. No filesystem
   writes.
2. **Resolve tag → peeled SHA** — `git ls-remote <repo> <pin_tag>`.
3. **Stage on the same volume** — staging dir is adjacent to the
   destination (sibling of `<skills>/<name>/`).
4. **Verify pinned SHA** — `git -C <staging> rev-parse HEAD` must equal
   `expected_sha`.
5. **Identity check** — `<staging>/SKILL.md` frontmatter `name:` must
   equal `skill_name`.
6. **Install deps** — `npm ci --omit=dev --ignore-scripts` inside
   staging. Both flags are mandatory.
7. **Publish via the OS no-replace primitive**:
   - **Linux** — `renameat2(2)` with `RENAME_NOREPLACE` (raw syscall
     via `libc`).
   - **Windows** — raw `MoveFileW` via `windows-sys`.
   - **Other platforms** — fail closed with `InstallFailed`.
8. **Cleanup** — on every failure path the staging dir is removed.

All external invocations use `std::process::Command::new(...).args(...)`
with **explicit argv** — no shell, no string interpolation.

## Synchronization safeguards

`Install/Update` plans and applies OpenCode and Pi targets
independently. A conflict in one never overwrites the other.

- Safe sync never touches `Conflict`, `Unowned`, or modified orphan
  targets.
- A conflict overwrite is path-specific and always asks for
  confirmation.
- Files and the ownership manifest are written via sibling temp file
  + rename.
- Files that cannot be parsed fail closed: nothing is installed that
  round.

## Checkpoint-path safeguards

- The configured checkout path is re-validated at every startup; a
  stale or moved checkout is an explicit error, never a silent
  fallback to a different checkout.
- The path field prefills from a cwd-anchored ancestor walk for an
  `agents/` directory (only when no settings file is present). The
  prefill is a hint: the picker never auto-confirms and never persists
  on prefill.
- An empty `agents/` directory is allowed — the runtime surfaces "no
  agents" through the normal empty-canonical-list path.
- The runtime never writes into `$HOME/.agenthd/agents/`: the
  canonical directory is always the configured checkout's `agents/`.
  The legacy local default directory is not part of the read or write
  paths under the new design.

## Out of scope

Local mode, Compare/import/export screens, Local↔Repo mode toggle,
project-local agents, agent runner, history/versions/rollback, remote
catalog, mouse interaction, themes, localization, background watching,
nested/pattern permission rules, or editing arbitrary unknown OpenCode
frontmatter.