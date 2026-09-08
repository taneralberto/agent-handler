# agenthd

`agenthd` is a small terminal UI that keeps your OpenCode agent definitions under
your own control. The canonical sources live in `agenthd`'s own config directory,
and `Install/Update` safely synchronizes them into the directory OpenCode reads
agents from.

## Bundled starter roles

`agenthd` ships one primary coordinator and seven OpenCode subagent starters in `src/agent.rs`:

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

`orchestrator` is `mode: primary`; the other seven are `mode: subagent`. All inherit OpenCode's default model.
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
| Subagent panel plugin | `$XDG_CONFIG_HOME/opencode/plugins/agenthd-subagents.tsx`, registered as `./plugins/agenthd-subagents.tsx` in `tui.json` |

`agenthd`-owned state always lives under `$HOME/.agenthd`, independent of
`XDG_CONFIG_HOME`. Only the OpenCode output directory follows the XDG layout,
because that's where OpenCode itself reads agents from.

On first run, if `$HOME/.agenthd` does not yet exist, `agenthd` looks for a
legacy install at `$XDG_CONFIG_HOME/agenthd` (or `$HOME/.config/agenthd`) and
moves it to `$HOME/.agenthd` once. The migration is opt-out by being
idempotent: if `$HOME/.agenthd` already exists, the legacy directory is left
untouched and the new one wins.

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

Install/Update: `i` apply all safe actions, `o` overwrite the selected conflict
(confirm), `r` refresh, `Esc` back.

Subagent panel: `i` install/update the managed OpenCode sidebar plugin and its
single `tui.json` entry, `u` uninstall both (confirm), `r` refresh, `Esc` back.
The panel lists child sessions with task title, agent, model, input-context
tokens, and live status. It never shows the delegated prompt body.

## Synchronization safeguards

- Safe sync never touches `Conflict`, `Unowned`, or modified orphan targets.
- A conflict overwrite is path-specific and always asks for confirmation.
- Files and the ownership manifest are written via sibling temp file + rename.
- Files that cannot be parsed fail closed: nothing is installed that round.

## Out of scope

Database, daemon, agent runner, versioning, remote catalog, project-local
targets, nested/pattern permission rules, mouse interaction,
themes, localization, or background file watching.