# agenthd

`agenthd` is a small terminal UI that keeps your agent definitions under your own
control. The canonical sources live in `agenthd`'s own config directory, and
`Install/Update` safely synchronizes them into the directories OpenCode and Pi
read agents from.

## Bundled starter roles

`agenthd` ships one primary coordinator and seven subagent starters under `src/agent/bundled/` (registry in `src/agent/bundled/mod.rs`, one `mod.rs` per role):

| Role | Purpose | Edits? |
| --- | --- | --- |
| `orchestrator` | Primary coordinator; delegates work and keeps decisions and acceptance centralized. | No |
| `scout` | Read-only codebase recon; targeted findings and risks. | No |
| `oracle` | Read-only advisor; surface drift, contradictions, narrowest next move. | No |
| `planner` | Read-only implementation planner; concrete, ordered plans with validation and risk discipline. | No |
| `reviewer` | Read-only change review; severity-ordered findings with file/line evidence. | No |
| `researcher` | Read-only web research; concise, well-sourced brief. | No |
| `delegate` | Lightweight implementation agent that executes an assigned task directly. | Yes |
| `worker` | Default implementation agent with plan-aware validation. | Yes |

`orchestrator` is `mode: primary`; the other seven are `mode: subagent`. All inherit OpenCode's default model. Pi receives equivalent Pi subagent definitions with the appropriate Pi tool allowlists.
The starter files in `$HOME/.agenthd/agents/` are yours: edit them freely, and
`Install/Update` will only fill in roles that are still missing — existing files
are never overwritten.

## Install

```sh
cargo install --path . --locked
```

Update with `cargo install --path . --locked --force`, remove with
`cargo uninstall agenthd`. The binary installs to `~/.cargo/bin`, which is
typically on `PATH` already via `~/.cargo/env`; add it to your shell's `PATH`
if it is not.

## Paths

| Purpose | Path |
| --- | --- |
| Canonical agents | `$HOME/.agenthd/agents/*.md` |
| Ownership manifest | `$HOME/.agenthd/state.json` |
| OpenCode agents | `$XDG_CONFIG_HOME/opencode/agents/*.md` (falls back to `$HOME/.config/opencode/agents/*.md`) |
| OpenCode skills | `$XDG_CONFIG_HOME/opencode/skills/<name>/` (falls back to `$HOME/.config/opencode/skills/<name>/`) |
| Pi agents | `$HOME/.pi/agent/agents/*.md` |
| Subagent panel plugin | `$XDG_CONFIG_HOME/opencode/plugins/agenthd-subagents.tsx`, registered as `./plugins/agenthd-subagents.tsx` in `tui.json` |

`agenthd`-owned state always lives under `$HOME/.agenthd`, independent of
`XDG_CONFIG_HOME`. The OpenCode output directory follows the XDG layout; Pi reads its global agents from `$HOME/.pi/agent/agents`.

On first run, if `$HOME/.agenthd` does not yet exist, `agenthd` looks for a
legacy install at `$XDG_CONFIG_HOME/agenthd` (or `$HOME/.config/agenthd`) and
moves it to `$HOME/.agenthd` once. The migration is opt-out by being
idempotent: if `$HOME/.agenthd` already exists, the legacy directory is left
untouched and the new one wins.

## Source layout

The paths above are resolved and read/written by a small store module
shared across the app. The on-disk paths themselves are unchanged.

- `src/store/mod.rs` — path resolution, the ownership manifest, and
  shared disk I/O (atomic temp+rename, parsing).
- `src/store/canonical.rs` — load/save of the canonical
  `$HOME/.agenthd/agents/*.md` agents (source of truth).
- `src/store/sync.rs` — plan/apply for the OpenCode and Pi targets
  driven by `Install/Update`.
- `src/store/plugin.rs` — the managed subagent sidebar plugin.
- `src/store/tests.rs` — store unit tests.

## Keys

Main menu: `Up/Down` or `j/k`, `Enter`, `q` / `Esc` exit.

Agents: `n` create, `Enter`/`e` edit, `d` delete (confirm), `u` update bundled prompts (confirm), `Esc` back.

`u` refreshes the prompt body of every bundled canonical agent that
already exists on disk (`delegate`, `oracle`, `orchestrator`, `planner`,
`researcher`, `reviewer`, `scout`, `worker`). The command arms a confirmation popup
first; press `y`/`Y` to apply or `n`/`N`/`Esc` to cancel. On apply,
only the prompt body is rewritten — each file's existing description,
mode, model, and permissions are read back and preserved. Missing
canonical files are skipped (startup seeding owns creation); any
user-created agent is never touched. Each save uses the same atomic
temp+rename path as the normal editor save.

Editor (Vim-style): starts in `NORMAL`. The current mode is shown above the
fields as `[ NORMAL ]` or `[ INSERT ]`.

`NORMAL`:

- `Up`/`Down` or `j`/`k`: move between fields (including stepping through the
  permission list via the existing field helpers).
- `Left`/`Right` or `h`/`l`: on Mode, cycle `subagent → primary → all`; on the
  permission list, step between rows; on the other fields, no-op.
- `i`: enter `INSERT` on a text field (Name, Description, Prompt). No-op on
  Mode, Model, and the permission list.
- `Enter` on Model: open the model picker.
- `e` on Prompt: edit the whole prompt in `$VISUAL`/`$EDITOR` (Windows fallback: Notepad); save and close it to return the edited text.
- `Space` on a permission row: cycle `inherit → allow → ask → deny → inherit`.
- `w` or `Ctrl+S`: save using the existing validation path.
- `q` or `Esc`: return to Agents using the existing dirty-confirmation path
  (clean draft discards immediately; dirty draft arms the confirmation popup).

`INSERT` (text fields only):

- Every printable key, including `q`, `w`, `h`, `j`, `k`, `l`, is appended as
  literal text. Navigation, save, and quit keys are no-ops in `INSERT`.
- `Backspace`: delete the previous character.
- `Esc`: return to `NORMAL` without discarding the draft. A second `Esc`
  (now in `NORMAL`) runs the existing dirty-confirmation path.

Permissions: `Space` cycles `inherit → allow → ask → deny → inherit`.

Model picker: `Up/Down`, `Enter` apply, `m` manual (blank = inherit), `r`
refresh, `Esc` cancel.

Install/Update: opens the **harness selector** on entry. `↑/↓` or `j/k` pick
OpenCode or Pi; `Enter` opens the per-file list scoped to that harness
only. On the list, `i` apply all safe actions for the bound harness, `o`
overwrite the selected conflict (confirm with `Y`, cancel with `N` /
`Esc`), `r` refresh the bound harness's plan. `Esc` walks back through the
sheets in order: armed popup → list → harness selector → main menu. A
session bound to one harness never reads the other's directory or
ownership map; `plan_for` and the per-target `apply_safe` cleanup
guarantee per-harness isolation.

Subagent panel: `i` install/update the managed OpenCode sidebar plugin and its
single `tui.json` entry, `u` uninstall both (confirm), `r` refresh, `Esc` back.
The panel lists child sessions with task title, agent, model, input-context
tokens, and live status. It never shows the delegated prompt body.

Tools: `i` install the third-party OpenCode skill, `r` refresh, `Esc` back.

## Tools (v1: install-only)

The `Tools` menu installs third-party OpenCode skills from a small **bundled
static catalog** baked into the binary (`src/tools/mod.rs::DEFAULT_CATALOG`,
which composes one entry from `src/tools/pi_psql/mod.rs::ENTRY`). The first
entry is `pi-psql`, pinned to the verified tag `opencode-2026-09-23` (peeled
commit `0dba366061911f0ec389f4a78cc46fd6d6a19d41`).

### Module layout

The installer lives under `src/tools/` as a preventive split for future
per-tool UI:

- `src/tools/mod.rs` — shared types (`ToolCatalogEntry`, `ToolStatus`,
  `ToolItem`, `ToolOutcome`, `SpawnSpec`, `SpawnOutput`, `SpawnRunner`,
  `RenameRunner`), the default catalog, and the install flow
  (`tool_status`, `install_tool`, `install_tool_with`).
- `src/tools/pi_psql/mod.rs` — the bundled `pi_psql::ENTRY` constants.
- `src/tools/tests.rs` — the installer unit tests; existing tests
  kept, with additional tests added during the npm-launcher rework
  (Windows npm-direct-`node` fix, `discovery_seam`, etc.).
- `src/tools/<tool>/view.rs` — a future per-tool view (config / execute
  UI). v1 ships no view; this file is intentionally not present.

The generic Tools list UI (render / open / refresh / handle / install
and the pure helper) lives in `src/app/tools.rs` as a child module of
the TUI. `Screen::Tools`, the `MainItem::Tools` entry, dispatch, and
footer all stay in `src/app/mod.rs`.

### Install contract

v1 is **install-only**. It never updates, replaces, force-installs, adopts
unowned targets, or removes anything already at the destination. Any
existing target — regular directory, regular file, or symlink — is a
**conflict** that refuses to install. Add another tool by appending one
catalog entry; no other edit is required when the default `SKILL.md`
`name:` identity check is sufficient.

Each entry declares:

- `repo` — git remote URL (`https://github.com/taneralberto/pi-psql.git` for the
  bundled entry).
- `skill_name` — expected `SKILL.md` frontmatter `name:` value (e.g. `pi-psql`).
- `destination_subpath` — directory name under `<xdg|home>/.config/opencode/skills/`.
- `pin_tag` and `expected_sha` — verified immutable pin. `git rev-parse HEAD`
  in the staged tree must equal `expected_sha`, or install fails closed.
- `node_min` — full `(major, minor, patch)` tuple compared against
  `node --version` before any network call. The bundled `pi-psql`
  entry sets `22.12.0` deliberately, to match the `^22.12.0` leg of
  `yargs@^18`'s `engines.node` (a transitive dependency). Recording
  `min_major = 22` alone would accept `22.x` versions that ship with a
  system package manager but cannot load yargs 18; the full tuple
  catches that case. Future entries can tighten or loosen the bound
  per their own transitive engines.

### Install flow (eight steps)

1. **Pre-flight** — `git --version`, `node --version` (parsed and compared
   to `node_min` full-tuple), `npm --version`. Missing tools or
   `node < node_min` yield `PrerequisitesMissing`. No filesystem writes.
2. **Resolve tag → peeled SHA** — `git ls-remote <repo> <pin_tag>`. Annotated
   tags return two lines; lightweight tags return one. The peeled commit
   SHA is the last 40-hex line. A sanity check refuses to proceed if the
   peeled SHA differs from `expected_sha` (`InstallFailed`).
3. **Stage on the same volume** — staging dir is adjacent to the destination
   (sibling of `<skills>/<name>/`). `git init <staging>`;
   `git -C <staging> remote add origin <repo>`;
   `git -C <staging> fetch --depth=1 origin <pin_tag>`;
   `git -C <staging> checkout FETCH_HEAD`. Fetch by **tag**, never by SHA.
4. **Verify pinned SHA** — `git -C <staging> rev-parse HEAD` must equal
   `expected_sha`. Tag moved, repo compromised, or wrong constant →
   remove staging, `InstallFailed`.
5. **Identity check** — `<staging>/SKILL.md` frontmatter `name:` must equal
   `skill_name`. Mismatch or malformed → remove staging,
   `IdentityMismatch`.
6. **Install deps** — `npm ci --omit=dev --ignore-scripts` inside staging.
   Both flags are mandatory: third-party post-install scripts run with the
   user account and must not be trusted implicitly. Non-zero exit →
   remove staging, `InstallFailed`.

   **Windows npm launcher.** The Node Windows installer ships `npm`
   (a bash script with no extension — unlaunchable by `CreateProcessW`)
   and `npm.cmd` (which internally invokes `cmd.exe /c` — also
   forbidden by this section's strict no-shell rule). The installer
   therefore reaches npm through `node` directly, with the absolute
   path to `npm-cli.js` as `argv[0]`:

   ```
   node "<launcher_parent>\node_modules\npm\bin\npm-cli.js" ci --omit=dev --ignore-scripts
   ```

   The path is resolved at runtime by walking PATH for `npm.cmd`
   entries and using each launcher's parent directory as a layout
   hint (`<launcher_parent>/node_modules/npm/bin/npm-cli.js`) — no
   `.cmd` is ever spawned or parsed, no shell is invoked. If no PATH
   entry resolves to a usable `npm-cli.js`, preflight returns
   `PrerequisitesMissing` with an actionable detail that names the
   missing prerequisite and asks the user to reinstall Node.js.
7. **Publish via the OS no-replace primitive**:
   - **Linux** — `renameat2(2)` with `RENAME_NOREPLACE` (raw syscall via
     `libc`). Returns `EEXIST` if `<target>` exists, `EXDEV` if staging
     is on a different filesystem.
   - **Windows** — raw `MoveFileW` via `windows-sys` (not
     `MoveFileExW`, which would lower to `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`
     and clobber). Per Microsoft's docs, `MoveFileW` returns `FALSE` with
     `ERROR_ALREADY_EXISTS` when the destination exists and
     `ERROR_NOT_SAME_DEVICE` when source/destination differ in volume.
   - **Other platforms** — fail closed with `InstallFailed`. There is no
     proven no-replace primitive for v1 on macOS/BSDs/etc.

   There is **no separate "exists" check before the primitive** — no
   TOCTOU window. `EEXIST` / `ERROR_ALREADY_EXISTS` → `Conflict`, no-op,
   staging removed. `EXDEV` / `ERROR_NOT_SAME_DEVICE` → `InstallFailed`,
   staging removed. Any other error → `InstallFailed`, staging removed.

   Staging is always adjacent to the destination on the same volume so
   the same-volume rule is met by construction.
8. **Cleanup** — on every failure path the staging dir is removed;
   `.key`, `connections.enc`, the destination, or any sibling tool are
   never touched.

All external invocations use `std::process::Command::new(...).args(...)` with
**explicit argv** — no shell, no string interpolation. Every spawn carries a
timeout (`30s` for `git`/`node`/`npm` metadata; `120s` for `npm ci`) and a
kill+reap watchdog that drains stdout/stderr on timeout. stderr is surfaced
in failure messages as the last ~20 lines.

### Statuses

`NotInstalled`, `Installed`, `PrerequisitesMissing`, `IdentityMismatch`,
`InstallFailed`, `Conflict` (target already exists). No `UpToDate`,
`UpdateAvailable`, force-install, or uninstall in v1.

### Windows path caveat

The Windows destination resolves under `%USERPROFILE%\.config\opencode\skills\<name>\`.
There is no `$XDG_CONFIG_HOME` on Windows; the home fallback is the
only path. Staging sits adjacent on the same drive so `MoveFileW` can
publish without crossing volumes.

### Windows npm runtime caveat

The Windows launcher for npm is `node <absolute npm-cli.js> ...` — no
`npm.cmd` and no shell. `npm-cli.js` is located by walking PATH for
`npm.cmd` entries and reading `<launcher_parent>\node_modules\npm\bin\npm-cli.js`.
If the canonical npm layout is missing (no `npm.cmd` in PATH, or no
`npm-cli.js` next to it), preflight returns `PrerequisitesMissing`
naming the missing prerequisite; staging is never created. See the
**Windows npm launcher** note under step 6 above for the full rationale.

### Credential non-interference

The installer publishes files only. It never reads `.key`,
`connections.enc`, or any decrypted secret, never invokes
`open-connection-manager`, never touches environment variables, and
never touches the connection-manager UI. The credential flow belongs to
the skill's runtime, not the installer.

### Remote catalog discovery: still out of scope

The bundled static catalog is **not** remote catalog discovery. The catalog
is a `const` baked into the binary; `agenthd` does not fetch a list of tools
from any remote source. The existing "Out of scope" line in this README —
which excludes remote catalog discovery — still applies. Adding a tool
remains a one-line source edit to `DEFAULT_CATALOG`.

## Synchronization safeguards

`Install/Update` plans and applies OpenCode and Pi targets independently. A conflict in one never overwrites the other.

- Safe sync never touches `Conflict`, `Unowned`, or modified orphan targets.
- A conflict overwrite is path-specific and always asks for confirmation.
- Files and the ownership manifest are written via sibling temp file + rename.
- Files that cannot be parsed fail closed: nothing is installed that round.

## Out of scope

Database, daemon, agent runner, versioning, remote catalog, project-local
targets, nested/pattern permission rules, mouse interaction,
themes, localization, or background file watching.