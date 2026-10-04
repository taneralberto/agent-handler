# agenthd

`agenthd` is a small terminal UI that keeps your agent definitions under your
own control. The canonical sources live in a single user-configured checkout
on disk, and `Install/Update` safely synchronizes them into the directories
OpenCode and Pi read agents from.

## Current GUI and packaging status (2026-10-04)

Latest evidence supersedes preparation-only claims below: root **576 passed,
1 ignored**; Tauri default/custom-protocol **95 passed each**, Node **84 passed**,
Angular build passed; installer **29** + packaging **14** = **43 passed**.
Approved source snapshot and real offline CachyOS paired package build succeeded;
actual isolated CLI + GUI custom-protocol installer succeeded, without host
installation or AUR publication. See the latest top note in `GUI_ROADMAP.md`
and [`packaging/arch/README.md`](packaging/arch/README.md) for checksum, logs,
`--nodeps` limitation and confirmed removal of the historical GUI `$srcdir` warning.
Current source: `/tmp/opencode/agenthd-arch-remapped/agenthd-0.1.0.tar.gz`;
paired CLI/GUI package: `agenthd-0.1.0-1-x86_64.pkg.tar.zst` in that directory.
Checksums and current matrix: [`packaging/arch/VALIDATION.md`](packaging/arch/VALIDATION.md).
Snapshot predates these doc updates and the root `npm run install:global`
entrypoint; it does not include that entrypoint. Latest working-tree docs are
not identical. Counts are last parent-verified, not rerun here.
Historical arch-final GUI in `/tmp/opencode/agenthd-gui-smoke` was launched and
terminated (log 0 bytes, no visual/IPC inspection); new remapped GUI **not launched**.
Current visual gates remain pending; local stub Discovery does not
certify real OpenCode. Older read-only Linux gate stays closed. The following
status details are historical where they conflict with this latest note.

`GUI_ROADMAP.md` (top status) is the authority; older slice descriptions
below are historical and superseded where they say edit-only, TUI-only
CRUD, or D3 implementation pending. The companion now implements Settings,
agent editor + create/rename/delete, and D3 long jobs. Listener-first
`agenthd-operation` events replace polling; Close requests cooperatively
cancel active jobs through the real emitter. Backend-retained snapshots and
monotonic sequence reconciliation support recovery; unresolved IPC recovery
keeps mutations fail-closed. Cancellation preserves partial work, no rollback.

Historical pre-Plans checks (superseded by the top counts): Tauri `cargo test --lib` **88 passed** in both
default and `--features custom-protocol` variants; `npm run test:unit`
**72 passed**; `npm run build` passed (unit/build evidence, not visual proof).
Root `cargo test --all-targets`: **573 passed, 1 ignored**, last verified in
the prior block, not rerun for packaging preparation. Installer tests were
rerun here: **29 passed**, alongside **8 packaging tests** (37 total).
Settings/editor/D3/CRUD visual acceptance remains **pending**; no GUI was
installed or launched against this current tree. The earlier read-only
paired GUI↔TUI gate remains **closed** by user report on Linux/CachyOS,
not Windows; it does not certify these newer controls or current binaries.

D4 Arch/CachyOS-first **local source package preparation** is available in
[`packaging/arch/README.md`](packaging/arch/README.md): paired `/usr/bin`
binaries, real snapshot checksum, no remote URL or AUR publication. List and
approve working-tree sources before generating artifacts. Full isolated
package build and inspection are complete for the approved remapped artifacts;
full dependency checks/clean chroot remain pending externally. No further code
changes needed for the agreed automated scope; next steps are manual acceptance
and external validation. No host install or current GUI runtime claim; D5
remains unresolved. Both CLI and GUI
are MIT licensed; see [LICENSE](LICENSE).

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
agenthd --repo <abs-path>            # point at a checkout (validated up front)
agenthd tui --repo <abs-path>        # explicit TUI mode (default)
agenthd --repo <abs-path> tui        # --repo and mode keyword in either order
```

`--repo` requires an absolute path. The path is validated up front so
the user sees a clear error if the checkout is unusable. When the path
is valid it is persisted to `settings.json` so subsequent launches
agree with the user. The `--repo` value is rejected when it equals
`tui` or `gui` so the parser never confuses the value with the mode
keyword — matters when the user types `agenthd --repo tui` by
accident.

### Modes (TUI + companion GUI)

The binary accepts a single positional mode keyword:

```sh
agenthd                # default: TUI
agenthd tui            # explicit TUI (same as default)
agenthd gui            # GUI — requires the companion binary `agenthd-gui(.exe)` next to `agenthd`
```

The GUI branch locates the companion binary `agenthd-gui` (or
`agenthd-gui.exe` on Windows) **adjacent to the current executable**
and spawns it with the same argv; `agenthd` then waits and forwards
the companion's exit code. If the companion is missing,
`agenthd gui` exits with code `2` and a clear stderr message that
names the missing file; `settings.json`, `state.json`, and the
OpenCode target tree are never touched. The companion check runs
before `resolve_checkout_path`, so even a valid `--repo` cannot turn
a missing-companion rejection into a `settings.json` write. PATH is
**not** consulted: spawning arbitrary PATH executables would let a
hostile `PATH` masquerade as the companion. The parser is shared by
every mode, so `--repo` applies to both TUI and GUI in the same way
— the GUI branch runs the same `resolve_checkout_path` override
precedence / save-if-changed logic as the TUI branch, then hands
control to the companion.

### Paired install contract (CLI + companion GUI)

The root CLI (`agenthd`) and the companion GUI (`agenthd-gui`) are
two **separate** crates with separate build steps. `cargo install
--path .` of the root repository only installs the CLI — the
companion is not produced and `agenthd gui` will refuse to start
until both binaries are present next to each other. The companion
lives at `spikes/tauri-angular/` and depends on the root library by
path, so installing it requires running the Angular frontend build
**first** to populate `spikes/tauri-angular/dist/`, then installing
the Tauri binary via `cargo install` (same cargo root as the CLI,
so `agenthd` finds it adjacent to itself with no PATH juggling):

```sh
# 1. Install the CLI (one crate, no frontend).
cargo install --path . --locked

# 2. Build the Angular bundle the Tauri companion embeds. The
#    `dist/` directory is what `tauri build` reads at install time,
#    so this MUST run before step 3.
cd spikes/tauri-angular
npm ci            # reproducible install from the versioned lockfile
npm run build     # Angular bundle into dist/

# 3. Install the companion binary into the SAME cargo root as
#    the CLI. `cargo install` defaults to `~/.cargo/bin` on Linux
#    and `C:\Users\<user>\.cargo\bin` on Windows, the same root
#    `cargo install --path .` of step 1 wrote `agenthd(.exe)` to,
#    so the root CLI's adjacent-companion lookup finds
#    `agenthd-gui(.exe)` without any copy step. If you customized
#    `CARGO_HOME` or passed `--root <dir>` to step 1, pass the
#    same value to step 3 (e.g. `CARGO_HOME=<dir> cargo install
#    --path src-tauri --locked --bin agenthd-gui --features
#    custom-protocol --root <dir>`) so both binaries land next
#    to each other — otherwise the CLI's adjacent-companion
#    lookup will not find the companion. The package name
#    is still `agenthd-tauri-angular-spike` (historical / internal);
#    `--bin agenthd-gui` pins the production-style binary name the
#    root CLI actually looks for. `--features custom-protocol` is
#    required for production installs: the feature is intentionally
#    opt-in (not in `default`) so `tauri dev` keeps working, but
#    a direct `cargo install` / `cargo build` of the companion
#    without it leaves `dev = true` in `tauri-macros` and the
#    codegen does not embed the frontend assets the production
#    runtime expects. **`cargo install --path` updates the
#    same package without `--force`** when the package being
#    installed is identical to the one already at the destination
#    (same crate); cargo refuses overwrites only when the
#    destination binary does not match the one cargo would
#    produce. Pass `--force` only when you intend to replace a
#    `agenthd-gui(.exe)` whose bytes do not match what this
#    command would install — i.e. an out-of-band build, a
#    different commit, or a `dev`-feature artifact. Forward
#    `--features custom-protocol` on the rerun as well; without
#    it cargo installs the dev-feature artifact that fails at
#    runtime. The orchestrator
#    `scripts/install.mjs --force` forwards `--force` to both
#    installs and is the recommended way to update an existing
#    paired build.
cargo install --path src-tauri --locked --bin agenthd-gui --features custom-protocol
```

The companion crate is the existing `spikes/tauri-angular/src-tauri/`
crate with its binary renamed to `agenthd-gui` — there is **no**
second copy. The crate directory stays under `spikes/` for now
(it still serves as a spike / slice boundary), but the binary name
and the displayed branding are production-style (`agenthd-gui`,
`agenthd GUI`). Commands, capabilities, CSP, and read-only
behaviour are unchanged from the spike's D1 contract. Step 3
above is the **install** path; the underlying `cargo build`
artifact is still `target/release/agenthd-gui(.exe)` under
`spikes/tauri-angular/src-tauri/`, but `cargo install` is what
copies it into the cargo root the CLI ships from — no manual
"drop the binary next to the CLI" step is needed (or implied).
The single-step `cargo install` of the root **does not** bundle
the companion; both crates must be installed separately.

**Historical source-installer boundary, not the current D4 status.** This slice installs both binaries
into the same cargo root and stops there; Arch packages, `.deb` /
`.rpm` / AppImage, winget / Scoop / MSI installers, and any
per-platform packaging are explicitly **out of scope** for this
slice and remain gate-of-Fase-6 (D4 pendiente) work.

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

From the repository root, the recommended command installs / updates both
the CLI and the companion GUI into the same cargo root, building the
Angular frontend first:

```sh
npm run install:global
# After installation, launch explicitly (default per-user destination):
~/.cargo/bin/agenthd gui --repo /absolute/path/to/checkout
```

This is a thin orchestrator: it preflights `cargo` / `npm` / `node`,
runs `npm ci --include=dev` and `npm run build` in
`spikes/tauri-angular/`, confirms `dist/.../browser/index.html`
was produced, and then runs both `cargo install` invocations from
the repo root with the same `--root`. By default the install
root is per-user, with precedence
`--root > CARGO_INSTALL_ROOT > CARGO_HOME > $HOME/.cargo`;
both binaries land in its `bin/` directory. This is not
`npm install -g`, a system-wide install, or an npm publication; no sudo is
needed. The root npm script delegates to `node scripts/install.mjs --force`,
so rerunning it replaces both destination binaries. Production frontend assets
are built and embedded through the companion's `custom-protocol` feature.
Installation never launches the GUI or writes agenthd settings.

For a portable or isolated destination, forward installer arguments through npm:

```sh
npm run install:global -- --root /absolute/path/to/install-root
```

Launch `/absolute/path/to/install-root/bin/agenthd` when using that override
(or the corresponding Cargo environment destination). Use
`npm run install:global -- --help` for the full installer flag reference.
The direct `node scripts/install.mjs` entrypoint remains available without
forced replacement unless you pass `--force`.

### Update

Before updating an existing paired build, **close both the TUI
and the GUI** so the destination binaries (`agenthd(.exe)` and
`agenthd-gui(.exe)`) are not locked on Windows. From the
repository root run the same orchestrator with `--force` so it
forwards `--force` to both `cargo install` invocations (the
companion one keeps `--features custom-protocol`, both land in
the same cargo root):

```sh
npm run install:global
```

After the orchestrator reports `completed`, launch the GUI via
`agenthd gui` or fall back to the TUI with `agenthd`. There is no
separate "GUI install" step: the orchestrator already produced
both, and `agenthd` locates the companion adjacent to itself.

`cargo install --path . --locked --force` from the repo root is
**not** the recommended update path. It installs **only** the
CLI, leaves the companion `agenthd-gui(.exe)` untouched, and
without `--features custom-protocol` it is also not the right
shape for the companion crate. Prefer the orchestrator above;
reach for the manual CLI-only install only when the GUI
companion is intentionally out of scope.

Prerequisites: Node.js with `npm` on `PATH` (Angular 22 / npm
lockfile-driven; **^22.22.3 || ^24.15.0 || >=26.0.0**), Rust with `cargo` on `PATH`, and the Tauri
WebView2 runtime on Windows plus `webkit2gtk-4.1` (and friends)
on Linux. The orchestrator reports a missing tool as a preflight
abort and does not write anything. See
"Paired install contract (CLI + companion GUI)" below for the
manual cargo steps (single CLI install without the orchestrator).

Manual single-CLI install (no companion GUI):

```sh
cargo install --path . --locked
```

Update with `cargo install --path . --locked --force`, remove with
`cargo uninstall agenthd`. The binary installs to `~/.cargo/bin`, which
is typically on `PATH` already via `~/.cargo/env`; add it to your
shell's `PATH` if it is not. The CLI alone is enough to run `agenthd
tui` but not `agenthd gui`.

Installing the CLI alone is not enough to run `agenthd gui`: the GUI
branch requires the companion binary `agenthd-gui(.exe)` next to the
CLI's binary. See "Paired install contract (CLI + companion GUI)"
above for the build steps.

### GUI Settings slice (Fase 5, mutación pequeña síncrona)

Slice vertical aprobado: la GUI Settings ahora persiste la ruta
del checkout que el usuario teclea, reutilizando
`workflows::apply_checkout` (la misma función que invoca la
pantalla Settings del TUI). Una caja de texto + botón Guardar
en la sección Settings invocan un tercer comando Tauri
(`apply_checkout`) que delega en el workflow; en éxito el
frontend re-dispara `settings_status` + `list_agents` para
recomponer las dos pantallas, en fallo preserva el borrador y
muestra el texto exacto de `ApplyError::message()` (mismas
cadenas que el TUI). El lock `busy` serializa TODAS las
acciones públicas (refresh + save) para que un refresh manual
no pueda correr contra un save en vuelo. El input queda
deshabilitado solo mientras `saving` está activo (teclear
durante el refresh inicial está permitido y `draftSeeded`
preserva el buffer). El botón Guardar queda deshabilitado
solo mientras `busy` está activo; no se bloquea por buffer
vacío (esa restricción era del flujo TUI, no del flujo GUI).

Evidencia de tests focales del backend
(`spikes/tauri-angular/src-tauri/`):

- `cargo test --locked --lib`: 16/16 OK (8 previos + 8 nuevos).
  Los 8 nuevos cubren el camino feliz (valid trim persiste y
  recomponer Settings/Agents del nuevo checkout), el cambio
  entre dos checkouts con agentes distintos (recompose Agents
  con el nuevo set), y cinco rechazos (vacío, relativo, no
  existe, sin `agents/`, fallo de escritura) que conservan
  `settings.json` byte-exact y `paths.canonical_dir` original.
  El test de symlink se mantiene `#[cfg(unix)]` igual que el
  resto del lib. Sin alias, sin copiar validación, sin `ensure_dirs`,
  sin escrituras en `canonical`/`targets`/`skills`/`state`/`canonical`.
  Sin cambios de schema, sin permisos nuevos, capabilities
  intactas (`["core:default"]`), CSP intacta, proyección
  sigue sin `prompt`/`permissions`.
- `cargo test --locked --lib --features custom-protocol`: 16/16 OK.
- `cargo clippy --locked --lib --tests [--features custom-protocol]`:
  sin warnings nuevos vs baseline (3 warnings preexistentes en
  `src/tools/mod.rs` del crate raíz, no tocados).
- `cargo fmt --check` y `npm run build` limpios.

Gate visual pendiente: end-to-end del runtime gráfico paired
GUI↔TUI (Save → refresh ambos → ver el mismo checkout y los
mismos agentes en ambas superficies). El gate visual del
slice read-only previo (comparación paired sobre mismo HOME)
sigue cerrado por reporte manual user-reported; este nuevo
slice mutación síncrona pequeña **NO** invalida ese gate
ni lo da por demostrado, requiere su propia confirmación
visual no instrumentada con `apply_checkout` Save →
refresh + commit/push fuera de alcance aquí.

### GUI editor slice (mutación pequeña síncrona — edit ONLY)

Segundo slice vertical aprobado: la GUI ahora puede editar
**agentes existentes**. Rename / create / delete siguen
siendo TUI-only en esta fase (D3 los desbloquea cuando
aplique). El editor es **inline-por-agente**: cada fila de
Agents expone un botón `Edit` que abre un panel con la fila
seleccionada en modo edición y el resto de la lista como
solo-lectura. Controles nativos `<input>`, `<select>` y
`<textarea>` — sin framework de formularios, sin DOM
adicional, sin CSS remota. Campos editables: `description`,
`mode`, `model`, `prompt` y la tabla completa de permisos
(los permisos desconocidos se preservan verbatim en el
round-trip; el lib los rechaza al guardar con el error
estándar `unknown permission key`).

Backend: dos comandos Tauri adicionales
(`load_agent_for_edit`, `save_agent_edit`) que delegan en
helpers de composición puros sobre `&Paths`. El seam de
apertura `agenthd::store::load_agent_for_edit` (nuevo en
este bloque) hace una sola lectura del archivo, valida
nombre / fuente canónica / symlink / no-regular / UTF-8,
parsea y devuelve el `Agent` + el SHA-256 de los **mismos
bytes** que consumió el parser. El seam de guardado
`agenthd::workflows::save_agent` (pre-existente, ya
extraído en Fase 2) hace la pasada rename-then-save; el
helper `compose_save_agent_edit` añade las precondiciones
GUI-only (contexto no vacío, checkout persistido `Ready`
sigue coincidiendo con el de apertura, nombre inmutable,
archivo sigue existiendo — no convierte una edit en alta)
antes de invocar el workflow. Errores literales del lib
alcanzan al frontend sin prefijo ni copia.

Frontend: el editor se abre desde una fila específica con
un click en `Edit`; `busy` lock se mantiene antes de cada
`await` en TODAS las acciones públicas. El draft del editor
vive en signals separados (`editDraft`, `editOriginal`,
`editContext`) y nunca se sobreescribe por un refresh del
listado. El refresh both (`refreshAll`) pide confirmación
nativa (`confirm`) antes de descartar un draft sucio, y
rechaza ejecutar si el editor está abierto en modo
"intentando guardar el checkout". El save cierra el draft
**antes** del refresh post-write para que un refresh
fallido no pueda enmascarar el éxito del save; el error de
refresh post-save se reporta aparte, sin reiniciar el
draft. El discard pide `confirm` nativo solo cuando hay
cambios sin guardar.

Evidencia de tests focales del backend (en
`spikes/tauri-angular/src-tauri/src/lib.rs`):

- `cargo test --locked --lib`: 28/28 OK (16 previos del
  slice Settings + 12 nuevos del slice editor: round-trip
  load → edit → save de todos los campos, refresh
  empty/stale/missing-file, rechazo name-change, rechazo
  cuando el archivo canónico desaparece, propagación del
  error `changed on disk`, rechazo cuando el checkout
  configurado cambió entre open y save, rechazo de
  contexto vacío, pin de no-escritura sobre `settings.json`
  / `state.json` / `target_dir` / `pi_target_dir`, pin de
  no-escritura en load, pin JSON de que el summary no se
  ensancha con campos del editor, pin top-level keys
  exactos de `AgentEditDto` y `AgentEditContext`).
- `cargo test --locked --lib --features custom-protocol`:
  28/28 OK.
- `cargo clippy --locked --lib --no-deps [--features
  custom-protocol]`: sin warnings nuevos vs baseline (3
  warnings preexistentes en `src/tools/mod.rs` del crate
  raíz, no tocados).

Tests del seam en `src/store/tests.rs`:

- `cargo test --locked --offline --all-targets` (root):
  7 nuevos tests focales para
  `load_agent_for_edit`: snapshot hash idéntico a
  `hash_file`, rechazo de nombre inválido (incluye
  traversal-style `../escape`), fail-closed ante fuente
  canónica faltante, rechazo de symlink en la fuente,
  rechazo de bytes no parseables, rechazo de archivo
  inexistente, pin de que el hash es hex ASCII minúscula
  de 64 chars.

Tests de helpers frontend (sin DOM/IPC):

- `node --experimental-strip-types --test
  spikes/tauri-angular/editor-dto.test.mjs`: 18/18 OK
  sobre las funciones puras `deepCopyDto` y `sameDto` que
  el componente importa desde
  `spikes/tauri-angular/src/app/editor-dto.ts`. Sin
  Angular runtime, sin Tauri mock, sin DOM.

`cargo fmt --check` y `npm run build` limpios.

Gate visual pendiente (igual que el slice Settings): la
mutación edit-only necesita su propia confirmación paired
GUI↔TUI sobre el mismo HOME — Edit → cambiar description
y un permiso → Guardar → ver el archivo canónico con esos
cambios en la TUI (sin shadow copy, sin IPC instrumentado).
**D3 APROBADO** por el usuario — cancelación cooperativa
vía puntos seguros del workflow con cambios parciales sin
rollback automático (NO es precondición de este slice
síncrono pequeño; la implementación queda pendiente y se
desbloqueará cuando el slice vertical incorpore operaciones
largas); **D4 APROBADO** con prioridad Arch / CachyOS —
PKGBUILD first, sin publicar en AUR, pruebas visuales y
de build/install solo AISLADAS en `/tmp/opencode`, sin
HOME real ni binarios reales del host (implementación
pendiente); **D5 conserva schema/rutas sin migrar
anunciado** (no se cierra cross-platform).

## Paths

| Purpose | Path |
| --- | --- |
| Settings (configured checkout path) | `$HOME/.agenthd/settings.json` |
| Ownership manifest | `$HOME/.agenthd/state.json` |
| Canonical agents | `<configured-checkout>/agents/*.md` |
| Checked-out skills | `<configured-checkout>/skills/<name>/` |
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
  checkout path), `validate_checkout_path`, and the cwd-ancestor hint
  resolver used by the first-run picker.
- `src/store/canonical.rs` — load/save of the canonical agents (source
  of truth) under the configured checkout.
- `src/store/sync.rs` — plan/apply for the OpenCode and Pi agent
  targets driven by `Install/Update`.
- `src/store/skills.rs` — plan/apply for the configured checkout's
  `skills/` directory into the OpenCode global skills dir; whole-tree
  hash, identity check, no force-overwrite, third-party tools
  (`pi-psql`) remain untouched unless the source ships them and the
  manifest already records them.
- `src/store/skills_tests.rs` — skills sync unit tests.
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
`j/k` pick OpenCode, Pi, or Skills; `Enter` opens the per-file list
scoped to that target only. On the OpenCode/Pi list, `i` applies all
safe actions for the bound harness, `o` overwrites the selected
conflict (confirm with `Y`, cancel with `N` / `Esc`), `r` refreshes
the bound harness's plan. On the Skills list, `i` applies the safe
plan, `r` refreshes from disk, `o` is intentionally refused with a
status-bar explanation (skills sync has no force-overwrite path —
third-party tools like `pi-psql` must remain untouched unless the
configured checkout ships them and the manifest already records
them). `Esc` walks back through the sheets in order: armed popup →
list → harness selector → main menu. A session bound to one harness
never reads the other's directory or ownership map; `plan_for` and
the per-target `apply_safe` cleanup guarantee per-harness isolation.

Tools: `i` install the third-party OpenCode skill, `r` refresh, `Esc`
back.

## Skills (configured-checkout sync)

The configured checkout's `skills/<name>/` directories install into
the OpenCode global skills root via `Install/Update` → `Skills`. The
installer is driven by `src/store/skills.rs` and surfaced through
`src/app/skills_list.rs`; the flow is intentionally separate from
the per-target agent sync because the safety contract differs:

- The unit of identity is the **whole tree**: a deterministic
  SHA-256 over `(relative path, bytes)` pairs in sorted order, so
  renaming a file inside the tree changes the hash.
- Each skill's `SKILL.md` frontmatter `name:` MUST equal the
  directory name. A mismatch fails closed at plan time.
- Symlinks anywhere in the source tree (root or any nested file)
  are refused at scan time.
- `pi-psql` (and any other third-party Tools-installed skill)
  remains invisible to the skills installer unless the
  configured checkout ships a `pi-psql/` directory AND the
  manifest already records ownership. A `pi-psql` left behind by
  the Tools installer is not scanned, recursed, adopted, or
  removed.
- Conflicts (target with different bytes, target is a regular
  file, target is a symlink) are never auto-overwritten. The
  UI surfaces them as `conflict` rows; `o` is refused with an
  explanatory message.
- Owned updates use a backup + `rename_no_replace` strategy so
  the user's installed skill directory survives an interrupted
  update. We do not claim an atomic directory swap.

State compatibility: `State.installed_skills` is `#[serde(default)]`,
so older `state.json` files written before this feature shipped
load cleanly into the default empty map and the next save rebuilds
the JSON without losing existing entries.

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

`Install/Update` → `Skills` syncs the configured checkout's
`skills/` directory into the OpenCode global skills root with its
own safeguards:

- Whole-tree hash (deterministic SHA-256 over sorted
  `(relative path, bytes)` pairs). Renaming a file inside a
  skill changes the tree hash, so the next plan classifies the
  row as `update`.
- Symlinks anywhere in the source tree (root or any nested file)
  are refused at scan time. The destination side rejects
  symlinks and non-directory entries as `conflict`.
- `SKILL.md` `name:` MUST equal the directory name; mismatches
  fail closed at plan time.
- `pi-psql` (and any other third-party Tools-installed skill)
  is invisible to the skills installer unless the configured
  checkout ships a same-named directory AND the manifest already
  records ownership. The destination-side scan only ever names
  directories that are either in the source or in the manifest.
- Owned updates use a sibling backup + `rename_no_replace`
  strategy so an interrupted update restores the user's
  installed skill directory. We never claim an atomic directory
  swap.
- There is no force-overwrite path. `o` on the Skills list is
  refused with an explanatory status-bar message.

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
