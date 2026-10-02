# Hoja de ruta GUI — handoff (ES)

Handoff autónomo para otro agente en otra PC.

> **Nota:** `PLAN.md` y `TOOL_INSTALLER_PLAN.md` fueron retirados
> del repositorio de forma deliberada (commit en `main`). No se
> reponen ni se apuntan como referencia canónica. Si una tarea los
> necesita, confirmar primero con el usuario.

## Objetivo

Añadir GUI a `agenthd` sin reescribir la base ni duplicar lógica. D1
cerrado por elección explícita del usuario: Tauri + Angular
(familiaridad Angular + spike ya ejecutado); D2 está cerrado (seam
CLI), D3–D5 se cierran en sus fases respectivas (ver gates). TUI y
`--repo` siguen siendo el comportamiento observable hasta que llegue
la GUI (fase 5).

## Estado actual (sesión Linux manual, breve y autoritativo)

- **Fase 5 — primer slice read-only cerrado.** Integración
  (slice "binario acompañante" + instalador
  `scripts/install.mjs`) ejecutada; **comparación paired
  GUI↔TUI sobre mismo HOME cerrada** por confirmación
  manual user-reported en sesión Linux interactiva
  (host Linux x86_64; binarios `~/.cargo/bin/agenthd` y
  `~/.cargo/bin/agenthd-gui` presentes; HOME
  `/home/lukateric`, checkout
  `/home/lukateric/dev/agent-handler`; usuario respondió
  «Sí, todo coincide» a la comparación + Refresh both,
  **no** instrumentada — sin payloads IPC, sin DOM, sin
  screenshots, sin equivalencia binarios↔código por
  timestamps). Detalle y límites en Fase 5 [x] "Comparación
  paired GUI↔TUI" y "Estado de la sesión y fuente de
  verdad" al final del documento.
- **Slice vertical de mutación pequeña síncrona —
  Settings (campo ruta absoluta + Guardar) —
  implementado y aprobado.** Backend: comando Tauri
  `apply_checkout` delega en `workflows::apply_checkout`
  vía `compose_apply_checkout` (lib como single source of
  truth; el workflow revalida internamente el path
  persistido en `settings.json`). Frontend: `<input>`
  nativo + botón `Guardar` en Settings, lock `busy` para
  TODAS las acciones públicas (refresh + save); tras un
  save exitoso el frontend dispara `settings_status` +
  `list_agents` dentro del mismo `busy`. Ver Fase 5 [x]
  "Slice vertical" abajo y
  `spikes/tauri-angular/{README.md,src-tauri/src/lib.rs,
  src/app/app.component.ts,html,css}`. **Pendiente del
  slice:** gate visual NUEVO — Save → refresh ambos →
  ver el mismo checkout y los mismos agentes en GUI y
  TUI (el gate visual read-only previo sobre el mismo
  HOME sigue cerrado por reporte manual user-reported y
  **no** es invalidado). **D3 (progreso y cancelación)
  sigue sin decidir** — no es precondición de este
  slice síncrono pequeño; **D4 / D5 siguen abiertos**.
  D3, D4, D5 siguen sin cerrar. Detalle en "Próxima tarea
  estrecha → punto 5".
- **Estado previo a publicación.** El usuario ejecuta
  `commit` + `push` por su cuenta (autorizado por el
  padre); este doc no afirma SHA futuro ni push ya
  ejecutado. **Entrega GUI prevista: 7 archivos
  GUI/docs en el commit:** `GUI_ROADMAP.md`,
  `README.md`, `spikes/tauri-angular/README.md`,
  `spikes/tauri-angular/src-tauri/src/lib.rs`,
  `spikes/tauri-angular/src/app/app.component.{ts,
  html, css}`. **Cambios locales ajenos al scope del
  commit, preservados en el worktree** (6 archivos
  modificados pero **no** subidos en ese commit):
  `agents/{lukateric,oracle,planner}.md` y
  `src/agent/starter_fixture/mod.rs` (baseline de
  los 3 modelos con `model` `gpt-6-sol` →
  `gpt-6.1-sol` y 3 `rendered_sha` refrescadas),
  `src/app/{mod,settings}.rs` (recovery-banner
  único en `path_input.error`, test que pisa Ctrl-U
  entre submit inválido y submit válido).
  Publicación: confirmar con `git log` + estado del
  remote tras `push`; no asumir worktree clean antes
  de ejecutar `commit`/`push`.

## Antes de empezar y estado observable hoy

> **Nota histórica.** Esta sección fija el estado del código
> observado al cierre del slice "binario acompañante" de Fase 5.
> Convive con secciones posteriores que documentan gates ya
> cerrados (comparación paired Linux, render suelto) y la
> distinción entre la rama actual de `agenthd gui`
> (localiza `agenthd-gui(.exe)` adyacente y reenvía argv) y el
> "rechazo" D2 original (que abortaba con exit `2` antes de
> cualquier efecto, sin escrituras). Ambos comportamientos
> siguen vivos: el rechazo se conserva como camino de fallo
> cerrado cuando el acompañante falta; ver "Decisiones
> pendientes → D2 → Estado actual".

- `src/main.rs` — boot, `TerminalGuard`, panic hook, ciclo TUI;
  rama GUI: localiza el acompañante `agenthd-gui(.exe)` adyacente
  al ejecutable y reenvía argv + exit code (abort con exit `2`
  si falta, sin escrituras).
- `src/launcher.rs` — `parse_launch` (parser CLI: `Mode`,
  `LaunchPlan`), `parse_repo_override` (lector de override usado
  por la rama de resolución/persistencia legado),
  `resolve_checkout_path`, `ResolveOutcome` (sin
  `crossterm`/`ratatui`/`std::panic`).
- `src/workflows.rs` — capa reusable (`read_checkout`,
  `apply_checkout`, `list_canonical_agents`,
  `plan_then_apply_skills`, `plan_then_apply_agents_safe`,
  `save_agent`).
- `src/store/` — `canonical`, `settings`, `skills`, `sync`, `mod`;
  ahí vive `require_canonical_source` y los invariantes fail-closed.
- `src/app/` — render + dispatch + estado de la TUI; mezcla capas en
  flujos no extraídos.
- `tests/cli_launch.rs` — integración D2: rechazo de `gui` sin
  efectos cuando el acompañante falta; con acompañante presente,
  persistencia de `--repo` con save-if-changed, no-rewrite
  cuando el valor no cambia, y `--repo` inválido falla cerrado.
- `REUSABLE_API_COVERAGE.md` — inventario y matriz con nombres reales
  de tests.
- `agenthd` abre la TUI por defecto; `agenthd tui` es explícito;
  `agenthd gui` localiza el acompañante adyacente y reenvía
  (abort con exit `2` y stderr claro si falta, antes de tocar
  `settings.json`). `--repo <path>` toma precedencia sobre
  `settings.json` y aplica a ambas modalidades; persiste cuando
  la modalidad existe (TUI hoy; GUI cuando el acompañante está
  presente por la misma rama save-if-changed de
  `resolve_checkout_path`). `workflows::read_checkout` clasifica
  `settings.json` en la rama persistida; Settings y Agents ya
  consumen workflows. `TerminalGuard` y `AgentDraft` /
  `EditorField` / `EditorMode` / `EditorOp` /
  `edit_prompt_externally` son TUI-coupled (`main.rs`,
  `app/editor.rs`); reusarlos exige evaluar caso por caso.
  **Windows 11 MSYS:** hay un **smoke parcial** registrado
  (arranque de `npm run tauri dev`, dev server en
  `127.0.0.1:14720`, compilación, lanzamiento del binario y
  `MainWindowHandle` observado vía `Get-Process` con título
  `agenthd spike (D1 / Tauri + Angular)`) — **no** equivale a
  ejecución validada del runtime gráfico ni del IPC end-to-end.
  Ver `spikes/tauri-angular/EVIDENCE.md` → "Observaciones
  Windows 11 MSYS" y "Limitaciones honestas". Empaquetado
  cross-platform (Arch / otros Linux / Windows) sigue siendo
  gate de Fase 6, D4 pendiente.

## Invariantes (la GUI los respeta tal cual)

Checkout validado antes de leer o escribir; `agents/` con
`validate_checkout_path`. SHA-256 contra hash al abrir y guardar;
`save_canonical` rechaza cambios externos. Conflictos por target con
confirmación explícita; skills sin force-overwrite.
`require_canonical_source` vive en `store/` y se invoca donde toca fallar cerrado.

## Decisiones pendientes

- **D1 — Forma GUI.**

  > **D1 cerrado por elección explícita del usuario.** Se mantiene
  > **Tauri + Angular** por familiaridad del usuario con Angular y
  > por la existencia del spike ya ejecutado
  > (`spikes/tauri-angular/`, etapas 1 y 2 — ver "Fase 4" y
  > "Próxima tarea estrecha"). **No** se afirma una comparación
  > factual entre Tauri, GTK4-rs, egui, iced y servidor web local.
  > Quedan dos frentes de validación gráfica, **distintos** entre
  > sí: (i) IPC visual (ventana nativa + JS↔Rust con CSP y
  > `127.0.0.1:14720`), que es **gate del primer slice read-only
  > de Fase 5** (ver punto 5); (ii) empaquetado cross-platform en
  > Arch / otros Linux / Windows, que es **gate de Fase 6** (D4
  > pendiente). D1 no validaba ninguno de los dos; era solo la
  > elección del framework. **Nota:** el smoke parcial de
  > Windows 11 MSYS registrado en el spike (window handle
  > observado vía `Get-Process`, sin contenido Angular ni IPC
  > observables) **no** cambia este estado — sigue siendo
  > smoke, no validación end-to-end del runtime gráfico.
- **D2 — `g` / `i` y `--repo`.** Cuál abre GUI/TUI/defecto; si
  `--repo` aplica a ambos; persistencia en `settings.json` o solo
  por argumento. Sin campo persistido ni subcomando `agenthd tui`
  antes de cerrar D2. Cierra antes del comando GUI de fase 5.

  > **D2 cerrado.** `agenthd` sin args y `agenthd tui` abren la
  > TUI; `agenthd gui` rechaza con exit code `2` y stderr claro
  > *antes* de cualquier efecto (sin escribir `settings.json`,
  > `state.json`, ni los árboles OpenCode/Pi). `--repo <abs>`
  > aplica a ambas modalidades y persiste igual que hoy cuando la
  > modalidad existe (TUI hoy; GUI aún no implementada). El parser
  > CLI es `src/launcher.rs::parse_launch` (modo + valor de
  > `--repo`); la rama de resolución/persistencia del override
  > sigue re-parseando argv con `parse_repo_override` dentro de
  > `resolve_checkout_path` — contrato legado
  > (save-if-changed, primer `--repo` gana, duplicados) intacto
  > para la TUI. `parse_launch` tolera posicionales desconocidos
  > y rechaza el valor `tui`/`gui` para `--repo` para que el orden
  > `--repo path` / modo (en cualquiera de los dos órdenes) nunca
  > confunda valor por modo. Tests focales en `src/launcher.rs`
  > (`parse_launch_*`) y de integración en `tests/cli_launch.rs`
  > (`agenthd_gui_*`) verifican el contrato sin efectos.
  >
  > **Estado actual (Fase 5, slice "binario acompañante"):** el
  > "rechazo" documentado arriba es **histórico**; la rama actual
  > de `agenthd gui` localiza el binario `agenthd-gui` adyacente
  > al ejecutable y reenvía argv + exit code (abort con exit `2`
  > si falta, sin escrituras). El parser CLI y el contrato D2 no
  > cambiaron.
- **D3 — Operaciones largas.** Canal de progreso y cancelación para
  `install_tool`, `Discovery` y futuros sync/plan.
- **D4 — Empaquetado.** Arch vía PKGBUILD/AUR; otros Linux vía
  `cargo install`/`.deb`/`.rpm`/AppImage; Windows vía
  `cargo install`/winget/Scoop/MSI, o ninguno.
- **D5 — Compatibilidad `settings.json` y rutas.** Política y
  migración probadas antes de cualquier cambio; `#[serde(default)]`
  no es regla universal.

## Gates de fase

Fases 1–3 no dependen de D1–D5. Fase 4 cerrada por elección
explícita del usuario (Tauri + Angular); ver "Decisiones
pendientes → D1". Fase 5 requiere D2; las **operaciones
largas** del slice vertical (no el read-only ni la
mutación pequeña síncrona) requieren además D3. Fase
6 requiere D4 y validación previa por plataforma. Cada
fase deja la TUI funcionando; la GUI entra como
segundo cliente.

## Checkboxes — entregado vs. pendiente

### Fase 1 — Caracterizar comportamiento

- [x] Inventario y borrador de matriz en `REUSABLE_API_COVERAGE.md`.
- [x] `cargo test --all-targets` verde en Linux (256 + 178; 1 ignored).
- [x] Firmar la matriz como definitiva (ver "Próxima tarea estrecha").

### Fase 2 — Separar workflows compartidos

- [x] Extraídos con tests focalizados: `read_checkout`, `apply_checkout`,
      `list_canonical_agents`. TUI migrada en `app/settings.rs` +
      `app/agents.rs`; la rama persistida reusa `workflows::read_checkout`.
- [x] **Skills-install orchestration** (esta extracción):
      `workflows::plan_then_apply_skills(&Paths, State) ->
      Result<Option<(State, Vec<SkillOutcome>)>>`. TUI migrada en
      `app/skills_list.rs::apply_skills_install`: la recarga de
      `State` sigue en la TUI; la replanificación desde disco, el
      no-op si plan vacío y la aplicación sin force-overwrite
      viven en el workflow. UI mantiene estado / status / teclas /
      render sin cambios visibles; `self.state` queda actualizado
      con la recarga aunque la planificación falle. Tests focales
      en `workflows.rs` (`plan_then_apply_skills_*`) cubren empty
      no-op / no write, replan desde disco frente a `State`
      stale, y conflicto sin overwrite.
- [x] **Per-target agent safe-install orchestration**
      (esta extracción): `workflows::plan_then_apply_agents_safe(
      &Paths, State, SyncTarget) ->
      Result<Option<(State, Vec<ApplyOutcome>)>>` (firma espejo
      de `plan_then_apply_skills`: target concreto, sin
      precondiciones de UI en el contrato). TUI migrada en
      `app/install_update.rs::apply_safe_install`: la recarga de
      `State`, la asignación de `self.state`, y el wiring post-
      apply (`last_outcomes`, `status`, `refresh_install_update`,
      `status_bar`) siguen en la TUI; el guard de target bound
      vive en el TUI y, en la rama sin target, llama a
      `load_canonical` una vez para preservar el orden del
      observable original (`load_canonical` antes del mensaje
      `pick a harness first`, igual que la versión inline
      previa). En la ruta normal (target bound) el workflow es
      el único dueño de `load_canonical` — no hay duplicación.
      La replanificación per-target vía `plan_for`, el no-op si
      plan vacío y la `apply_safe` si no vacío viven en el
      workflow. `force_overwrite` se queda en la TUI
      reutilizando `store::force_install` (espejo del path
      seguro, sin extracción: el seam reusable es la
      orquestación segura, no el force). UI sin cambios
      visibles: los mensajes `error: ...`, `nothing to install
      for <target>`, `safe install <target>: N ok, M failed`,
      y `pick a harness first` se conservan bit-for-bit.
      Tests focales en `workflows.rs`
      (`plan_then_apply_agents_safe_*`) cubren empty no-op /
      no write, replan desde disco frente a `State` stale,
      scope per-target (OpenCode vs Pi sin cruces), y fail-
      closed ante fuente canónica ausente. Tests TUI directos
      en `app::tests::apply_safe_install_*` cubren el round-
      trip del handler: `error: ...` cuando source ausente + sin
      target, y `pick a harness first` cuando source válido +
      sin target (fijan el orden observable
      `load_canonical → guard target`).
- [x] **Editor save helper** (esta extracción):
      `workflows::save_agent(&Paths, Option<String>,
      Option<String>, Agent) -> Result<String>`. TUI migrada
      en `app/editor.rs::apply_editor_op` para el brazo
      `EditorOp::Save`: la llamada pasa de la free fn
      `save_agent` inline al workflow reusable con cuerpo y
      orden preservados bit-for-bit (hash previo del source
      antes del rename, `rename_canonical` con prefijo
      `"rename: "`, `save_canonical` con el `prior_hash`
      original y prefijo `"save: "`). Sin cambio de contrato
      visible: `App::apply_editor_op` sigue clonando el prior
      hash, mapeando `Ok(name)` a `"saved \`<name>\`"` y
      limpiando `editor_draft` / `editor_original_name` /
      `editor_prior_hash`, y `Err(e)` a `"error: {e}"`.
      `AgentDraft`, `EditorField`, `EditorMode`, `EditorOp`,
      el model picker y `edit_prompt_externally` se quedan en
      `app/editor.rs` — son TUI-coupled y no se trasladan.
      Comportamiento **no** atómico a través del rename +
      save preservado tal cual: un rename exitoso seguido de
      un `save_canonical` fallido deja el source renombrado
      con los bytes previos; el workflow no compensa con un
      rename-back. Tests focales en `workflows.rs`
      (`save_agent_*`) cubren alta, update, rename, fuente
      stale sin mover, y destino ocupado. Los tests TUI
      previos del editor (`save_*`, `editor_*`) siguen
      verdes sin cambios — la sustitución es transparente.
- [x] **Tools screen — evaluado UI-only (sin extracción).**
      `crate::tools::{DEFAULT_CATALOG, tool_status,
      install_tool}` ya ofrece la API reusable completa que
      `app/tools.rs` consume directamente. `install_tool`
      tiene además un seam inyectable
      (`install_tool_with` / `install_tool_at` con `spawn` /
      `rename` runners) cubierto por `tools::tests`. No hay
      orquestación nueva que extraer; mover el dispatch /
      `Screen::Tools` / selección de filas a `workflows.rs`
      ensuciaría el workflow con tipos UI-specific
      (`Screen::Tools { installing, .. }`, `KeyCode`) sin
      valor reusable para una futura GUI. `tools_screen_install_target`
      es un helper UI trivial (clamp `selected` dentro de
      `entries`) y se queda en `app/tools.rs`. 9 tests TUI
      directos en `app::tests::tools_*` +
      `app::tests::ctrl_c_from_tools_screen_*` fijan: refresh
      catálogo → entradas `NotInstalled` sin preflight ni
      red; status filesystem per-entry (directorio → `Installed`,
      archivo → `Conflict`); `tools_screen_install_target`
      valid/invalid; tecla `i` con `selected` fuera de
      rango → no-op (no llega a `install_tool`); bloqueo de
      teclas mientras `installing = true`; Press-vs-Release
      desde la pantalla Tools; Ctrl-C desde Tools → screen
      sin cambio y sin error.
- [x] **Dispatch en `app/mod.rs` — evaluado UI-only (sin
      extracción).** `handle_event` (filtra `KeyEventKind::Press`)
      y `handle_key` (chequea Ctrl-C antes del `match` sobre
      `Screen`) son UI puro: el `match` sobre variantes de
      `Screen` enruta al handler correcto. No hay workflow
      reusable ni orquestación a extraer; un wrapper en
      `workflows.rs` necesitaría acceso a `Screen`, `KeyEvent`
      y al `App` para mutar `self.screen`, lo que mueve UI
      al seam reusable sin beneficio. Los 9 tests TUI de la
      fila anterior cubren el dispatch desde la pantalla
      Tools; el resto del dispatch sigue cubierto por los
      tests TUI previos (`main_menu_index_to_screen_mapping_is_pinned`
      y otros).

**Fase 2 cerrada:** la lógica compartible pendiente está
toda extraída (`plan_then_apply_skills`,
`plan_then_apply_agents_safe`, `save_agent`) o evaluada
explícitamente como UI-only (`tools`, dispatch). D1
(forma GUI) se cerró después por elección explícita del
usuario — ver "Decisiones pendientes → D1" y "Fase 4".

### Fase 3 — Aislar el launcher

- [x] `launcher::parse_repo_override`, `resolve_checkout_path`,
      `ResolveOutcome` sin TUI/panic. Tests focales verdes: precedencia
      del override (`--repo` > `settings.json` > default), parseo
      (listo / stale / primera ejecución), y no-reescritura de
      `settings.json` cuando `--repo` iguala el valor persistido.
- [x] Seam CLI `tui` / `gui` con D2 cerrado: `agenthd` y
      `agenthd tui` abren TUI; `agenthd gui` rechaza antes de
      efectos con exit code `2`. Parser CLI en
      `launcher::parse_launch` (`Mode`, `LaunchPlan`); la rama de
      resolución/persistencia del override re-parsea argv vía
      `parse_repo_override` dentro de `resolve_checkout_path`
      (contrato legado intacto: save-if-changed, primer `--repo`
      gana). `--repo` aplica a ambas modalidades y persiste cuando
      la modalidad existe. Posicionales desconocidos se ignoran; el
      valor `--repo tui`/`--repo gui` se rechaza para no confundir
      valor y modo en ningún orden; duplicados de `--repo`
      mantienen el contrato legado (primer `--repo` gana,
      idéntico a `parse_repo_override`). Sin campo nuevo en
      `settings.json`, sin movimiento de `TerminalGuard` / panic
      hook. Tests focales en `launcher.rs` y de integración en
      `tests/cli_launch.rs`. Conteo actualizado en "Validación y
      plataforma".

### Fase 4 — Decidir forma GUI (D1)

- [x] Spike Tauri + Angular del candidato implementado
      (`spikes/tauri-angular/`): seam `lib` en la raíz
      (single source of truth, sin fixtures en producción),
      dos comandos read-only (`compose_settings_status`,
      `compose_agents_list`) declarados como wrappers de un
      solo nivel sobre `workflows::read_checkout` y
      `workflows::list_canonical_agents`, build y tests
      verdes del spike (`npm ci`, `npm run build`,
      `cargo build`, `cargo test --lib` 8/8 ok, `cargo
      clippy --lib --tests` y `cargo fmt --check` limpios),
      fix de port/colisión IPv4/IPv6 a `127.0.0.1:14720`
      documentado en `spikes/tauri-angular/EVIDENCE.md`
      ("Diagnóstico: colisión IPv4/IPv6 en 1420 y migración
      a 14720"). Limitaciones runtime honestas conservadas:
      **no hay evidencia automatizada de visualización ni
      de IPC end-to-end** (intento de ejecución con
      `DISPLAY=:0` + `timeout 12s` → exit 124 sin stderr;
      los tests del backend invocan los composition helpers,
      no el dispatch Tauri en runtime). **Smoke Windows
      11 MSYS posterior:** `cargo test --lib` 8/8 ok,
      `npm run build` ok (116.20 kB initial / 34.81 kB
      transfer), `npm run tauri dev` arrancó y
      `Get-Process` observó un `MainWindowHandle` con
      título `agenthd spike (D1 / Tauri + Angular)` a
      los 12 s (PID 11624), run timed interrumpido a
      55 s (exit 143) sin contenido Angular ni IPC
      observables — **sigue sin ser** visualización ni
      IPC end-to-end. Ver
      `spikes/tauri-angular/EVIDENCE.md` → "Observaciones
      Windows 11 MSYS". Detalle completo en "Próxima
      tarea estrecha → Spike etapa 2 ejecutado".
- [x] **D1 cerrado por elección explícita del usuario:**
      Tauri + Angular. La decisión se basa en (a) la
      familiaridad del usuario con Angular y (b) el spike
      ya ejecutado (`spikes/tauri-angular/`, etapas 1 y 2).
      **No** se afirma comparación factual con GTK4-rs /
      egui / iced / servidor web local. Quedan dos frentes
      de validación gráfica, **distintos** entre sí: (i)
      IPC visual (ventana nativa + JS↔Rust con CSP y
      `127.0.0.1:14720`), gate del primer slice read-only
      de Fase 5 (ver punto 5); (ii) empaquetado
      cross-platform Arch / otros Linux / Windows, gate
      de Fase 6 (D4 pendiente). D1 no validaba ninguno de
      los dos; era solo la elección del framework. La
      **base del primer slice read-only Settings + Agents
      (Fase 5)** ya está ejecutada: el slice "binario
      acompañante" (Fase 5 [x]) localiza el companion
      `agenthd-gui(.exe)` adyacente al CLI, aborta con
      exit `2` sin escrituras cuando falta, y cuando está
      presente reenvía argv + exit code al acompañante
      preservando la rama TUI. El contrato de
      instalación "emparejada" (CLI + GUI como dos
      crates separados) está documentado en `README.md`
      → "Paired install contract (CLI + companion GUI)".
      Lo que queda pendiente del primer slice read-only
      **no** es la integración: es la **validación por
      el usuario** del binario acompañante **contra la
      configuración real compartida con la TUI** (mismo
      `HOME`, sin `--repo`, sin aislamiento de `HOME`),
      para confirmar que los paneles Settings y Agents
      reflejan el mismo estado que la TUI observa hoy.
      Esa validación es la próxima tarea estrecha (ver
      punto 5); **no** se afirma contenido Angular ni
      IPC end-to-end del binario de producción — lo
      único observado a nivel de paired build es el
      handle / título del proceso acompañante tras
      `cargo install` en raíz temporal preaprobada (ver
      "Validación y plataforma"). La confirmación
      manual user-reported del spike en `tauri dev`
      (Settings + Agents + Refresh both) **sigue
      siendo evidencia aparte y separada**: es del
      spike en modo dev, no del binario de
      producción. Gates de Fase 5: el primer slice
      read-only depende solo de D2 (D3 no le aplica —
      D3 bloquea las **operaciones largas** que lo
      extiendan; la mutación pequeña síncrona Settings
      tampoco exige D3); D1 ya está cerrado. D3 / D4 / D5
      siguen
      pendientes; el empaquetado cross-platform (Arch
      / otros Linux / Windows) sigue siendo gate
      separado de Fase 6 (D4 pendiente).

### Fase 5 — Slice Settings + Agents y expandir

- [x] **Instalador fuente (orquestadora `scripts/install.mjs`).**
  Wrapper del proceso manual de "Paired install contract" en
  `README.md`: preflight `cargo` / `npm` / `node`, valida que
  `spikes/tauri-angular/{package.json,package-lock.json}` y
  `spikes/tauri-angular/src-tauri/Cargo.toml` existan, corre
  `npm ci --include=dev` + `npm run build`, confirma
  `dist/agenthd-tauri-angular-spike/browser/index.html`, y
  ejecuta **dos** `cargo install` desde el repo root con el
  mismo `--root`. CLI: `--locked --bin agenthd` (la GUI no se
  build-ea aquí). GUI: `--locked --bin agenthd-gui
  --features custom-protocol`. El `--root` se resuelve **una
  vez** (precedencia `--root > CARGO_INSTALL_ROOT >
  CARGO_HOME > $HOME/.cargo`, cwd-relative si el usuario pasó
  un path relativo) y se pasa idéntico a ambos `cargo install`.
  En Windows, npm se invoca vía `cmd /c npm ...args` con argv
  fijo y nunca se interpola el `--root` en el shell; cargo
  se lanza con `spawn(cmd, args)` sin shell. Soporta
  `--force` (forward a ambos `cargo install`, **no**
  siempre). El instalador aborta ante cualquier exit no-cero
  con `phase` / `path` / `code` claros y nunca escribe
  `settings.json` ni lanza el binario; **no** se promete
  atomicidad. Documenta **deliberadamente** que
  `[install].root` en `Cargo.toml` **no** se respeta (el
  usuario debe `--root` para alinearlo). CLI guard con
  `pathToFileURL(process.argv[1]).href === import.meta.url`
  para que los tests importen sin auto-ejecutar; `main`,
  `parseArgs`, `resolveInstallRoot`, `buildPlan`, `run`,
  `HELP_TEXT` exportados. **29 tests** (`node --test
  scripts/install.test.mjs`, **padre re-run PASS**)
  sin dependencias externas: parser strict, precedencia
  `--root`, orden del plan, flags en ambos installs,
  `--force` opcional, feature `custom-protocol` solo en GUI,
  dispatch Windows npm sin interpolar root + fallback
  `ComSpec`, dispatch Linux npm directo + `shell:false`,
  cargo siempre directo + `shell:false`, aborts
  npm / preflight / CLI / GUI sin éxito falso, `--help`
  sin invocar herramientas, errores del parser abortan
  código 1 sin tools. Ver bloque "Validación y plataforma →
  Instalador fuente" para el detalle de la corrida real.
  **Esta orquestadora NO cierra D4** (empaquetado Arch /
  otros Linux / Windows) ni sustituye el gate visual de Fase
  5 (ver "Decisiones pendientes → D2 → Estado actual" y
  "Defecto user-reported y fix acotado" para los gates
  visuales pendientes y sus límites). Es solo un
  instalador fuente: el slice ya estaba aprobado.

- [x] **Binario acompañante (slice read-only, fase 5 — primer
      paso).** El CLI `agenthd` localiza el binario
      `agenthd-gui` (o `agenthd-gui.exe` en Windows) adyacente
      al ejecutable actual; si falta, aborta con exit `2` y un
      mensaje de stderr que nombra el archivo faltante **antes**
      de que `resolve_checkout_path` corra (y por tanto antes
      de cualquier escritura de `settings.json`). Cuando está
      presente, `resolve_checkout_path` se ejecuta igual que en
      la TUI (precedencia de `--repo`, save-if-changed,
      `save_settings` ya crea el padre), y luego el CLI hace
      `Command::new(companion).args(args).status()` con el env
      heredado naturalmente — el código de salida del
      acompañante se reenvía al llamador. La rama GUI no llama
      `ensure_dirs` ni instala `TerminalGuard` / panic hook:
      el acompañante posee su propia superficie de ventana. PATH
      **no** se consulta: solo el nombre exacto adyacente al
      ejecutable actual. El contrato de instalación
      "emparejada" (CLI + GUI como dos crates separados, sin
      segundo copiado del spike) está documentado en el
      `README.md`. La rama TUI / default queda **intacta**:
      mismas pruebas, mismo contrato D2, mismo comportamiento
      observable. Tests focales nuevos en
      `src/main.rs::tests` (4: locator puro contra
      `current_exe` inyectado, exact-sufijo de plataforma
      única — sin fallback a otras extensiones —,
      no-match de archivos adyacentes no-companion, y
      spawn-and-wait con proceso falso en `TempDir` —
      `cmd.exe` renombrado en Windows, script shebang en
      Unix); tests de integración actualizados en
      `tests/cli_launch.rs` (5: 2 de rechazo con mensaje
      compañero-ausente sobre copia aislada del binario en
      `TempDir` — el suite ya no asume que `target/debug`
      está limpio; un `agenthd-gui(.exe)` adyacente al bin
      de producción no rompe los tests — más 3 de
      companion-present: persistencia de `--repo` con
      `settings.json` escrito y `state.json` / OpenCode / Pi
      target trees no escritos; no-rewrite cuando `--repo`
      iguala el valor persistido (la rama
      `save-if-changed` en `resolve_checkout_path` mantiene
      sus bytes, ahora ejercitada también en integración);
      y `--repo` inválido falla cerrado sin `settings.json`).
      El test unitario de no-rewrite para `--repo` igual al
      valor persistido se sigue cubriendo en `launcher::tests
      ::resolve_repo_override_does_not_rewrite_settings_when_unchanged`
      (independiente de plataforma) en paralelo a la nueva
      cobertura de integración. El spike
      `spikes/tauri-angular/src-tauri/` renombra su binario a
      `agenthd-gui` vía `[[bin]]` y baja el branding "spike"
      en `productName` / `identifier` / título de ventana /
      `<h1>` Angular; comandos, capabilities, CSP y
      comportamiento read-only **sin cambios**. Sin campos
      nuevos en `settings.json`, sin permisos nuevos, sin
      escrituras nuevas desde la GUI, sin D3/D4/D5 tocados.
      Ver `README.md` → "Paired install contract (CLI +
      companion GUI)" y "Modes (TUI today, GUI planned)"
      para los detalles exactos y el contrato observable.
- [x] **Comparación paired GUI↔TUI sobre mismo HOME
      (gate visual del primer slice read-only).**
      Confirmación manual user-reported en sesión Linux
      interactiva (no instrumentada por el agente): padre
      lanzó `~/.cargo/bin/agenthd gui` desde la raíz del
      repo (cwd = repo root, sin `--repo`, sin aislamiento
      de `HOME`); usuario reportó explícitamente
      «Sí, todo coincide» a «¿La GUI abierta muestra el
      mismo checkout y los mismos agentes que la TUI, y
      «Refresh both» actualiza sin errores?». Se indicó
      abrir `~/.cargo/bin/agenthd tui` en otra terminal
      para la comparación lado-a-lado. Conclusión honesta:
      el gate visual del primer slice read-only de Fase 5
      **queda cerrado por reporte manual user-reported**,
      con los mismos límites ya advertidos para la
      confirmación previa del spike en `tauri dev` (no
      instrumentación: **no** se capturaron payloads IPC,
      **no** se inspeccionaron CSP / devtools / red, **no**
      se inspeccionó el DOM, **no** se tomaron screenshots,
      **no** se ejecutó el runtime empaquetado en otras
      plataformas, **no** se afirma equivalencia entre los
      binarios en `~/.cargo/bin/` y el código local por
      timestamps — los binarios de hoy no se certifican
      contra el árbol actual). Contexto manual reportado:
      host Linux x86_64 con binarios `agenthd` y
      `agenthd-gui` presentes en `~/.cargo/bin/`; HOME
      `/home/lukateric` y checkout
      `/home/lukateric/dev/agent-handler`. D3 previo al
      cierre de las **operaciones largas** del slice
      vertical (no afecta a esta confirmación read-only);
      D4 y D5 siguen pendientes como gates separados.
      Empaquetado Arch / otros Linux / Windows sigue
      siendo gate de Fase 6 (D4 pendiente), no cubierto
      por esta confirmación.
- [x] **Slice vertical — primer slice de mutación pequeña
      síncrona (GUI Settings: campo ruta absoluta + Guardar,
      validación/persistencia, refresco Settings + Agents).**
      Aprobado por el usuario; sync pequeño síncrono no
      requiere D3. `compose_apply_checkout(&Paths, &str) ->
      Result<String, String>` clona `Paths` y delega una sola
      llamada a `workflows::apply_checkout` (alias
      `apply_checkout_workflow` para evitar colisión con el
      nombre del comando Tauri), devuelve el path aplicado en
      éxito o el literal `ApplyError::message()` en fallo (sin
      copiar validación, sin las seis variants de error — el
      lib es single source of truth). Comando Tauri
      `apply_checkout(checkout_path: String) ->
      Result<String, String>` resuelve `Paths::from_env()`
      con `map_err` y reenvía la `Result` del helper; el
      frontend registra `invoke<string>("apply_checkout", {
      checkoutPath })` (camelCase) y trata el `Err` como
      rejection literal. Sin cambios de schema, sin permisos
      nuevos, capabilities intactas, CSP intacta. **Tests
      focales:** `cargo test --locked --lib` 16/16 OK
      (8 previos + 8 nuevos: valid trim persiste + recomponer
      Settings/Agents, cambio entre dos checkouts con agentes
      distintos, vacío / relativo / no-existe / sin-`agents` /
      escritura-falla conservan `settings.json` byte-exact y
      rutas originales, symlink `#[cfg(unix)]` rechaza);
      `--features custom-protocol` 16/16 OK; `cargo clippy
      --locked --lib --tests [--features custom-protocol]`
      sin warnings nuevos vs baseline; `cargo fmt --check` y
      `npm run build` limpios. **Frontend:** `checkoutPath`
      signal inicializado una vez (Ready/Stale), `busy` lock
      para TODAS las acciones públicas (refresh + save) antes
      de `await` y en `finally`; `saveCheckout` dispara el
      `invoke("apply_checkout", ...)` y, **después** del
      éxito del workflow (que ya revalida internamente el
      path persistido en `settings.json`), el frontend
      refresca `settings_status` + `list_agents` dentro del
      mismo `busy`. La revalidación post-write **vive en
      `workflows::apply_checkout`** (lib como single source
      of truth) — el frontend **no** revalida por su
      cuenta, solo dispara los dos `invoke` read-only
      posteriores. Si el save tuvo éxito pero el refresh
      posterior falla, el lib ya persistió el path y el
      frontend muestra el error de refresh sin enmascarar
      `saveError`; no hay banner verde de éxito. Binding
      nativo `<input (input)>` sin FormsModule; input
      deshabilitado solo durante `saving` (teclear durante
      el refresh inicial está permitido y `draftSeeded`
      protege el buffer); botones deshabilitados solo con
      `busy` (no se bloquea por buffer vacío — esa
      restricción es del flujo TUI, no del flujo GUI);
      css mínimo aditivo (`.path-editor`). **Gate
      visual:** NUEVO, pendiente — Save → refresh ambos →
      ver el mismo checkout y los mismos agentes en GUI y
      TUI. Gate read-only previo (Fase 5 [x] "Comparación
      paired GUI↔TUI sobre mismo HOME") sigue cerrado por
      reporte manual user-reported y no es invalidado por
      este slice. **D3 sigue sin decidir** (no precondición
      de este slice); **D4 / D5 siguen abiertos**. Sin
      `commit` / `push`, sin mutación de `HOME`, sin instalar
      dependencias del host. Logs en `/tmp/opencode/`.
- [ ] Slice vertical (operaciones largas requieren D3;
      mutación pequeña síncrona no): `sync` masivo,
      `install_tool`, `agent editor`, `skills`. D3 SOLO
      es precondición de las **operaciones largas**
      (`install_tool`, `Discovery`, `sync` / `plan`
      masivos); un editor síncrono pequeño vía wrapper
      Angular **no** requiere D3 — el camino concreto
      ya está ejecutado y aprobado por el usuario en el
      slice "Settings (ruta + Guardar)" (Fase 5 [x]
      "Slice vertical"). Próximo paso (si procede): un
      nuevo slice de mutación — el que sea, **no** se
      decide en este handoff.

### Fase 6 — Verificar empaquetado

- [ ] Validación Arch / otros Linux / Windows en tiempo de
      ejecución (ejecución real, no solo `cargo build --target`).
- [ ] Artefactos y checklist firmado por plataforma; requiere D4.

## Próxima tarea estrecha

1. **Fase 1 firmada.** Recorrida read-only de la matriz con
   reclasificaciones puntuales aplicadas en `REUSABLE_API_COVERAGE.md`
   (Summary ampliado a listado exhaustivo). Nombres de `Cross-cutting
   invariants` comprobados estáticamente contra el código, no fila
   por fila. Revisión estática en Windows; conteos históricos de
   tests Linux (256 + 178; 1 ignored) no re-ejecutados.
2. **(Evaluada — sin extracción.)** `launcher.rs` ya aísla el parseo de argv
   y la resolución del checkout respecto a la TUI; `main.rs` lee `env::args`
   como entrada genérica, y mantiene `TerminalGuard`, panic hook y ciclo
   de `ratatui` como TUI-específicos. Moverlos no reduce dependencias
   reales de cara a la GUI. Bloques de terminal duplicados: limpieza TUI
   aparte, no prerrequisito de GUI; reevaluar tras D1.
3. **D2 cerrado.** `launcher::parse_launch` es el parser de la
   superficie CLI; `gui` rechaza antes de cualquier efecto;
   `--repo` aplica a ambas modalidades y persiste cuando la
   modalidad existe. La rama de resolución/persistencia del
   override sigue re-parseando argv vía `parse_repo_override`
   (contrato legado: save-if-changed, primer `--repo` gana).
   Ver "Decisiones pendientes → D2".
4. **Acción siguiente (Fase 4 cerrada / D1 cerrado).** Fase 2
   cerrada: `plan_then_apply_skills`,
   `plan_then_apply_agents_safe`, `save_agent` extraídos a
   `workflows.rs`; `tools` y dispatch en `app/mod.rs`
   evaluados UI-only y no extraíbles (documentado arriba).
   El spike de Tauri + Angular está **ejecutado** (etapas
   1 y 2, ver más abajo) y **D1 (forma GUI) está cerrado
   por elección explícita del usuario** — Tauri + Angular
   por familiaridad del usuario con Angular y por la
   existencia del spike; **no** se afirma comparación
   factual con GTK4-rs / egui / iced / servidor web local.
   Sin cambios de contrato CLI/GUI; `gui` sigue siendo
   solo la rama de rechazo (este estado corresponde a
   la fecha del spike etapa 2 — el slice "binario
   acompañante" de Fase 5 [x] y la confirmación paired
   Linux de Fase 5 [x] ocurrieron **después**; ver
   "Estado actual" arriba). Fase 3 sigue cerrada
   (launcher aislado, D2 sellado). Próxima tarea
   estrecha: ver punto 5.

   **Spike etapa 1 ejecutado: seam `lib` creado (single
   source of truth, sin fixtures en producción).**
   Pre-espacio de Fase 4: se introdujo `src/lib.rs`
   declarando como `pub mod` los módulos UI-independientes
   (`agent`, `launcher`, `models`, `store`, `tools`,
   `workflows`) para que un futuro crate Tauri pueda
   depender de `agenthd` por path. El bin (`src/main.rs`)
   NO redeclara esos módulos con `mod` — los importa una
   sola vez vía
   `pub use agenthd::{agent, launcher, models, store, tools,
   workflows};`, de modo que `crate::store` / `crate::
   workflows` / etc. dentro de `app/` resuelve al módulo
   del lib y no duplica el cuerpo. El módulo `app`
   (TUI-específico, `ratatui`/`crossterm`) queda declarado
   localmente con `mod app;` y no entra a la superficie
   pública del lib. Sin nuevas dependencias en
   `Cargo.toml` (cero Tauri).

   **Fixture `STARTERS` / `starter_agent` no entra al
   binario de producción.** Permanece detrás de
   `#[cfg(test)]` en `src/agent.rs` (gate de lib). Como
   `cfg(test)` no se propaga entre crates, el bin en modo
   test no ve los símbolos `#[cfg(test)]` del lib; para
   que `src/app/mod.rs::tests` pueda usarlos,
   `src/main.rs` re-incluye el módulo bajo `#[cfg(test)]`
   con `#[path = "agent/starter_fixture/mod.rs"] mod
   starter_fixture;` — la fuente es la misma (single source
   of truth para el markdown), y el gating está en ambos
   lados, así que producción no incluye ningún byte de los
   `agents/*.md`. Verificado: `strings target/debug/agenthd
   | grep -c '^---$'` → `0`; las dos pruebas de búsqueda
   por strings únicos de los markdown (frontmatter
   `---`, frases como "planner breaks tasks") devuelven
   vacío. El precio es una duplicación **aceptable de tests
   de fixture** (2 tests del fixture corren una vez en el
   lib y otra en el bin — solo `starter_registry_*`), que
   el task autoriza explícitamente como trade-off para
   mantener producción limpia. El resto de tests del lib
   ya NO se duplican. `src/app/mod.rs::tests` cambió
   SOLO esos dos imports a `crate::starter_fixture::{...}`
   (sin reescrituras a mayor escala).

   Validación corrida: `cargo fmt --check` limpio;
   `cargo test --all-targets` — **233 (lib) + 61 (bin) +
   2 (cli_launch integ) + 178 (tools_install_smoke) = 474
   ok, 1 ignored** (los 2 tests del fixture corren 2x: una
   en el lib, otra en el bin; los demás tests no
   duplicados); `cargo test --test cli_launch` 2/2 ok
   (rechazo `gui` y `gui --repo` sin efectos colaterales,
   contrato CLI intacto); `cargo clippy --all-targets` sin
   warnings nuevos (diff textual vs baseline: vacío).
   **Este 474 es la snapshot del root crate en Linux
   al cierre del spike etapa 1, antes del slice
   "binario acompañante" de Fase 5; el conteo
   post-slice actual del root crate es 491 ok
   (Windows 11 MSYS — ver "Validación y plataforma").
   El conteo Linux post-slice no se re-ejecutó.**

   Cambio API mínimo asociado: el método `Mode::prev` que
   vivía como `impl Mode { pub(super) fn prev }` en
   `app/editor.rs` se promueve a método del propio tipo
   en `agent.rs` (mismo cuerpo, ahora `pub`) — necesario
   por orphan rules porque el `impl` ya no puede estar
   en el crate del bin tras mover `Mode` al lib.

   **D1 no se cerró tras este paso:** la decisión de
   mantener Tauri + Angular fue posterior, por elección
   explícita del usuario. Este spike etapa 1 es solo el
   seam `lib` que habilita la dependencia por path que
   el slice read-only de Fase 5 reutilizará. La base
   del slice read-only (binario acompañante, paired
   install contract en `README.md`) está ejecutada en
   Fase 5 [x]; el slice vertical de mutación pequeña
   síncrona (GUI Settings: campo ruta absoluta +
   Guardar) está implementado y aprobado en Fase 5
   [x] "Slice vertical"; las **operaciones largas /
   siguientes mutaciones del slice vertical** (`sync`
   masivo, `install_tool`, `agent editor`, `skills`
   — D3 SOLO es precondición de las operaciones
   largas; no aplica al read-only ni a la mutación
   pequeña síncrona Settings) quedan descritas como
   Fase 5 [ ]; su ejecución queda pendiente.

   **Spike etapa 2 ejecutado: prototipo aislado
   `spikes/tauri-angular/`.** Scaffold oficial Tauri v2 +
   Angular 22 inspeccionado en `/tmp/opencode/cta-template`
   con `npm create tauri-app@4.7.4 -- --template angular
   --tauri-version 2` (sin instalar paquetes de sistema
   en el host del spike). El `src-tauri/Cargo.toml`
   declara `agenthd = { path = "../../.." }` y nada más
   del lib se toca. **Dos comandos Tauri read-only**
   declarados en `spikes/tauri-angular/src-tauri/src/lib.rs`
   como wrappers de un solo nivel: cada comando hace
   `Paths::from_env()` + un composition helper puro
   (`compose_settings_status(&Paths)`,
   `compose_agents_list(&Paths)`). Los helpers reusan
   `workflows::read_checkout` y
   `workflows::list_canonical_agents` del lib. Sin
   argumentos del frontend, sin `ensure_dirs`, sin
   `save_*` / `delete_*`, sin plugins (`tauri-plugin-opener`
   quitado de la plantilla), capabilities en
   `["core:default"]` solamente. CSP mínima con la directiva
   `connect-src: ipc: http://ipc.localhost` requerida por
   Tauri v2 para que el IPC JS↔Rust funcione — sin ella
   los `invoke()` desde Angular serían bloqueados por el CSP
   en runtime. CSP de producción (`security.csp`):
   `default-src 'self'; connect-src: ipc: http://ipc.localhost;
   img-src: 'self' data:; style-src: 'self' 'unsafe-inline';
   script-src: 'self'`. CSP de dev (`security.devCsp`):
   añade `ws://127.0.0.1:14720 http://127.0.0.1:14720` para
   el HMR de Vite/Angular; esta directiva NO se inyecta en
   builds de producción (`tauri build` solo usa `csp`).
   Fuentes: <https://v2.tauri.app/security/csp/> y
   `SecurityConfig::devCsp` en <https://schema.tauri.app/config/2>.
   **El runtime IPC sigue sin comprobarse** (la ventana
   nativa no se ha podido ejecutar en este host); la
   corrección se basa en documentación oficial, no en
   ejecución end-to-end.
   `AgentSummary` no expone `prompt` ni `permissions` —
   los tests `projection_drops_prompt_and_permissions` y
   `ready_checkout_json_has_no_prompt_or_permission_leak`
   fijan el contrato sobre el JSON serializado (afirman
   que el payload no contiene las keys `"prompt"` /
   `"permissions"` y que las keys top-level son
   exactamente `{description, mode, model, name}`).
   Frontend Angular con un único `AppComponent` y dos
   `signal()` por panel; carga inicial al abrir vía
   `ngOnInit` (un solo `refreshAll()`) y refresco manual
   vía botones; sin auto-poll.
   Validación REAL corrida en este host (no proyección):
   `npm install` 218 paquetes, `package-lock.json`
   generado y versionado con el spike (195 KB);
   `npm ci` 218 paquetes en 2s (lockfile reproducible);
   `npm run build` Angular 22 bundle 209 KB initial / 57 KB
   gzipped, output en `dist/agenthd-tauri-angular-spike/browser/`;
   `cargo check` ok;
   `cargo build` ok (2.64s, binario 193 MB debug);
   `cargo test --lib` **8/8 ok** (los 8 tests llaman a
   los composition helpers, no a `read_checkout` directo);
   `cargo clippy --lib --tests` sin warnings nuevos del
   spike;
   `cargo fmt --check` limpio.
   El workspace raíz sigue compilando intacto
   (`cargo test --all-targets` = 474 ok + 1 ignored,
   mismos números que antes del spike; `agenthd gui`
   sigue rechazado por `src/main.rs` con exit code 2).
   **Este 474 es la snapshot del root crate en Linux
   al cierre del spike etapa 2, antes del slice
   "binario acompañante" de Fase 5; el conteo
   post-slice actual del root crate es 491 ok
   (Windows 11 MSYS — ver "Validación y plataforma").
   El conteo Linux post-slice no se re-ejecutó.**
   Lockfiles generados y versionados con el spike:
   `spikes/tauri-angular/src-tauri/Cargo.lock` (115 KB,
   Tauri 2.12.0, wry 0.57.0) y
   `spikes/tauri-angular/package-lock.json` (195 KB,
   Angular 22.0.1, @tauri-apps/api 2.x).
   `node_modules/`, `dist/`, `target/`, `gen/schemas/`
   ignorados localmente. Pros y contras factuales
   (Arch / otros Linux / Windows, scaffold vs slice
   vertical, lockfile doble) y limitaciones honestas
   (intento de ejecución del binario en este host con
   `DISPLAY=:0` y `timeout 12s
   ./target/debug/agenthd-tauri-angular-spike` → exit
   124, stderr vacío: el proceso siguió corriendo hasta
   el timeout pero **no demuestra visualización ni
   IPC/ventana funcional**; los tests del backend no
   invocan el dispatch Tauri sino los helpers que los
   comandos llaman) en `spikes/tauri-angular/EVIDENCE.md`.
   Instrucciones para ejecutar el spike end-to-end
   (`npm install && npm run tauri dev`) en
   `spikes/tauri-angular/README.md`.
   **D1 cerrado por elección explícita del usuario**
   (Tauri + Angular — familiaridad con Angular + spike ya
   ejecutado). Este spike sigue siendo una pieza de
   evidencia; **no** se afirma validación de visualización
   ni de IPC end-to-end en este host. El siguiente paso
   es el slice read-only de Fase 5 (ver punto 5).
5. **Próxima tarea estrecha (Fase 5).** D1 cerrado por
   elección explícita del usuario (Tauri + Angular). El
   spike ya ejecutado deja dos comandos read-only
   (`compose_settings_status`, `compose_agents_list`)
   reusando el lib. **Confirmación manual user-reported
   (Windows 11 interactivo):** el usuario reportó que
   Settings y Agents aparecen y que Refresh both
   actualiza — ver
   `spikes/tauri-angular/EVIDENCE.md` → "Observaciones
   Windows 11 MSYS → Verificación manual user-reported".
   Esa confirmación es **manual y user-reported**, **no**
   instrumentada ni capturada por el agente: **no** se
   capturaron payloads de IPC, **no** se inspeccionaron
   CSP / devtools / red, **no** se inspeccionó el DOM,
   **no** se tomaron screenshots, **no** se afirmó
   contenido específico mostrado, y **no** se ejecutó
   el runtime empaquetado en otras plataformas.

   **Lo que la confirmación manual establece:** el gate
   visual del primer slice read-only de Fase 5
   (aparición de los paneles Settings y Agents, y
   refresco manual al pulsar "Refresh both") queda
   **satisfecho** por el reporte manual del usuario
   en una sesión `npm run tauri dev` interactiva en
   Windows 11. La atribución es honesta — el agente
   no instrumentó esa sesión — pero **sí** se da por
   satisfecho lo que el usuario atestiguó sobre esos
   tres puntos. **Lo que la confirmación manual NO
   hace:** no inspecciona payloads de IPC, no
   inspecciona CSP en runtime, no inspecciona
   devtools ni red, no afirma contenido específico
   mostrado, y no cubre otras plataformas
   (empaquetado Arch / otros Linux / Windows sigue
   siendo gate **separado** de Fase 6, D4 pendiente).
   Un eventual probe automatizado end-to-end — si se
   ejecuta — sería validación adicional para
   profundizar confianza, pero **no** es cierre
   formal obligatorio del gate visual de Fase 5,
   que queda satisfecho por la confirmación
   user-reported.

**Estado actual del primer slice read-only
   (Fase 5).** La integración **ya está ejecutada**
   (el slice "binario acompañante" de Fase 5 está
   marcado [x]): la rama `agenthd gui` de `src/main.rs`
   ya no rechaza `gui` por "no implementado"; localiza
   el binario `agenthd-gui` adyacente al ejecutable y
   aborta con exit `2` y stderr claro si falta **antes**
   de tocar `settings.json` (acompañante ausente ⇒
   exit `2` con stderr, **sin escrituras** a
   `settings.json` / `state.json` / OpenCode / Pi); cuando
   el acompañante está presente lo lanza con el mismo
   argv y reenvía su código de salida. `--repo` aplica
   y persiste igual que en la TUI cuando el
   acompañante está presente (`settings.json` se
   escribe por la rama save-if-changed de
   `resolve_checkout_path`); `--repo` igual al valor
   persistido **no** reescribe `settings.json`. Los
   comandos `compose_settings_status` y
   `compose_agents_list` del acompañante son read-only
   y reusan el lib; `ensure_dirs` y `TerminalGuard` /
   panic hook **no** se llaman en la rama GUI. La
   semántica de la TUI y de `--repo` queda intacta
   (TUI **sin cambios observables**; mismas pruebas,
   mismo contrato D2, misma precedencia override /
   save-if-changed). El contrato de instalación
   "emparejada" (CLI + GUI como dos crates separados,
   sin segundo copiado del spike) está documentado en
   `README.md` → "Paired install contract (CLI +
   companion GUI)". El binario del spike se renombró a
   `agenthd-gui` y bajó el branding "spike" en
   `productName` / `identifier` / título de ventana /
   `<h1>` Angular; comandos, capabilities, CSP y
   comportamiento read-only **sin cambios**.

   **Qué se ha validado a nivel de paired build
   (Windows 11 MSYS):** instalación con `cargo install
   --path . --locked --root <isolated temp>` (CLI) +
   `cargo install --path spikes/tauri-angular/src-tauri
   --locked --bin agenthd-gui --root <same isolated
   temp>` (GUI) → `agenthd.exe gui` cronometrado
   produjo un proceso acompañante `agenthd-gui.exe` con
   `MainWindowHandle=789140` y título `'agenthd GUI'`.
   **Solo** se observó el handle / título por
   enumeración de procesos; **no** se instrumentó
   contenido Angular, **no** se capturaron payloads de
   IPC, **no** se inspeccionaron CSP / devtools / red,
   **no** se tomó screenshot del runtime de
   producción, y **no** se inspeccionó el DOM. Esa
   validación histórica **no** cerraba por sí sola el
   gate visual del primer slice read-only: el cierre
   vino luego por la confirmación manual user-reported
   en sesión Linux interactiva sobre el mismo HOME (ver
   Fase 5 [x] "Comparación paired GUI↔TUI" arriba y
   "Estado actual" más abajo).
   El cleanup de la paired build se hizo por timeout;
   **no** se reclama empaquetado ni distribución por
   paquete (riesgo de reproducibilidad: `cargo`
   advirtió que `yoke-derive v0.8.3` está yanked del
   registro `locked`; la instalación completó pero un
   rerun con `--locked` puede fallar si el resolver no
   encuentra esa versión). La confirmación manual
   user-reported del spike en `tauri dev` (Settings +
   Agents + Refresh both) **sigue siendo evidencia
   aparte y separada**: es del spike en modo dev, no
   del binario de producción, y **no** captura
   payloads IPC, **no** inspecciona CSP / devtools /
   red, **no** afirma contenido específico mostrado,
   **no** cubre otras plataformas.

   **Próxima tarea estrecha (Fase 5).** El gate visual
   del primer slice read-only de Fase 5 está **cerrado**
   por confirmación manual user-reported en sesión Linux
   interactiva (Fase 5 [x] "Comparación paired GUI↔TUI"
   arriba); la integración está ejecutada (slice
   "binario acompañante" + instalador
   `scripts/install.mjs`); el slice vertical de
   **mutación pequeña síncrona** está implementado y
   aprobado (Fase 5 [x] "Slice vertical — primer slice de
   mutación pequeña síncrona") — campo ruta absoluta +
   Guardar en Settings, comando Tauri `apply_checkout`
   que delega en `workflows::apply_checkout` (que
   revalida internamente el path persistido), lock
   `busy` para refresh + save, refresh posterior
   dentro del mismo `busy`. Las **siguientes
   mutaciones** (`agent editor`, `skills`, `sync`
   masivo, `install_tool`) siguen descritas en Fase 5
   [ ] y **no** son integración nueva — reutilizarán
   los workflows extraídos en Fase 2
   (`plan_then_apply_skills`,
   `plan_then_apply_agents_safe`, `save_agent`) y los
   composition helpers `compose_settings_status` /
   `compose_agents_list` ya reusados por el slice
   read-only. **D3 es precondición SOLO de las
   operaciones largas** (`install_tool`, `Discovery`,
   `sync` / `plan` masivos): canal de progreso y
   cancelación para que el usuario pueda abortar y
   observar avance. Un editor síncrono pequeño (p. ej.
   `save_agent` directo vía wrapper Angular, equivalente
   al slice Settings ya ejecutado) **no** requiere D3.
   La decisión D3 sobre el mecanismo concreto (worker
   thread, thread-pool, async, message-passing, etc.)
   **no** se toma en este handoff; queda bloqueada por
   falta de aprobación explícita del usuario. D4 y D5
   pendientes como gates separados (empaquetado,
   schema/rutas).

## Validación y plataforma

- `cargo test --all-targets` — conteo **histórico** del root crate (Linux, **antes** de la integración del slice "binario acompañante"): 233 (lib) + 61 (bin) + 2 (cli_launch integ) + 178 (smoke) = **474 ok, 1 ignored** — **única duplicación: 2 tests del fixture `starter_fixture::tests::starter_registry_*` corren 2x (una en el lib, otra en el bin)**; los demás tests no se duplican. **El conteo Linux post-slice no se re-ejecutó:** los conteos observados del root crate **después** de integrar el slice "binario acompañante" son los de **Windows 11 MSYS** documentados más abajo — **interim 490 ok, 0 failed, 0 ignored** (run anterior del root crate en Windows 11 MSYS, con 4 tests `cli_launch`); **current 491 ok, 0 failed, 0 ignored** (run posterior tras habilitar el quinto test de integración `cli_launch`). El lib corre los 233 tests de los módulos compartidos (`agent`, `launcher`, `models`, `store`, `tools`, `workflows`) y el bin corre **65 tests post-slice** (59 TUI-specific de `app::tests::*` + 2 del fixture re-incluido en `src/main.rs` + **4 nuevos del slice "binario acompañante"** en `src/main.rs::tests`: `locate_companion_returns_none_when_no_companion_adjacent`, `locate_companion_finds_platform_native_companion_adjacent`, `locate_companion_ignores_non_companion_files`, `spawn_propagates_companion_exit_code`); pre-slice eran 61 bin (59 + 2). Los **5** tests de integración en `tests/cli_launch.rs` (`agenthd_gui_rejects_with_clear_error_and_no_side_effects`, `agenthd_gui_with_repo_rejects_without_writing_settings`, **`agenthd_gui_with_companion_present_persists_repo_without_target_writes`**, **`agenthd_gui_with_companion_does_not_rewrite_settings_when_unchanged`**, **`agenthd_gui_with_companion_present_and_invalid_repo_fails_closed`**) se ejecutan contra una copia aislada del binario en un `TempDir` (no contra el `target/debug/agenthd(.exe)` directo, para que un `agenthd-gui(.exe)` instalado o copiado por el usuario junto al binario de producción no falsifique los tests de compañero-ausente). Los 2 tests originales (`gui` y `gui --repo`) ahora usan la copia aislada y esperan el nuevo mensaje de stderr (`companion binary \`agenthd-gui...`); los 3 nuevos (companion-present) copian un companion falso (script shebang en Unix, `cmd.exe` renombrado en Windows), validan que `settings.json` se persiste con el `--repo`, que `state.json` / OpenCode / Pi target trees no se escriben, que `--repo` igual al valor persistido no reescribe `settings.json` (la rama `save-if-changed` en `resolve_checkout_path` mantiene sus bytes), y que `--repo <path-inexistente>` falla cerrado (exit 1, sin `settings.json`). El contrato verificable (exit code 2 con stderr claro y sin `settings.json` cuando el acompañante falta) se preserva bit-for-bit. `Cargo.lock` confirma tamaño de binario de producción sin fixtures (verificado por `strings`: cero markdown de los `agents/*.md`). **Bloqueador pre-existente en Windows para `cargo test --all-targets`: resuelto.** Los call sites problemáticos en `src/store/{tests, settings}.rs` que usaban `std::os::unix::fs::symlink` / paths Unix-style absolutos sin `#[cfg(unix)]` se corrigieron (5 call sites: `canonical_dir_from` en `settings.rs` y tests; arm `unix_symlink` en tests de symlink-rejection y validate; paths absolutos en tests de missing-checkout) junto con la construcción de sufijos del `rename_seam` con separador nativo del host. Conteos observados en Windows 11 MSYS post-corrección (run **current** con 5 tests `cli_launch`): 238 (lib) + 65 (bin) + 5 (cli_launch integ) + 183 (tools_install_smoke) = **491 ok, 0 failed, 0 ignored** — el comando `cargo test --all-targets` ahora pasa entero en Windows. La corrección confirma que los tests del root crate ya no tropiezan con la falta de `std::os::unix` en MSVC; **no** se afirma validación cross-platform end-to-end del runtime gráfico ni del empaquetado Windows (siguen siendo gate de Fase 6, D4 pendiente), y Arch / otros Linux siguen pendientes en máquina real. (D2 añadió
  13 tests focales del parser y 2 tests de integración del rechazo
  `gui`; el slice "binario acompañante" actualiza los 2 tests
  originales de `cli_launch` (mismo contrato, mensaje actualizado,
  ahora con copia aislada del binario en `TempDir`) y añade 2
  tests de integración nuevos (companion-present con
  `--repo` válido y con `--repo` inválido), más 4 tests focales
  en `src/main.rs::tests` (locator puro, decoy,
  fake-process spawn-and-wait, exact-sufijo); la primera
  extracción de Fase 2 añadió 3 tests focales del workflow
  `plan_then_apply_skills`; la segunda extracción añade 4
  tests focales del workflow `plan_then_apply_agents_safe` y 2
  tests TUI directos del handler `apply_safe_install` para fijar
  el orden observable `load_canonical → guard target` y el mensaje
  `pick a harness first`; la tercera extracción añade 5 tests
  focales del workflow `save_agent` (alta, update, rename, fuente
  stale sin mover, destino ocupado); la evaluación final de Fase
  2 añade 9 tests TUI directos en `app::tests::tools_*` +
  `app::tests::ctrl_c_from_tools_screen_*` (refresh catálogo,
  status filesystem per-entry, helper de selección
  valid/invalid, `i` con `selected` fuera de rango, bloqueo
  de teclas mientras `installing = true`, Press-vs-Release,
  Ctrl-C) sin ampliar la cobertura reusable porque ninguno
  invoca `install_tool` real — son tests UI-only del flujo
  ya cubierto por `tools::install_tool_with`; la rama 178
  cubre `tests/tools_install_smoke.rs`).
- `cargo fmt --check` — limpio.
- `cargo clippy --all-targets --all-features -- -D warnings` —
  falla en línea base por warnings preexistentes. Métrica útil: el
  conjunto de warnings no debe crecer respecto a la línea base
  capturada (no introducir warnings nuevos). D2, la segunda y
  tercera extracciones de Fase 2 no añaden warnings. Tras la
  creación del seam `lib` (Fase 4 spike, paso 1) y el refactor
  del fixture para evitar bytes de markdown en producción: diff
  textual vs baseline (líneas `warning: <contenido>` sin contar
  las líneas resumen agregadas por target) = **vacío**, ningún
  warning nuevo.
- `cargo build --target x86_64-pc-windows-msvc` solo verifica que
  compila. "Linux validado" = solo el entorno Linux actual (probado
  como Linux genérico, no como Arch específica); Arch, otros Linux
  y Windows pendientes en máquina real (`MoveFileW`, shim
  npm-via-node, `cfg(windows)`); el README no debe declarar Arch ni
  Windows soportados hasta entonces. **Windows 11 MSYS:** hay un
  smoke parcial registrado en `spikes/tauri-angular/EVIDENCE.md`
  ("Observaciones Windows 11 MSYS") — `cargo test --lib` 8/8 OK,
  `npm run build` OK, `npm run tauri dev` arrancó y
  `Get-Process` observó un `MainWindowHandle` con título
  `agenthd spike (D1 / Tauri + Angular)` (PID 11624) a los 12 s,
  con un run timed interrumpido a los 55 s (exit 143) sin
  observarse contenido Angular ni respuesta IPC. **No** equivale a
  Windows validado: el runtime gráfico y el IPC end-to-end
  siguen pendientes — el smoke reduce incertidumbre sobre
  arranque del binario y presencia de WebView2, no sobre
  visualización ni IPC. **Confirmación manual user-reported
  (Windows 11 interactivo):** el usuario reportó que Settings
  y Agents aparecen y que Refresh both actualiza (ver
  `spikes/tauri-angular/EVIDENCE.md` → "Observaciones Windows
  11 MSYS → Verificación manual user-reported"); esa
  confirmación es **user-reported**, **no** instrumentada por
  el agente, **no** captura payloads IPC, **no** inspecciona
  CSP / devtools / red, **no** afirma contenido específico, y
  **no** equivale a Windows validado ni empaquetado.
  Empaquetado Windows sigue siendo gate de Fase 6.
- **Instalador fuente `scripts/install.mjs` (orquestadora
  node portable, sin dependencias externas).** `node --test
  scripts/install.test.mjs` — **29 ok, 0 failed**, suite
  verde en el host real Windows 11 MSYS y simulada en
  `linux` vía `main({platformName: "linux"})`. Cobertura
  puntual (no exhaustiva): parser strict, precedencia
  `--root > CARGO_INSTALL_ROOT > CARGO_HOME > $HOME/.cargo`
  con env vars vacíos = ausentes, plan completo
  (preflight → `npm ci --include=dev` → `npm run build` →
  CLI install → GUI install con `--features custom-protocol`),
  `--force` solo cuando se pasa, dispatch Windows npm vía
  `ComSpec /d /s /c <line>` con root nunca en argv, fallback
  a `cmd.exe`, dispatch Linux npm directo + `shell:false`,
  cargo siempre directo + `shell:false`, aborts npm /
  preflight / cargo CLI / cargo GUI sin éxito falso,
  `--help` y errores del parser sin invocar herramientas.
  Validación end-to-end real (no solo tests): el
  orquestador ejecutó en una raíz temporal preaprobada
  `…/opencode/unified-install-1790880069/bin/`, produciendo
  `agenthd.exe` y `agenthd-gui.exe` (este con la feature).
  El orquestador **no** se ejecutó contra `~/.cargo/bin/`
  (HOME real **no certificado**). El flujo paired desde
  esa raíz no fue cronometrado. Riesgo de reproducibilidad:
  `yoke-derive v0.8.3` yanked del registro `locked`; el
  `Cargo.lock` no se tocó por esta sesión — un rerun con
  `--locked` puede fallar (riesgo no cerrado). `cargo fmt
  --check` falló por diffs preexistentes en `src/lib.rs` y
  companions (no introducidos aquí; código ajeno no
  tocado). `git diff --check` solo emite advertencias
  LF/CRLF preexistentes (sin errores).
- Root crate `cargo test --all-targets` en Windows 11 MSYS:
  verde tras la corrección de portabilidad de los tests
  `unix_symlink` / unix-path en `src/store/{skills_tests,
  settings, tests}` y de la construcción de sufijos del
  `rename_seam` con separador nativo del host; los conteos
  observados son 238 (lib) + 65 (bin) + 5 (cli_launch integ) +
  183 (tools_install_smoke) = **491 ok, 0 failed, 0 ignored**.
  `cargo test --test cli_launch` corre los 5 tests verde
  (incluidos los `companion-present` con el fake `cmd.exe`).
  `cargo fmt --check` limpio. **No** se afirma validación
  cross-platform end-to-end: la corrección confirma que los
  tests del root crate ya no tropiezan con la falta de
  `std::os::unix` en MSVC, no que el runtime gráfico o el
  empaquetado Windows estén validados — esos siguen siendo
  gate de Fase 6 (D4 pendiente), y Arch / otros Linux siguen
  pendientes en máquina real.
- **Observación Fase 5 — paired build instalada en raíz
  temporal preaprobada (Windows 11 MSYS).** Comandos
  manuales previos a `scripts/install.mjs` instalaron
  ambos binarios en una raíz temporal preaprobada en
  Windows con `cargo install --path . --locked --root
  <isolated temp>` (CLI) y `cargo install --path
  spikes/tauri-angular/src-tauri --locked --bin
  agenthd-gui --root <same isolated temp>` (GUI); esta
  última **sin** `--features custom-protocol` (receta
  anterior al fix). Un `agenthd.exe gui` cronometrado tuvo
  un proceso acompañante `agenthd-gui.exe` con
  `MainWindowHandle=789140` y título `'agenthd GUI'`. La
  paired build **no** instrumentó contenido Angular ni IPC
  (solo handle / título por enumeración de procesos);
  cleanup por timeout. **No** se reclama empaquetado ni
  distribución por paquete. **Riesgo de reproducibilidad:**
  `yoke-derive v0.8.3` yanked del registro `locked`;
  rerun con `--locked` puede fallar (riesgo no cerrado).
  **Esta paired build (sin la feature) fue la que disparó
  el defecto "Hmmm… can't reach this page" del usuario**;
  ver "Defecto user-reported y fix acotado". La confirmación
  manual user-reported del spike (Settings + Agents +
  Refresh both, ver
  `spikes/tauri-angular/EVIDENCE.md` → "Verificación
  manual user-reported") sigue siendo **distinta y
  separada**: manual, no-instrumentada, no captura payloads
  IPC ni inspecciona CSP / devtools / red.

### Validación de esta sesión (root crate, formateo, settings/state)

- **Fallo de baseline y verificación normalizada.** El test
  `starter_registry_matches_baseline` falló con
  `left = af9e620c...` vs `right = 252296...` (caso
  `oracle`); antes de tocar `BASELINE` se ejecutó un test
  temporal `normalize_model_to_baseline_matches_pinned_hashes`
  que sobreescribe en memoria el campo `model` de los 3
  agentes afectados (`oracle`, `planner`, `lukateric`) al
  valor anterior `openai/gpt-6-sol` y recomputa los 3
  SHA-256 (prompt, rendered, render_pi) por agente. Con la
  normalización, los 24 hashes de los 8 agentes
  (incluidos los 5 no afectados, byte-exactos) coinciden
  con `BASELINE`. La causalidad quedó demostrada; las 2
  pruebas temporales se eliminaron y solo se actualizaron
  los 3 `rendered_sha` de los agentes con cambio de modelo.
- **`cargo test --all-targets` (root crate, post-fix):**
  233 (lib) + 65 (bin) + 5 (cli_launch integ) + 178
  (smoke) = **481 ok, 1 ignored**, 0 failed. Logs en
  `/tmp/opencode/logs/cargo-test-all-targets.log`.
- **`node --test scripts/install.test.mjs`:** 29/29 ok,
  0 failed. Log en `/tmp/opencode/logs/install-test.log`.
- **`cargo test --lib` y `--features custom-protocol`
  (Tauri manifest, `spikes/tauri-angular/src-tauri`):**
  **16/16 ok** en ambas variantes (8 read-only previos
  + 8 nuevos `compose_apply_checkout_*` del slice
  vertical). Log en
  `/tmp/opencode/logs/tauri-manifest-test.log`.
  **Conteo histórico Tauri8** = el previo a esta sesión
  (8 tests read-only, antes del slice vertical); los
  `Tauri16` actual en ambas variantes (default y
  `--features custom-protocol`) **son la evidencia
  actual** del slice vertical.
- **`node --experimental-vm-modules
  /tmp/opencode/harness/harness.mjs`** (harness
  **temporal, no trackeado** en el repo, ni en
  `package.json` ni en infraestructura): transpila el
  **TS original** de
  `spikes/tauri-angular/src/app/app.component.ts` a
  ESM y lo ejecuta en un `node:vm` con `@angular/core`
  y `@tauri-apps/api/core` **sintéticos** (mock de
  `Component` no-op, `signal()` mínimo, `invoke`
  diferido por test, **sin** runtime Angular, **sin**
  DOM, **sin** IPC real). **28/28 checks passed** en
  esta sesión (padre re-ejecutó: `harness: 28/28
  checks passed` en `/tmp/opencode/harness/run.log`).
  Cubre el comportamiento del componente Angular
  (`ngOnInit`, `refreshAll`, `refreshSettings`,
  `refreshAgents`, `saveCheckout`, `onCheckoutPathInput`,
  `settingsLabel`, `settingsDetails`, `toMessage`,
  errores de `invoke`, serialización del lock `busy`,
  drafting desde `Ready`/`Stale`, post-save
  refresh, etc.) **sin tocar el DOM ni abrir el
  runtime gráfico**. Log en
  `/tmp/opencode/harness/run.log`.
- **`cargo fmt --check`** root y Tauri: ambos limpios.
  Los tres hunks aplicados (`spikes/tauri-angular/src-tauri/
  src/lib.rs`) cierran el diff de formato preexistente.
  Logs en `/tmp/opencode/logs/fmt-root.log` y
  `/tmp/opencode/logs/fmt-tauri.log`.
- **`git diff --check`:** limpio (sin warnings ni errores).
  Log en `/tmp/opencode/logs/git-diff-check.log`.
- **SHA-256 de `settings.json` y `state.json` (HOME
  real, no instrumentado en esta sesión):** iguales
  antes y después de la corrida de tests (padre
  ejecutó la comprobación por separado: settings
  `95b6ea82880d2b08…` y state `d04b5f550121dcb1…`,
  prefijos abreviados a propósito; no se incluyen los
  hashes completos en el doc). El refactor del fixture
  y el formateo del `lib.rs` Tauri no escriben ni en
  `settings.json` ni en `state.json` ni en los árboles
  OpenCode/Pi.
- **Gate visual de Fase 5 (paired GUI↔TUI):** YA cerrado
  por confirmación manual user-reported en sesión Linux
  interactiva sobre el mismo HOME («Sí, todo coincide»)
  — no instrumentado, sin capturas IPC / CSP / devtools /
  screenshots. La producción existente
  (`~/.cargo/bin/agenthd` + `~/.cargo/bin/agenthd-gui`)
  **no se reinstaló** en esta sesión: el SHA-256 de los
  binarios instalados es el histórico y no se certifica
  contra el árbol actual.

### Defecto user-reported y fix acotado

- **Síntoma:** el usuario reportó "Hmmm… can't reach this
  page" en su GUI `agenthd-gui` instalada. La causa
  operativa confirmada por fuentes oficiales es que el
  `cargo install --path src-tauri --locked --bin
  agenthd-gui` previo se hizo sin la feature
  `custom-protocol`; sin ella `tauri-macros` deja
  `dev = cfg!(not(feature = "custom-protocol"))` en
  `true` y el codegen cae en la rama de dev, que omite
  el embedding del frontend. Fuentes: `tauri-macros
  src/context.rs::generate_context` (campo `dev`
  gobernado por `cfg!(not(feature = "custom-protocol"))`)
  y la receta de release del companion en README que
  requería la feature opt-in. **No** se inspeccionaron
  strings específicos del binario fallido ni se tomaron
  capturas del error: la diagnosis se hizo por
  documentación, no por instrumentación.
- **Fix:** `spikes/tauri-angular/src-tauri/Cargo.toml`
  añade `[features] custom-protocol = ["tauri/custom-protocol"]`
  (sin `default`, para preservar `tauri dev`). README
  raíz (paso 3) y spike README (paso 3) pasan a requerir
  `--features custom-protocol` en `cargo install` /
  `cargo build` directos. La orquestadora
  `scripts/install.mjs` lo aplica por defecto en el
  install del GUI (ver entrada [x] arriba). Comandos
  históricos no reescritos.
- **Validación de codegen y receta:** `cargo tree -e
  features` con la feature muestra `tauri custom-protocol`
  y `tauri-macros custom-protocol` activas; sin la feature
  ninguna de las dos aparece. `cargo test --lib` 8/8 OK
  con y sin feature. `npm run build` regenera `dist/`.
  `cargo install --locked --bin agenthd-gui --features
  custom-protocol --root <temp>` produce `agenthd-gui.exe`
  en la raíz temporal. `scripts/install.mjs` cubre el
  flujo end-to-end con 29 tests `node --test` en verde
  (ver "Validación y plataforma → Instalador fuente").
  **No** se reemplazaron binarios en `~/.cargo/bin/` por
  la sesión del fix; el estado actual de HOME **no está
  certificado**. `cargo fmt --check` FALLÓ por diffs
  preexistentes en `src/lib.rs` y companions (no
  introducidos por este fix; código ajeno no tocado).
  `git diff --check` solo LF/CRLF preexistentes (sin
  errores).
- **Confirmación user-reported del render (producción,
  aislada del flujo paired).** El worker compiló
  `agenthd-gui.exe` con la feature aplicada en
  `…/opencode/gui-custom-protocol-1790876724499052800/bin/`,
  el padre lo lanzó DIRECTO como exe suelto (no como
  acompañante del CLI — no fue flujo paired) y el usuario
  reportó "Sí se mostraba". Confirmación **manual,
  user-reported, no instrumentada**; cubre solo el render
  del binario temporal con la receta corregida. El render
  del binario suelto con la feature ya **no** es gate
  pendiente.
- **Reinstall con el orquestador (raíz temporal).** El
  orquestador `scripts/install.mjs` (sin `--force`,
  ejecución real desde el repo root en Windows 11 MSYS)
  reinstaló en `…/opencode/unified-install-1790880069/bin/`:
  ambos `agenthd.exe` y `agenthd-gui.exe` (este con la
  feature aplicada). **No** se ejecutó contra `~/.cargo/bin/`;
  HOME real no certificado. Flujo paired desde esa raíz
  **no** cronometrado.
- **Gate visual de Fase 5 — tres observaciones distintas
  y separadas:** (1) **Render suelto del binario temporal**
  (esta sección) — user-reported "Sí se mostraba", **ya
  no** es gate pendiente; no cubre paired/IPC. (2) **Spike
  en `tauri dev`** — Settings + Agents + Refresh both,
  user-reported manual, **evidencia aparte** del binario
  de producción; no captura payloads IPC. (3) **Validación
  paired contra TUI en sesión Linux interactiva** (ver
  Fase 5 [x] "Comparación paired GUI↔TUI") — gate
  visual del primer slice read-only de Fase 5 **cerrado
  por reporte manual user-reported** («Sí, todo
  coincide»); mismos límites que (1) y (2) — no
  instrumentado, no captura payloads IPC, no
  inspecciona CSP / devtools / red / DOM, no
  screenshots, no cubre empaquetado.

## Estado de la sesión y fuente de verdad

- **Cerrado:** Fases 1–4, Fase 3 (D2 — el "rechazo" de
  `agenthd gui` documentado en D2 es **histórico**; ver
  "Decisiones pendientes → D2 → Estado actual"); slice
  "binario acompañante" de Fase 5; instalador fuente
  `scripts/install.mjs` (29 tests `node --test` en
  verde); fix acotado del defecto "Hmmm… can't reach
  this page" (feature `custom-protocol` añadida + receta
  documentada); confirmación user-reported del render del
  binario suelto ("Sí se mostraba"); **comparación
  paired GUI↔TUI sobre mismo HOME** cerrada por
  confirmación manual user-reported en sesión Linux
  interactiva (Fase 5 [x] "Comparación paired GUI↔TUI"
  arriba; «Sí, todo coincide»); **slice vertical de
  mutación pequeña síncrona** (GUI Settings: campo ruta
  absoluta + Guardar, validación/persistencia vía
  `workflows::apply_checkout`, refresco Settings + Agents)
  implementado y aprobado por el usuario — backend 16/16
  con y sin `--features custom-protocol`, lock `busy` en
  el frontend, schema y CSP intactos (Fase 5 [x]
  "Slice vertical" arriba).
- **Pendiente (fuente de verdad para próxima tarea):**
  validar manualmente los **nuevos controles Settings**
  (campo ruta absoluta + Guardar + Refresh) frente a la
  TUI tras un build actualizado (gate visual NUEVO del
  slice vertical, **sin** dar por cerrado aún): abrir la
  GUI, guardar una ruta válida y volver a refrescar
  Settings y Agents; abrir la TUI y confirmar que
  muestran el mismo checkout y los mismos agentes. Las
  **siguientes mutaciones** (`agent editor`, `skills`,
  `sync` masivo, `install_tool`) siguen pendientes como
  subfases del slice vertical. **D3 (progreso y
  cancelación) es precondición SOLO de las operaciones
  largas** (`install_tool`, `Discovery`, `sync` /
  `plan` masivos): un editor síncrono pequeño vía
  wrapper Angular **no** exige D3. D4 (empaquetado Arch
  / otros Linux / Windows) y D5 (compatibilidad
  `settings.json` y rutas) siguen como gates
  separados. `scripts/install.mjs` **no** cierra D4.
  Detalle en "Próxima tarea estrecha → punto 5".
- **No certificado (límites de la confirmación manual):**
  payloads IPC, CSP / devtools / red, DOM y screenshots
  en runtime del flujo paired en HOME real; equivalencia
  binarios `~/.cargo/bin/` ↔ código local por timestamps
  (los binarios del día no se certifican contra el árbol
  actual); empaquetado Arch / otros Linux / Windows.
- **Cambios locales al cierre de la sesión de
  redacción del doc** (`git status` unstaged = 13
  archivos; el usuario ejecuta `commit` + `push` por
  su cuenta; este doc no afirma SHA futuro ni push
  ya ejecutado). **Previstos para el commit (7
  archivos GUI/docs):** `GUI_ROADMAP.md` (este),
  `README.md` (sección "GUI Settings slice" añadida),
  `spikes/tauri-angular/README.md` (sección 3 del
  `apply_checkout` añadida), `spikes/tauri-angular/src-
  tauri/src/lib.rs` (comando `apply_checkout` +
  helper `compose_apply_checkout` + 8 tests focales +
  formateo), `spikes/tauri-angular/src/app/app.component.
  ts` (signal `checkoutPath` / `saving` / `saveError` /
  `busy`, `saveCheckout()`, draft seeding,
  post-write revalidate en el mismo `busy`),
  `spikes/tauri-angular/src/app/app.component.html`
  (input + botón `Guardar` + errores de validación),
  `spikes/tauri-angular/src/app/app.component.css`
  (`.path-editor` aditivo). **Preservados fuera del commit
  (6 archivos, no se suben):** `agents/lukateric.md`,
  `agents/oracle.md`, `agents/planner.md` (modelo
  `gpt-6-sol` → `gpt-6.1-sol` en los tres) +
  `src/agent/starter_fixture/mod.rs` (3 `rendered_sha`
  refrescadas en `BASELINE` por el cambio de modelo —
  prueba de causalidad con normalización documentada en el
  comentario del fixture; `prompt_sha` y `pi_sha`
  inalterados para los 3; los 5 agentes no afectados
  byte-exactos contra el baseline previo) +
  `src/app/{mod,settings}.rs` (recovery banner único:
  `path_input.error = None` al abrir; `path_input.error`
  recibe solo validación post-Enter; nuevo test que pisa
  Ctrl-U entre submit inválido y submit válido; los 9
  tests TUI previos del editor y de Settings se mantienen
  verdes). **No se creó commit ni se hizo push en esta
  sesión.** Último commit local conocido en `main` =
  `88d9409 feat: add unified CLI and GUI source
  installer` (sin certificación del estado del remoto —
  `origin/main` no se consultó en esta sesión). Los
  `scripts/install.mjs`, `scripts/install.test.mjs`,
  `EVIDENCE.md` / `Cargo.toml` del spike referenciados
  arriba son **históricos** de commits ya en `main`
  (`88d9409`, `64e1a3c`, `ccd585f`, `4d984a2`,
  `2a63b15`); no se vuelven a listar como cambios
  pendientes. `PLAN.md` y `TOOL_INSTALLER_PLAN.md` no
  restaurados.

## Handoff (importante)

> El siguiente agente, en otra PC, debe ejecutar `git fetch` y
> `git pull origin main` **después** de que el usuario haya
> publicado el push. Tras la actualización local:
>
> - `git status` debe estar limpio.
> - `git log` debe incluir la auditoría de fase 1 (`4327db2`),
>   la evaluación de boot (`2a63b15`) y la última actualización
>   de este roadmap.
> - En un handoff remoto normal, `HEAD` debe coincidir con
>   `origin/main`.
>
> Si el push falla, **no** afirmar que la documentación está
> publicada; notificar el fallo y esperar a que el usuario lo
> resuelva. No restaurar `PLAN.md` ni `TOOL_INSTALLER_PLAN.md` —
> su retirada es deliberada.