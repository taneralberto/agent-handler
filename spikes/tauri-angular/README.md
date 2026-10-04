# Spike D1: Tauri v2 + Angular sobre `agenthd`

## Última evidencia (2026-10-04)

Matriz: `../../packaging/arch/VALIDATION.md`. Remap compiler aprobado y paquete
offline CachyOS **SUCCESS**, parent-verified; warning `$srcdir` ausente y strings
de ambos binarios sin nuevo build-source. No rebuild en este update docs.
Run enfocado **43 PASS** (29 installer + 14 packaging), Bash syntax y diff
check PASS; demás resultados son últimos verificados, no rerun aquí.

Autoridad: nueva sección superior de `../../GUI_ROADMAP.md`; resumen anterior
debajo es histórico donde contradiga esto. Tauri default/custom-protocol
**95 PASS cada uno**, Node **84 PASS**, Angular build PASS; root **576 PASS,
1 ignored**, installer+packaging **39 PASS** histórico. Paquete paired offline CachyOS
e installer aislado CLI + GUI custom-protocol **SUCCESS**, sin host install/AUR.
Nuevo source/paquete: `/tmp/opencode/agenthd-arch-remapped/`; checksums, log
`arch-remap-package-build.log` y límites `--nodeps`: `../../packaging/arch/README.md`.
Snapshot aprobado precede los updates docs, no bit-idéntico al working tree.
Fixture `/tmp/opencode/agenthd-gui-smoke`: proceso histórico arch-final launch + termination reportado
por padre vía notificación shell SSH; log redirigido 0 bytes, sin screens/DOM/IPC
inspection ni aceptación UI. Comandos
paired y procedimiento allí. Gates visuales actuales pendientes, Discovery
stub no certifica OpenCode real, Tools remoto pendiente; read-only Linux gate
anterior sigue cerrado. Nuevo GUI remapped **no lanzado**; CLI packaged negativo
exit 1 sin children HOME/XDG, evidencia `/tmp/opencode/arch-cli-negative-Nyfcgx`.
D5 abierto, sin claim native all-platform. Implementación DONE; siguiente paso
aceptación manual + entorno externo, sin más código para scope acordado.

## Historial de preparación (2026-10-04; autoridad: top de `GUI_ROADMAP.md`)

Este crate ya es el companion `agenthd-gui`, junto al CLI; el texto D1 de
abajo es histórico (rechazo de `agenthd gui`, solo lectura/edit-only y D3
pendiente están superados). Settings, editor, create/rename/delete y D3
long jobs implementados. Listener-first `agenthd-operation`, sin polling;
Close solicita cancelación cooperativa mediante emitter real. Snapshots
retenidos en backend y `seq` monotónico permiten recovery; IPC no resuelto
mantiene mutaciones fail-closed. Sin rollback automático.

Checks del writer CRUD, reportados y comprobados en logs: Tauri
`cargo test --lib` **88/88**, con `--features custom-protocol` **88/88**;
`npm run test:unit` **72/72**; `npm run build` PASS. No se rerun Rust ni
frontend aquí. Settings/editor/D3/CRUD visual **PENDIENTE**; ninguna GUI
instalada/lanzada contra árbol actual. Read-only paired GUI↔TUI anterior
**CERRADO** por reporte Linux/CachyOS (no Windows), sin certificar el árbol.

D4 local paired package preparado: ver `../../packaging/arch/README.md`
(snapshot revisado antes de crear, checksum real, sin AUR). Build/test
aislado ya autorizado para etapa posterior, no ejecutado aquí; D5 abierto.
MIT para CLI + GUI; `../../LICENSE`. Node Angular actual:
**^22.22.3 || ^24.15.0 || >=26.0.0** (aquí seleccionado 22.22.3).
Los conteos y límites anteriores debajo se conservan como evidencia histórica,
no son el estado actual ni invalidan este resumen.

Prototipo aislado para evaluar Tauri v2 + Angular como candidato
a la forma GUI del proyecto `agenthd` (decisión D1 de
`GUI_ROADMAP.md`).

**No es** una GUI para el binario de producción; es un spike
opt-in que vive en `spikes/tauri-angular/` y depende del lib
`agenthd` por path. El binario `agenthd` sigue rechazando
`agenthd gui` con exit code 2 — este prototipo no toca esa
ruta, no se acopla a la CLI. **El spike en sí no cierra D1**:
no compara candidatos entre sí, no ejecuta el runtime
gráfico end-to-end, no decide empaquetado. D1 lo cerró el
usuario por elección explícita sobre la base del spike Tauri
existente y su familiaridad con Angular, **sin** comparación
factual entre frameworks (Tauri, GTK4-rs, egui, iced,
servidor web local). Ver
"Estado del spike" abajo y `GUI_ROADMAP.md` → "Decisiones
pendientes → D1".

## Instalación recomendada (orquestadora en raíz)

Para instalar el CLI + companion binario en una sola pasada,
el repositorio raíz ofrece `scripts/install.mjs` (referenciado
en `README.md` → "Install"). Esa orquestadora resuelve el
install root, preflights `cargo` / `npm` / `node`, corre
`npm ci --include=dev` + `npm run build` aquí, confirma que
`dist/agenthd-tauri-angular-spike/browser/index.html` existe, y
ejecuta ambos `cargo install` con el mismo `--root` y la feature
`custom-protocol`. Soporta `--force` para sobreescribir
binarios previos. Las instrucciones de abajo (`npm ci`,
`npm run build`, `cargo install`) son el detalle manual del
paso 3 del contrato en pareado; la orquestadora ya hace todo
eso.

## Qué demuestra

Cinco comandos Tauri: tres read-only + dos mutantes (Fase
5 — slices verticales de mutación pequeña síncrona
aprobados). Todos compuestos sobre el lib `agenthd`
(single source of truth):

1. **`settings_status`** → clasifica `settings.json` en
   `Empty` / `Ready` / `Stale` / `Error` usando
   `agenthd::workflows::read_checkout`. Misma función que
   la pantalla Settings del TUI; misma salida textual.
2. **`list_agents`** → re-lee el checkout configurado vía
   `read_checkout` + `Paths::with_settings` +
   `workflows::list_canonical_agents`, y proyecta
   `name` / `mode` / `model` / `description`.
3. **`apply_checkout`** (Fase 5, slice mutación pequeña
   síncrona de Settings) → envuelve
   `agenthd::workflows::apply_checkout` (el mismo workflow
   que llama la pantalla Settings del TUI) vía un helper
   `compose_apply_checkout(&Paths, &str) -> Result<String,
   String>`. El comando Tauri reenvía esa `Result` literal:
   en `Ok` el frontend recibe el path absoluto (trimmed) que
   el helper acaba de escribir; en `Err` recibe como
   rejection el texto de `ApplyError::message()` intacto
   (mismas cadenas que el TUI). Después de un save exitoso
   el frontend re-invoca `settings_status` + `list_agents`
   para recomponer Settings + Agents; si la revalidación
   post-escritura falla (puede pasar incluso cuando el
   write tuvo éxito), el lib persiste pero devuelve el
   error para que la GUI lo muestre sin rollback (el TUI
   hace lo mismo).
4. **`load_agent_for_edit`** (Fase 5, slice editor edit-only)
   → envuelve `agenthd::store::load_agent_for_edit` (seam
   nuevo en este bloque) vía un helper
   `compose_load_agent_for_edit(&Paths, &str) ->
   Result<(AgentEditDto, AgentEditContext), String>`. El
   seam del lib lee el archivo canónico **una vez** y
   devuelve el `Agent` parseado + el SHA-256 de los mismos
   bytes que consumió el parser; el helper envuelve la
   respuesta en un DTO con todos los campos editables
   (`name`, `description`, `mode`, `model`, `prompt`,
   `permissions`) + un `AgentEditContext` con
   `checkout_path` / `original_name` / `prior_hash` que el
   frontend debe round-trip al save. Errores del lib
   (`changed on disk since this edit started`, symlink
   rechazado, etc.) llegan al frontend verbatim.
5. **`save_agent_edit`** (Fase 5, slice editor edit-only) →
   envuelve `agenthd::workflows::save_agent` (workflow
   pre-existente, extraído en Fase 2) vía un helper
   `compose_save_agent_edit(&Paths, AgentEditContext,
   AgentEditDto)`. El helper añade las precondiciones
   GUI-only antes de invocar `save_agent`: contexto no
   vacío, checkout persistido `Ready` sigue coincidiendo
   con el de apertura (evita agente homónimo en otro
   repo), nombre inmutable (no convierte edit en rename),
   archivo canónico sigue existiendo (no convierte edit en
   alta). Errores del lib llegan al frontend verbatim;
   éxito cierra el editor **antes** del refresh post-write
   para que un refresh fallido no pueda enmascarar el
   éxito del save.

**Lo que NO hace el prototipo** (decisiones explícitas):

- No expone `prompt` ni `permissions` en la proyección
  de la lista (`AgentSummary`). El editor usa un DTO
  separado (`AgentEditDto`) que sí los carga — son
  editables, no son una superficie expuesta por el
  listado read-only.
- No llama a `Paths::ensure_dirs`, no escribe en
  `targets/`/`skills/`/`state.json` desde
  esta superficie. Los únicos writes son
  `apply_checkout` → `save_settings` → `write_target`
  sobre `settings.json`, y `save_agent_edit` →
  `save_agent` → `save_canonical` → `write_target`
  sobre el `.md` editado. `settings.json` y
  `state.json` son byte-exact antes y después de cada
  comando del editor (test pin).
- No incluye rename / create / delete desde la GUI.
  Esos flujos siguen siendo TUI-only hasta que D3 (u
  otra decisión) los habilite; el slice editor es
  **edit-only** sobre agentes ya existentes en el
  checkout configurado.
- No incluye plugins (`fs`, `shell`, `dialog`, `opener`,
  `http`, ...). Solo `core:default` en capabilities.
- No está acoplado al binario `agenthd`. No lee argv. No
  reemplaza a la TUI. La configuración del checkout sigue
  siendo edit-able en la TUI y en la GUI, mismos textos,
  mismo `settings.json`.
- No marca D1 como cerrado.

## Estructura

```
spikes/tauri-angular/
├── README.md                  ← este archivo
├── .gitignore                 ← node_modules/, dist/, target/, ...
├── package.json               ← Angular 22 + @tauri-apps/api
├── angular.json
├── tsconfig.json
├── tsconfig.app.json
├── src/                       ← frontend Angular
│   ├── index.html
│   ├── main.ts
│   ├── styles.css
│   └── app/
│       ├── app.config.ts
│       ├── app.component.{ts,html,css}
└── src-tauri/                 ← backend Tauri v2 (Rust)
    ├── Cargo.toml             ← depende de agenthd por path
    ├── Cargo.lock             ← generado y versionado con el spike (lockfile reproducible)
    ├── build.rs
    ├── tauri.conf.json
    ├── capabilities/
    │   └── default.json       ← solo `core:default`
    ├── icons/                 ← copiados de create-tauri-app
    └── src/
        ├── main.rs
        └── lib.rs             ← 2 #[tauri::command] + tests focales
```

## Prerrequisitos verificados en este host

Comprobado durante la inspección previa al spike y, en una
corrida posterior, en un segundo host. Cada bloque corresponde
a un host distinto; no mezclar.

**Linux (host histórico del spike):**

- `node` 22.22.3
- `npm` 12.0.2
- `cargo` 1.96.0 (edition 2021)
- `pkg-config` 3.0.7
- `webkit2gtk-4.1` presente vía pkg-config (las GTK nativas
  que Tauri necesita en Linux están disponibles sin instalar
  paquetes de sistema)

**Windows 11 MSYS (smoke posterior — ver
`spikes/tauri-angular/EVIDENCE.md` → "Observaciones Windows
11 MSYS"):**

- Host: Windows 11 con MSYS.
- `node` v24.16.0
- `npm` 12.0.1
- `cargo` 1.98.1
- Toolchain Rust: `x86_64-pc-windows-msvc`.
- WebView2 runtime: observado instalado en este host.

Si tu host Linux no tiene las nativas GTK/WebKitGTK, el
comando `cargo build` de `src-tauri/` fallará con un error
de `pkg-config` o de cabecera. **No instales paquetes de
sistema sin permiso del usuario**; el spike debe reportar
el bloqueo exacto.

## Cómo ejecutar el spike

Desde el directorio `spikes/tauri-angular/`:

```bash
# 0) Ya hecho: `npm install` (218 paquetes, 45s) generó
#    `package-lock.json` (195 KB, generado y versionado
#    con el spike).
#    `npm ci` (218 paquetes, 2s) confirma reproducibilidad.
#    `npm run build` generó `dist/agenthd-tauri-angular-spike/`
#    (el directorio `dist/` sigue llamándose por el nombre
#    del proyecto Angular, **no** por el nombre del binario
#    de Tauri; el binario se llama `agenthd-gui`).

# 1) Re-verifica el frontend (idempotente, usa el lockfile):
npm ci
npm run build

# 2) Modo desarrollo (levanta Angular dev server en
#    127.0.0.1:14720 y compila/ejecuta el binario Tauri).
#    Requiere un display usable con WebKitGTK inicializado.
#    Nota: en este host, `DISPLAY=:0` +
#    `timeout 12s ./target/debug/agenthd-tauri-angular-spike`
#    (nombre histórico del binario en ese pase de Linux; el
#    binario pasó después a llamarse `agenthd-gui` vía
#    `[[bin]]` en `src-tauri/Cargo.toml` — la observación
#    aquí registrada es la del momento del pase y no se
#    reescribe)
#    produjo exit 124 con stderr vacío, lo cual NO
#    demuestra que el runtime gráfico funcione. Ver
#    `spikes/tauri-angular/EVIDENCE.md` → "Limitaciones
#    honestas".
#    En el host Windows 11 MSYS posterior, `npm run tauri
#    dev` sí arrancó el dev server, compiló y lanzó el
#    binario, y `Get-Process` observó un
#    `MainWindowHandle` con título `agenthd spike (D1 / Tauri + Angular)`
#    a los 12 s (título **histórico** del momento del smoke; el
#    branding posterior renombró la ventana a `agenthd GUI`, pero
#    la observación aquí registrada es la original, no la
#    renombrada); un run timed fue interrumpido a los 55 s
#    (exit 143) sin observarse contenido Angular ni respuesta IPC
#    — **sigue sin ser** prueba de visualización ni de IPC
#    end-to-end. Ver `spikes/tauri-angular/EVIDENCE.md` →
#    "Observaciones Windows 11 MSYS".
npm run tauri dev

# 3) Build de producción (Angular a dist/, Tauri a bundle).
#    `npm run tauri build` / `cargo tauri build` activan la
#    feature `custom-protocol` por el wrapper; si invocas el
#    backend directamente (sin el wrapper npm), pasa el flag
#    a mano — la feature es opt-in por diseño para preservar
#    `tauri dev`:
#       cd src-tauri
#       cargo build --release --features custom-protocol --bin agenthd-gui
npm run tauri build
```

Para los tests focales del backend Rust (no necesitan
nativas GTK porque son tests puros — `cargo test --lib` no
inicializa WebKit):

```bash
cd src-tauri
cargo test --lib
```

Conteos observados (bloque 2026-10-04 — correcciones
del contracto D3, ver `EVIDENCE.md` → "2026-10-04 —
D3 long-job registry — correcciones del contracto"
para los detalles):

- Sin `--features`: 61/61 OK (33 tests previos del slice
  editor + 28 tests en `jobs::tests` que cubren el
  registry del D3 corregido — id monotonic, reserva
  long/short, cancel pre-terminal / terminal / unknown,
  `current_seq` per-registry (no global estática),
  `install_latest` atómico con seq bump, snapshot
  reads sin bump, `should_block_close` long-active /
  short-busy, spawn-failure injection, catalog
  lookup, `tool_catalog_status` row projection,
  `permission_keys` projection, wire pins).
- Con `--features custom-protocol`: 61/61 OK.
- Con `--locked --offline`: 61/61 OK.

Conteos históricos previos (8/8, 16/16, 54/54) —
citados en secciones anteriores de este README — **no
se reescriben**; siguen siendo los del momento de su
pase. Los conteos actuales son los de la tabla de
arriba.

### Comandos Tauri registrados (post-corrección)

Read-only / corto:
- `settings_status`, `list_agents`, `load_agent_for_edit`
  (existentes)
- `tool_catalog_status`, `permission_keys` (nuevos, backend-
  autoritativos; reemplazan los hardcodes `TOOL_CATALOG`
  y `KNOWN_PERMISSION_KEYS` del frontend)

Mutantes cortos (guarda `try_reserve_short` integrada):
- `apply_checkout`, `save_agent_edit` (existentes; usan
  `guarded_short` para reservar antes de delegar a
  `compose_apply_checkout` / `compose_save_agent_edit`)

D3 long-job:
- `op_start(request)` → `Result<LongOutcome, OperationError>`
  con `LongOutcome = { job_id, view: CurrentView }`
- `op_current()` → `CurrentView` (no-nullable)
- `op_status(job_id)` → `Result<CurrentView, OperationError>`
- `op_cancel(job_id)` → `Result<CurrentView, OperationError>`
  (idempotente; `UnknownJob` solo para ids desconocidos)

Los aliases `op_apply_checkout` / `op_save_agent_edit`
del slice previo se **eliminaron**: el frontend usa
las versiones originales (`apply_checkout` /
`save_agent_edit`) que comparten la misma guarda de
reserva corta. Una sola ruta de escritura, un solo
path de validación, sin DTOs paralelos.

Para los tests del frontend (DTO + component reducer
del D3, sin DOM / sin Angular runtime / sin IPC real):

```bash
cd spikes/tauri-angular
npm run test:unit
# 36/36 OK (21 DTO tests previos + 15 component tests
# nuevos que cubren subscribe-before-current /
# event-before-start-reply / terminal-before-old-running /
# new-event-before-old-null / old-job-after-new-job /
# registration-fail-then-retry / destruction-late-cleanup /
# uncertain-start-cancel-reconcile / gate-all-mutations /
# dirty-draft-list-empty / editor-save-error /
# discovered-models-don't-overwrite-model /
# terminal-dupe-one-refresh + static-template
# editor-hoisted assert).
```

Los tests usan `tempfile::TempDir` para construir `Paths`
aislados (mismo patrón que los tests del lib `agenthd`); no
mutan `HOME` ni `XDG_CONFIG_HOME`. Cada test invoca los
composition helpers (`compose_settings_status`,
`compose_agents_list`) que los comandos Tauri envuelven
tras `Paths::from_env()`. Los nuevos tests del D3
cubren el `OperationRegistry` con el mismo patrón
(`Paths::resolve(...)` raw en un `tempfile::TempDir`),
sin tocar el env.

## Decisiones de arquitectura observables en el código

- **`pub use agenthd::...` no `mod`**: el bin de `agenthd`
  ya adoptó este patrón en `src/main.rs` (etapa 1 del spike).
  El Tauri crate hace `use agenthd::agent::Mode`,
  `use agenthd::store::Paths`, `use agenthd::workflows::*`,
  nunca re-implementa la lógica.
- **`#[serde(tag = "kind", rename_all = "snake_case")]`** en
  `SettingsStatus` y la discriminated-union correspondiente
  en `app.component.ts`: mantiene el contrato Rust ↔ TS
  explícito en un solo lugar por lado.
- **CSP estricta con `connect-src` mínima para IPC**: la
  directiva `connect-src: ipc: http://ipc.localhost` es
  requerida por Tauri v2 para que la IPC JS↔Rust funcione —
  sin ella los `invoke()` desde Angular serían bloqueados
  por el CSP en runtime. CSP de producción (`security.csp`):
  `default-src 'self'; connect-src: ipc: http://ipc.localhost;
  img-src: 'self' data:; style-src: 'self' 'unsafe-inline';
  script-src: 'self'`. CSP de dev (`security.devCsp`,
  inyectada solo durante `tauri dev`, no en builds):
  añade `ws://127.0.0.1:14720 http://127.0.0.1:14720` para
  el HMR de Vite/Angular. No se cargan recursos remotos en
  producción; los assets de Angular bundled en `dist/` son
  la única fuente. Fuentes: documentación oficial de Tauri
  v2 (<https://v2.tauri.app/security/csp/>) y
  `SecurityConfig::devCsp` en el schema 2.12. **El runtime
  IPC no se ha comprobado end-to-end** — la corrección se
  basa en documentación oficial.
- **Capabilities mínimas**: `["core:default"]` solamente. El
  frontend no puede acceder al filesystem del host, abrir
  URLs externas, ni lanzar procesos. Solo invoca los
  comandos definidos aquí. El evento `agenthd-operation`
  del D3 usa `core:default::listen` / `unlisten` —
  ningún permiso extra.

- **D3 wire: `agenthd-operation` (evento único)**.
  `jobs.rs` define `pub const OPERATION_EVENT: &str =
  "agenthd-operation"`. El backend emite
  `CurrentView` (snapshot completo, no delta) con
  `seq: String` (BigInt en JS) y `Option<JobSnapshot>`.
  El frontend registra un único `listen<CurrentView>`
  y aplica los snapshots en un reducer
  (`applyCurrentView`) que descarta los de `seq <= lastSeq`.
  El registry es process-global: `tauri::Builder::manage(Arc<OperationRegistry>)`
  + `app.state()` desde cada comando, y un
  `Arc::clone` hacia cada worker. Sin polling, sin
  timers. La reserva long/short es exclusiva: una
  segunda `start_operation` mientras hay un long
  activo devuelve `OperationError::Busy { active_job_id }`;
  un long activo bloquea el `request_apply_checkout`
  / `request_save_agent_edit` síncronos, y viceversa.
  Los workers son `std::thread::Builder::spawn`,
  no Tauri-managed tasks, y se ejecutan FUERA del
  lock del registry. Sin auto-rollback: el
  `OperationReport::partial` queda tal cual
  (skipped / failed rows preservados) y el reducer
  del frontend lo renderiza sin aplanar.

## Estado del spike

> **Nota posterior:** D1 (forma GUI) se cerró por
> elección explícita del usuario (Tauri + Angular), sin
> comparación factual con el resto de candidatos; ver
> `GUI_ROADMAP.md` → "Decisiones pendientes → D1". Este
> spike sigue siendo la pieza de evidencia del candidato
> elegido. La IPC visual queda como gate del primer
> slice de Fase 5 (no como gate de D1); la
> verificación por plataforma Arch / Windows queda
> como gate de Fase 6 (D4 pendiente).

**Registro al cierre del spike (antes de la elección
del usuario):** D1 aún no estaba cerrado. Este spike es
una pieza de evidencia técnica: ejercita el seam del
lib y deja los lockfiles generados y versionados con el
spike para que el siguiente agente pueda reproducir el
build sin surprises. **No** valida el runtime gráfico
(ver `EVIDENCE.md` → "Limitaciones honestas": en este
host `DISPLAY=:0` + `timeout 12s` produjo exit 124 con
stderr vacío, lo cual NO demuestra visualización ni IPC/
ventana funcional — esta limitación se **registra**, no
convierte la ejecución gráfica en gate de D1) y **no**
se había ejecutado en Arch ni en Windows (queda para
Fase 6 de empaquetado; **NO es gate de D1**). La propuesta de
cierre de D1 en ese momento era (a) comparar
pros/contras factuales Tauri vs GTK4-rs vs egui vs iced
vs web local tomando como entrada lo que el spike
produce de verdad (scaffold reproducible, 8/8 tests
focales del backend, lockfiles generados y versionados
con el spike, lockfile doble, dependencia cruzada por
path, pros/contras observables), más las limitaciones
honestas registradas (runtime gráfico no demostrado en
este host, tests del backend no invocan el dispatch
Tauri), y (b) la aceptación explícita del usuario. El
siguiente paso propuesto entonces era esa comparación
de pros/contras, no invertir en el slice vertical de
Fase 5 (Fase 5 venía después de cerrar D1); esa
propuesta fue superada por la elección explícita del
usuario documentada arriba.

## Layout visual (redesign 2026-10-04)

El frontend ahora usa un sidebar fijo de 210px a la
izquierda con un `<ul>` de nav nativo (`aria-current="page"`
en el botón activo) y una workspace a la derecha con
header (breadcrumb, título, descripción, contador, badge
semántico) y panel global compacto de operación + conexión
siempre visible. Las vistas lógicas — Agents, Sync &
Plans, Tools, Models, Settings, Job — se ocultan con
`[hidden]` (CSS fuerza `display: none !important`); los
drafts del editor, las confirmaciones armadas, el panel Job
y el catálogo de tools se preservan al navegar. El panel
global expone los botones Refresh status / Cancel / Retry
connection, accesibles aunque la vista activa no sea Job y
aunque `mutationsEnabled` sea `false` o `startPending` esté
activo.

La nav la maneja una sola `signal<ViewId>` en el
componente (`setView(id)` es el único mutador); sin router,
sin framework CSS, sin nuevas deps. La paleta es dark
slate con acento azul (CSS-only, fuentes del sistema,
sin assets remotos); el layout colapsa a sidebar top-bar
horizontal bajo 760px y las celdas usan `min-width: 0` /
`.table-wrap` con scroll interno para evitar overflow
horizontal del body en paths largos. Contraste WCAG AA
(numérico, `app-component.test.mjs`): texto blanco sobre
botón primary `--primary` #1565c0 (default) = **5.75:1**,
sobre `--primary-hover` #2479c8 (hover) = **4.52:1**, ambos
≥4.5; el `<select>` de permisos dentro del editor lleva
`aria-label` key-specific (`'Permission mode for ' + key`)
para distinguir filas a screen readers, **visual unverified**
hasta el gate visual (no `tauri dev` en esta ronda).
Tests del slice: **92/92 OK**
(`TMPDIR=/tmp/opencode npm_config_cache=/tmp/opencode/npm-validation-cache npm run test:unit`);
build OK (bundle 195.57 kB raw / 51.88 kB transfer).
Los detalles del slice y los conteos están en
`EVIDENCE.md` → "2026-10-04 — GUI presentation redesign".
Logs en `/tmp/opencode/gui-redesign-a11y-{unit,build}.log`.
