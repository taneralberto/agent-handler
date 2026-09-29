# Hoja de ruta GUI — handoff (ES)

Handoff autónomo para otro agente en otra PC. `PLAN.md` y
`TOOL_INSTALLER_PLAN.md` están eliminados en el árbol de trabajo:
no se reponen ni se apuntan como referencia canónica.

## Objetivo

Añadir GUI a `agenthd` sin reescribir la base ni duplicar lógica. Sin
framework GUI hasta cerrar D1; D2–D5 se cierran en sus fases
respectivas (ver gates). TUI y `--repo` siguen siendo el
comportamiento observable hasta D2.

## Antes de empezar y estado observable hoy

- `src/main.rs` — boot, `TerminalGuard`, panic hook, ciclo TUI.
- `src/launcher.rs` — `parse_repo_override`, `resolve_checkout_path`,
  `ResolveOutcome` (sin `crossterm`/`ratatui`/`std::panic`).
- `src/workflows.rs` — capa reusable (`read_checkout`,
  `apply_checkout`, `list_canonical_agents`).
- `src/store/` — `canonical`, `settings`, `skills`, `sync`, `mod`;
  ahí vive `require_canonical_source` y los invariantes fail-closed.
- `src/app/` — render + dispatch + estado de la TUI; mezcla capas en
  flujos no extraídos.
- `REUSABLE_API_COVERAGE.md` — inventario y matriz con nombres reales
  de tests.
- `agenthd` abre la TUI por defecto; `--repo <path>` toma precedencia
  sobre `settings.json`. `workflows::read_checkout` clasifica
  `settings.json` en la rama persistida; Settings y Agents ya
  consumen workflows. `TerminalGuard` y `AgentDraft` / `EditorField`
  / `EditorMode` / `EditorOp` / `edit_prompt_externally` son
  TUI-coupled (`main.rs`, `app/editor.rs`); reusarlos exige evaluar
  caso por caso tras D1. Windows sin validar en ejecución.

## Invariantes (la GUI los respeta tal cual)

Checkout validado antes de leer o escribir; `agents/` con
`validate_checkout_path`. SHA-256 contra hash al abrir y guardar;
`save_canonical` rechaza cambios externos. Conflictos por target con
confirmación explícita; skills sin force-overwrite.
`require_canonical_source` vive en `store/` y se invoca donde toca fallar cerrado.

## Decisiones pendientes

- **D1 — Forma GUI.** Escritorio nativo (Tauri, GTK4-rs, egui, iced)
  o servidor web local. Cierra antes de fase 4.
- **D2 — `g` / `i` y `--repo`.** Cuál abre GUI/TUI/defecto; si
  `--repo` aplica a ambos; persistencia en `settings.json` o solo
  por argumento. Sin campo persistido ni subcomando `agenthd tui`
  antes de cerrar D2. Cierra antes del comando GUI de fase 5.
- **D3 — Operaciones largas.** Canal de progreso y cancelación para
  `install_tool`, `Discovery` y futuros sync/plan.
- **D4 — Empaquetado.** Arch vía PKGBUILD/AUR; otros Linux vía
  `cargo install`/`.deb`/`.rpm`/AppImage; Windows vía
  `cargo install`/winget/Scoop/MSI, o ninguno.
- **D5 — Compatibilidad `settings.json` y rutas.** Política y
  migración probadas antes de cualquier cambio; `#[serde(default)]`
  no es regla universal.

## Gates de fase

Fases 1–3 no dependen de D1–D5. Fase 4 requiere D1. Fase 5 requiere
D1 + D2 (subfases largas además D3). Fase 6 requiere D4 y validación
previa por plataforma. Cada fase deja la TUI funcionando; la GUI
entra como segundo cliente.

## Checkboxes — entregado vs. pendiente

### Fase 1 — Caracterizar comportamiento

- [x] Inventario y borrador de matriz en `REUSABLE_API_COVERAGE.md`.
- [x] `cargo test --all-targets` verde en Linux (256 + 178; 1 ignored).
- [ ] Firmar la matriz como definitiva (ver "Próxima tarea estrecha").

### Fase 2 — Separar workflows compartidos

- [x] Extraídos con tests focalizados: `read_checkout`, `apply_checkout`,
      `list_canonical_agents`. TUI migrada en `app/settings.rs` +
      `app/agents.rs`; la rama persistida reusa `workflows::read_checkout`.
- [ ] Pendientes: `install_update`, `skills_list`, `editor`, `tools`,
      dispatch en `app/mod.rs`.

### Fase 3 — Aislar el launcher

- [x] `launcher::parse_repo_override`, `resolve_checkout_path`,
      `ResolveOutcome` sin TUI/panic. Tests focales verdes: precedencia
      del override (`--repo` > `settings.json` > default), parseo
      (listo / stale / primera ejecución), y no-reescritura de
      `settings.json` cuando `--repo` iguala el valor persistido.
- [ ] Pendiente: seam CLI `agenthd tui` / `g` / `i` (espera D2).

### Fase 4 — Decidir forma GUI (D1)

- [ ] Spike por candidato; pros/contras referidos a Arch / otros
      Linux / Windows; firma del documento.

### Fase 5 — Slice Settings + Agents y expandir

- [ ] Slice vertical; requiere D1 + D2 (subfases largas además D3).

### Fase 6 — Verificar empaquetado

- [ ] Validación Arch / otros Linux / Windows en tiempo de
      ejecución (ejecución real, no solo `cargo build --target`).
- [ ] Artefactos y checklist firmado por plataforma; requiere D4.

## Próxima tarea estrecha

1. Revisar la matriz de `REUSABLE_API_COVERAGE.md` de forma
   independiente — verificar cada entrada contra el código y los
   tests reales (no contra el propio borrador); corregir
   imprecisiones (nombres, archivos, líneas, conteos, estados de
   tests); solo entonces marcarla revisada. No firmar en verde sin
   recorrer la matriz.
2. Evaluar si el boot de `main.rs` (TerminalGuard, panic hook,
   ciclo TUI, argv) debe aislarse del guard TUI-coupling; solo si
   reduce dependencia real.
3. No introducir `g` / `i` ni `--repo` GUI hasta cerrar D2.

## Validación y plataforma

- `cargo test --all-targets` — 256 + 178 ok, 1 ignored.
- `cargo clippy --all-targets --all-features -- -D warnings` —
  falla en línea base por warnings preexistentes. Métrica útil: el
  conjunto de warnings no debe crecer respecto a la línea base
  capturada (no introducir warnings nuevos).
- `cargo build --target x86_64-pc-windows-msvc` solo verifica que
  compila. "Linux validado" = solo el entorno Linux actual (probado
  como Linux genérico, no como Arch específica); Arch, otros Linux
  y Windows pendientes en máquina real (`MoveFileW`, shim
  npm-via-node, `cfg(windows)`); el README no debe declarar Arch ni
  Windows soportados hasta entonces.

## Handoff (importante)

El árbol de esta PC **no está commiteado ni pusheado**:

- Modificados (uncommitted): `src/main.rs`,
  `src/app/{mod,agents,editor,settings}.rs`.
- Sin tracking: `GUI_ROADMAP.md`, `REUSABLE_API_COVERAGE.md`,
  `src/launcher.rs`, `src/workflows.rs`. No estarán presentes en otra
  PC hasta sincronizar (commit + push aquí, pull allá, o parche).
- Eliminados (en working tree, sin commit): `PLAN.md`,
  `TOOL_INSTALLER_PLAN.md`.

No significa que estén en `origin/main`. El siguiente agente en
otra PC: `git status` + `git diff` antes de commit; no asumir
untracked en su copia; no restaurar ni commitear los eliminados
sin confirmar intención del usuario (desconocida); transferir este
doc y los untracked por parche o commit + push explícito.

> **Nota:** este snapshot del árbol proviene de esta máquina. El
> siguiente agente debe re-verificar el estado real en su propio
> árbol antes de actuar. `PLAN.md` y `TOOL_INSTALLER_PLAN.md` no
> están disponibles aquí; su eliminación exige decisión del
> usuario y no debe darse por sentada como canónica.