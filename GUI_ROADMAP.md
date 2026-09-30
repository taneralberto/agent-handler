# Hoja de ruta GUI — handoff (ES)

Handoff autónomo para otro agente en otra PC.

> **Nota:** `PLAN.md` y `TOOL_INSTALLER_PLAN.md` fueron retirados
> del repositorio de forma deliberada (commit en `main`). No se
> reponen ni se apuntan como referencia canónica. Si una tarea los
> necesita, confirmar primero con el usuario.

## Objetivo

Añadir GUI a `agenthd` sin reescribir la base ni duplicar lógica. Sin
framework GUI hasta cerrar D1; D2 está cerrado (seam CLI), D3–D5
se cierran en sus fases respectivas (ver gates). TUI y `--repo`
siguen siendo el comportamiento observable hasta que llegue la GUI
(fase 5).

## Antes de empezar y estado observable hoy

- `src/main.rs` — boot, `TerminalGuard`, panic hook, ciclo TUI;
  rechazo de `gui` pre-efectos (D2 cerrado, fase 3).
- `src/launcher.rs` — `parse_launch` (parser CLI: `Mode`,
  `LaunchPlan`), `parse_repo_override` (lector de override usado
  por la rama de resolución/persistencia legado),
  `resolve_checkout_path`, `ResolveOutcome` (sin
  `crossterm`/`ratatui`/`std::panic`).
- `src/workflows.rs` — capa reusable (`read_checkout`,
  `apply_checkout`, `list_canonical_agents`).
- `src/store/` — `canonical`, `settings`, `skills`, `sync`, `mod`;
  ahí vive `require_canonical_source` y los invariantes fail-closed.
- `src/app/` — render + dispatch + estado de la TUI; mezcla capas en
  flujos no extraídos.
- `tests/cli_launch.rs` — integración D2: rechazo de `gui` sin
  efectos (incluyendo `--repo`).
- `REUSABLE_API_COVERAGE.md` — inventario y matriz con nombres reales
  de tests.
- `agenthd` abre la TUI por defecto; `agenthd tui` es explícito;
  `agenthd gui` rechaza con exit `2` antes de cualquier efecto.
  `--repo <path>` toma precedencia sobre `settings.json` y aplica a
  ambas modalidades; persiste cuando la modalidad existe (TUI hoy).
  `workflows::read_checkout` clasifica `settings.json` en la rama
  persistida; Settings y Agents ya consumen workflows.
  `TerminalGuard` y `AgentDraft` / `EditorField` / `EditorMode` /
  `EditorOp` / `edit_prompt_externally` son TUI-coupled
  (`main.rs`, `app/editor.rs`); reusarlos exige evaluar caso por
  caso tras D1. Windows sin validar en ejecución.

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
explícitamente como UI-only (`tools`, dispatch). La
próxima decisión bloqueante es D1 (forma GUI), no más
extracciones de Fase 2.

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
      no el dispatch Tauri en runtime). Detalle completo en
      "Próxima tarea estrecha → Spike etapa 2 ejecutado".
- [ ] Comparación y firma de D1 pendientes. D1 **no se
      marca cerrada** ni habilita Fase 5 hasta comparar
      pros/contras factuales de Tauri vs GTK4-rs vs egui vs
      iced vs servidor web local tomando como entrada la
      evidencia de este spike, las validaciones pendientes
      (ejecución real con display del binario Tauri en Arch
      vía `pacman`, otros Linux, Windows vía WebView2 +
      MSVC; pruebas del runtime gráfico y del dispatch
      Tauri end-to-end), y la aceptación explícita del
      usuario. La **ejecución real en Arch y en Windows NO
      es gate de D1**: queda en Fase 6 (verificación de
      empaquetado por plataforma).

### Fase 5 — Slice Settings + Agents y expandir

- [ ] Slice vertical; requiere D1 + D2 (subfases largas además D3).

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
4. **Acción siguiente (Fase 4 / D1).** Fase 2 cerrada:
   `plan_then_apply_skills`, `plan_then_apply_agents_safe`,
   `save_agent` extraídos a `workflows.rs`; `tools` y
   dispatch en `app/mod.rs` evaluados UI-only y no
   extraíbles (documentado arriba). El spike de Tauri +
   Angular está **ejecutado** (etapas 1 y 2, ver más abajo);
   **D1 (forma GUI) sigue sin cerrarse** — la decisión es
   entre escritorio nativo (Tauri, GTK4-rs, egui, iced) o
   servidor web local, y requiere comparar la evidencia del
   spike ya hecho con el resto de candidatos y la firma
   explícita antes de cualquier extracción nueva o slice
   vertical. Sin cambios de contrato CLI/GUI; `gui` sigue
   siendo solo la rama de rechazo. Fase 3 sigue cerrada
   (launcher aislado, D2 sellado).

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

   Cambio API mínimo asociado: el método `Mode::prev` que
   vivía como `impl Mode { pub(super) fn prev }` en
   `app/editor.rs` se promueve a método del propio tipo
   en `agent.rs` (mismo cuerpo, ahora `pub`) — necesario
   por orphan rules porque el `impl` ya no puede estar
   en el crate del bin tras mover `Mode` al lib.

   **D1 sigue sin cerrarse** tras este paso: la etapa 1
   solo habilita la dependencia por path; la elección de
   framework GUI y el slice vertical siguen bloqueados por
   la comparación y firma de D1 (etapa 2 ya ejecutada).

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
   **D1 sigue sin cerrarse**: este spike es solo una
   pieza de evidencia, no valida el runtime gráfico ni
   marca D1 como decidido. **El próximo paso no es
   invertir en el slice vertical de Fase 5 — eso viene
   después de D1.** El siguiente paso real es comparar
   pros/contras factuales de Tauri vs GTK4-rs vs egui
   vs iced vs servidor web local, tomando como entrada
   (a) la evidencia de este spike (scaffold reproducible,
   8/8 tests focales, lockfiles generados y versionados
   con el spike, lockfile doble, dependencia
   cruzada por path), (b) las validaciones pendientes
   (ejecución real con display del binario Tauri, Arch
   vía `pacman`, otros Linux, Windows vía WebView2 +
   MSVC; pruebas del runtime gráfico y del dispatch
   Tauri end-to-end), y (c) la aceptación explícita del
   usuario. La decisión D1 es bloqueante para Fase 4 y
   precede a Fase 5; Fase 5 no se inicia hasta que D1
   esté cerrado.

## Validación y plataforma

- `cargo test --all-targets` — 233 (lib) + 61 (bin) + 2 (cli_launch integ) + 178 (smoke) ok, 1 ignored — **única duplicación: 2 tests del fixture `starter_fixture::tests::starter_registry_*` corren 2x (una en el lib, otra en el bin)**; los demás tests no se duplican. El lib corre los 233 tests de los módulos compartidos (`agent`, `launcher`, `models`, `store`, `tools`, `workflows`) y el bin corre 61 tests (59 TUI-specific de `app::tests::*` + 2 del fixture re-incluido en `src/main.rs`). `Cargo.lock` confirma tamaño de binario de producción sin fixtures (verificado por `strings`: cero markdown de los `agents/*.md`). (D2 añadió
  13 tests focales del parser y 2 tests de integración del rechazo
  `gui`; la primera extracción de Fase 2 añadió 3 tests focales del
  workflow `plan_then_apply_skills`; la segunda extracción añade
  4 tests focales del workflow `plan_then_apply_agents_safe` y 2
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
  Windows soportados hasta entonces.

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