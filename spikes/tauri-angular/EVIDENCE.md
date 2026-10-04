# Spike D1 / Tauri v2 + Angular — Evidencia observada

## Última evidencia — paquete e installer reales, aceptación UI pendiente

Matriz actual: `../../packaging/arch/VALIDATION.md`. Recipe remap aprobado,
nuevo archive/hash/build offline CachyOS **SUCCESS**, parent-verified;
artifact anterior conservado como histórico. Warning `$srcdir` eliminado.
Run enfocado actual **43 PASS** (29 installer + 14 packaging), log
`/tmp/opencode/arch-remap-fixture-tests.log`; Bash syntax y diff check PASS.
No rerun root/Tauri/Node/Angular en este slice. Fixtures no certifican live IPC
de `AppHandle<Wry>`; no nueva feature de test Tauri.

Autoridad: nueva sección superior de `../../GUI_ROADMAP.md`; límites de
preparación inferiores son históricos. Root **576 PASS, 1 ignored**, Tauri
default/custom-protocol **95/95 cada uno**, Node **84**, Angular build PASS,
installer **29** + packaging **10** = **39** histórico, superado por 43 arriba.
Snapshot aprobado `/tmp/opencode/agenthd-arch-remapped/agenthd-0.1.0.tar.gz`:
SHA-256 `8188cc343e0b8da48210059521419ffe4d5fd1c87451fafce7ae2e77f66567b7`.
Paquete `agenthd-0.1.0-1-x86_64.pkg.tar.zst` allí: SHA-256
`ef721fde48ed4f84ec221c5864af24606d6da4287b79264000768dc35c0fa380`.
Código aprobado incluido, docs de este append posteriores/no archivados;
working-tree sources no committed, no bit-identidad con docs actuales.
`arch-remap-package-build.log` y `paired-install-final.log` bajo `/tmp/opencode/`:
makepkg offline CachyOS e installer paired custom-protocol **SUCCESS**.
Source verification Passed; `/tmp/opencode/arch-remap-extracted`: binarios 755, licencia 644 + cmp PASS,
ldd sin missing. `--nodeps --noconfirm` por cargo no registrado en pacman
(rustc 1.96 presente), no clean chroot/dependency-check completo. WARN `$srcdir`
histórico arch-final ausente ahora; tres root Tools dead-code baseline.
Strings ambos binarios: cero nuevo build-source; GUI cuatro rutas esperadas
`/usr/src/debug/agenthd/agenthd-0.1.0/src/`, sin binary patch/warning filters. No reproducibilidad de binarios
ni all-platform native. Caches copiados aislados, sin red/host install/AUR.
Fixture `/tmp/opencode/agenthd-gui-smoke` preparado con snapshot agents+skills,
HOME/XDG nuevos y stub models 10s; launch + termination del proceso GUI histórico arch-final
reportado por padre vía notificación shell SSH, log redirigido 0 bytes.
Sin screens/DOM/IPC inspection ni aceptación manual UI. Procedimiento/comandos
en fixture README. Gates visuales actuales pendientes; Discovery fixture no
certifica OpenCode real, Tools remoto pendiente. Read-only Linux gate previo
sigue cerrado; estado real user externo `e4e8…` no tocado, no claim `978…` intacto.
GUI remapped **no lanzado**. CLI packaged negativo: `agenthd gui --repo missingcheckout`,
exit 1, stderr `agenthd: resolve configured checkout`, sin children HOME/XDG;
stdout/stderr en `/tmp/opencode/arch-cli-negative-Nyfcgx`.
Implementación DONE; próximo paso aceptación manual + entorno externo,
sin más cambios de código necesarios para scope automatizable acordado.

## Historial 2026-10-04 — D3 + CRUD y preparación D4

Autoridad: sección superior de `GUI_ROADMAP.md`. La evidencia del cuerpo
pertenece a sus sesiones (Linux genérico/Windows MSYS incluidos); no se
reescribe como prueba del árbol actual. Claims anteriores D3 pendiente,
Close solo bloqueado, emitter no-op, edit-only/CRUD TUI-only y conteos
61/36 quedan **superados** por las correcciones + CRUD actuales.

Validación nueva del slice plans/inventory read-only, que conserva D3 + CRUD:

| Check | Resultado | Log bajo `/tmp/opencode/` |
| --- | --- | --- |
| Tauri manifest: `cargo fmt -- --check` | PASS | `gui-plans-fmt.log` |
| Tauri manifest: `cargo test --lib --locked --offline` | 95 passed | `gui-plans-cargo-default.log` |
| Tauri manifest: `cargo test --lib --locked --offline --features custom-protocol` | 95 passed | `gui-plans-cargo-custom.log` |
| Frontend: `TMPDIR=/tmp/opencode npm run test:unit` | 84 passed | `gui-plans-unit.log` |
| Frontend: `TMPDIR=/tmp/opencode npm run build` | PASS, sin warnings | `gui-plans-angular-build.log` |

Rust usa `TMPDIR=/tmp/opencode` + `CARGO_TARGET_DIR=/tmp/opencode/cargo-target`.
`operation_plan` solo admite sync_agents (opencode/pi) e install_skills;
otros requests se rechazan antes de resolver environment. Helper con RAW
Paths revalida Ready, scope checkout persistido y State.load; DTO copia
labels/paths/hashes/reason del root, sin nuevas reglas ni escrituras.
Siete tests nuevos fijan decoy canonical no leído, destinos per-target,
ownership removals Skills, árbol completo sin cambios, wire/registration,
errores fail-closed y vacío válido (skills/ ausente no es vacío válido).
Nueve tests nuevos del componente real fijan requests read-only, labels
wire/render bindings, error + retained STALE, guards destroy/connection,
bootstrap y terminal completed/cancelled/failed una vez, RefreshAll con
Settings empty y Tools independiente, draft preservado y destroy temprano.
Planes advisory no ejecutables/no atómicos; op_start sigue replanteando.
Sin controles force/overwrite/rollback, sin cambios a workflows mutantes.
Sin GUI visual/runtime, red, real HOME, instalación ni commit en este slice.

El fix CRUD anterior usa `deepCopyDto`/`sameDto`: cambios de prompt, permisos, descripción,
mode y modelo escrito/elegido rearman rename/delete antes de invocar.
Sus tres regresiones siguen verdes en esta corrida; sin prueba visual.
Root **576 ok, 1 ignored** es último resultado reportado anterior, con
cinco modelos GPT aprobados y prueba causal fixture; no rerun ni cambios
root en el slice read-only. Checks realizados
en preparación: `node --check scripts/package-arch.mjs`,
`node --check scripts/package-arch.test.mjs`, `TMPDIR=/tmp/opencode node
--test scripts/{install,package-arch}.test.mjs` **39/39** (29 installer,
10 packaging), JSON manifests + Bash `-n` del recipe de fixture. El fixture
verifica bytes tracked modificados/new untracked, exclusiones, rechazo de
symlinks/special candidates, hashes deterministas/changed source, SHA real,
validación de output y no-overwrite. No snapshot final del árbol actual.

D3 usa listener/events, no polling; Close solicita cancelación cooperativa
por emitter real. Recovery usa snapshot backend retenido y secuencia
monotónica, fail-closed ante IPC recovery sin resolver. Settings/editor/
D3/CRUD visual **PENDIENTE**: no GUI instalada ni lanzada contra árbol
actual. Gate read-only paired GUI↔TUI sigue **CERRADO**, user-reported
Linux/CachyOS, no Windows; no prueba binarios actuales ni IPC/DOM visual.

Host de preparación confirmado `/etc/os-release`: CachyOS; makepkg/tar/gzip
disponibles, rustc 1.96.0 y Node seleccionado 22.22.3. Angular requiere
**^22.22.3 || ^24.15.0 || >=26.0.0**. D4: template paired + generator
local + tests, sin AUR/remote source URL. Próxima etapa de build/test aislado
ya autorizada, no ejecutada aquí; sin makepkg completo, instalación ni
runtime. Snapshots reproducibles, no binarios reproducibles/offline vendored.
Yank confirmado de `yoke-derive` 0.8.3 no invalida pins de `--locked`;
caches/network pueden faltar; Rust <1.87 tiene problema conocido.
MIT CLI + GUI (`Copyright (c) 2026 taneralberto`), sin upgrade de deps.
D5 sigue abierto. Ver `../../packaging/arch/README.md` para workflow local.

Recopilado durante la etapa 2 del spike D1. Las
verificaciones del cuerpo de este documento se ejecutaron
en host **Linux genérico** (no Arch específica) y son la
**verdad ejecutada**, no una proyección de lo que debería
pasar: las verificaciones que terminan con `OK` se corrieron
de verdad; las que terminan con `N/A` no se aplicaron a ese
host. Tras la elección del usuario sobre D1, se añadió una
corrida posterior en host **Windows 11 con MSYS** cuyo
registro (smoke parcial) vive en "Observaciones Windows 11
MSYS" más abajo — **no** sustituye ni contradice las
observaciones Linux; agrega lo que ese nuevo host permite
atestiguar sin afirmar lo que no atestigua.

## Plantilla oficial inspeccionada (sin instalar sistema)

Comando ejecutado en `/tmp/opencode/cta-template/` (no se
instalaron paquetes de sistema, solo `npx create-tauri-app`
en directorio temporal):

```bash
cd /tmp/opencode/cta-template
npm create tauri-app@4.7.4 -- \
  --yes --template angular --manager npm \
  --identifier com.tauri-inspect.dev --tauri-version 2
```

Plantilla generada por **create-tauri-app@4.7.4** con
**Angular 22.0.1** y **Tauri 2** (CLI `@tauri-apps/cli@^2`).
Versiones detectadas:

- `@angular/common`, `@angular/core`, `@angular/compiler`:
  `^22.0.1`
- `@angular/build`, `@angular/cli`, `@angular/compiler-cli`:
  `^22.0.1`
- `rxjs`: `~7.8.2`
- `tslib`: `^2.8.1`
- `typescript`: `~6.0.3`
- `@tauri-apps/api`: `^2`
- `@tauri-apps/cli`: `^2`
- `tauri` (Rust): `version = "2", features = []`
- `tauri-build` (Rust): `version = "2", features = []`
- `tauri-plugin-opener` (Rust, plantilla): `"2"` —
  **omitido en el spike** (no usamos opener)

## Prerrequisitos nativos en este host

Verificado en este host con `pkg-config --list-all`:

- `pkg-config` 3.0.7 ✓
- `webkit2gtk-4.1` y `webkit2gtk-web-extension-4.1` ✓
- `gtk+-3.0`, `gdk-3.0` ✓

**No se instaló ningún paquete de sistema en este host.** Si
tu host no tiene `webkit2gtk-4.1`, `cargo build` de
`src-tauri/` falla con `Package webkit2gtk-4.1 was not found
in the pkg-config search path` — el spike lo reporta, no
instala nada.

## Capacidades y CSP aplicadas (en archivos del spike)

- `tauri.conf.json` → `security.csp` =
  `"default-src 'self'; connect-src: ipc: http://ipc.localhost; img-src: 'self' data:; style-src: 'self' 'unsafe-inline'; script-src: 'self'"`.
  La directiva `connect-src: ipc: http://ipc.localhost` es la
  mínima requerida por Tauri v2 para que la IPC funcione —
  sin ella los `invoke()` desde Angular fallarían en runtime
  con un error de CSP. Fuente: documentación oficial
  <https://v2.tauri.app/security/csp/>.
- `tauri.conf.json` → `security.devCsp` =
  `"...; connect-src: ipc: http://ipc.localhost ws://127.0.0.1:14720 http://127.0.0.1:14720; ..."`.
  Este CSP solo se inyecta durante `tauri dev`. Habilita
  `ws://127.0.0.1:14720` (Vite/Angular HMR vía WebSocket)
  y `http://127.0.0.1:14720` (Angular dev server). No se usa
  en builds de producción (`tauri build` solo usa `csp`).
  Fuente: schema oficial <https://schema.tauri.app/config/2>
  (`SecurityConfig::devCsp`, Tauri 2.12).
- `capabilities/default.json` → `permissions: ["core:default"]`
  solamente. Cero permisos de plugin: ni `fs`, ni `shell`,
  ni `dialog`, ni `opener`, ni `http`.

## Verificaciones frontend (Angular 22)

Ejecutadas en `spikes/tauri-angular/`:

| Check | Comando | Resultado REAL |
|---|---|---|
| Lockfile generado | `npm install --no-audit --no-fund` | **OK**: 218 paquetes, `package-lock.json` (195 KB) creado |
| Lockfile reproducible | `npm ci --no-audit --no-fund` | **OK**: 218 paquetes instalados desde lockfile en 2s |
| Build Angular | `npm run build` | **OK**: bundle 209 KB initial / 57 KB gzipped, en `dist/agenthd-tauri-angular-spike/browser/` (index.html, main-*.js, styles-*.css). Tiempo: 1.3s |

El `package-lock.json` queda generado y versionado con el
spike en `spikes/tauri-angular/package-lock.json` para
reproducibilidad. Mismo estado para
`spikes/tauri-angular/src-tauri/Cargo.lock`.

## Verificaciones backend Rust (Tauri v2)

Ejecutadas en `spikes/tauri-angular/src-tauri/`:

| Check | Comando | Resultado REAL |
|---|---|---|
| Compilación lib | `cargo check` | **OK** |
| Compilación binario | `cargo build` | **OK** (2.64s; binario 193 MB debug en `target/debug/agenthd-tauri-angular-spike` — nombre histórico del binario en ese pase de Linux; el binario pasó después a llamarse `agenthd-gui` vía `[[bin]]` en `src-tauri/Cargo.toml`, pero la observación aquí registrada es la del momento del pase y no se reescribe) |
| Tests focales | `cargo test --lib` | **OK**: 8/8 tests pasan (ver detalle abajo) |
| Clippy | `cargo clippy --lib --tests` | **Sin warnings nuevos del spike** (solo los 3 warnings pre-existentes del lib `agenthd`) |

### Tests focales del backend (8/8 OK)

Los tests ejercen los composition helpers (`compose_settings_status(&Paths)` y `compose_agents_list(&Paths)`) que los comandos Tauri envuelven tras `Paths::from_env()`. Ningún test llama a `read_checkout` directo — todos pasan por el mismo helper que el runtime en producción.

| Test | Qué verifica |
|---|---|
| `compose_settings_status_empty` | Sin `settings.json` → `SettingsStatus::Empty` y `settings_file` correcto |
| `compose_settings_status_ready` | Con checkout válido → `SettingsStatus::Ready { checkout_path }` igual al path configurado |
| `compose_settings_status_stale` | Path inválido → `SettingsStatus::Stale { banner, raw_path }` con banner que contiene "does not exist" |
| `compose_agents_list_ready_projects_one_summary` | Checkout Ready con un agente → `AgentsList { agents: [1 elemento], error: None }`; campos `name/mode/model/description` correctos |
| `compose_agents_list_empty_surfaces_friendly_error` | Empty → error contiene "no checkout configured yet" |
| `compose_agents_list_stale_blocks_listing` | Stale → error contiene "configured checkout is unusable" |
| `projection_drops_prompt_and_permissions` | `Agent → AgentSummary` descarta `prompt`/`permissions`. Construye un `Agent` con `prompt="SECRET PROMPT..."` y `permissions={bash:allow, read:deny}`, lo proyecta, y verifica que el JSON serializado NO contiene ni "SECRET PROMPT" ni "bash"/"read", y que las keys son exactamente `{description, mode, model, name}` |
| `ready_checkout_json_has_no_prompt_or_permission_leak` | End-to-end: settings.json válido + agente con prompt "TOP SECRET" + permissions `{bash:allow, edit:deny}` → settings JSON y agents JSON no contienen "TOP SECRET"/"bash"/"edit" como keys, y los campos top-level son `{settings_file, status}` y `{agents, error}` respectivamente |

Ambos comandos Tauri (`settings_status`, `list_agents`) son
wrappers de un solo nivel sobre `Paths::from_env()` + helper
de composición. La rama de fallo de `Paths::from_env()` está
manejada en el comando (no en el helper), para mantener el
helper puro y testeable sin entorno.

## Compatibilidad cross-platform (factual)

Observaciones sobre la documentación oficial de Tauri v2 (no
probadas en este host, que es Linux genérico):

### Arch Linux

`pacman -S webkit2gtk-4.1 base-devel curl wget file openssl
appmenu-gtk-module libgtk-3 libappindicator-gtk3 librsvg` es
lo que la documentación oficial de Tauri v2 lista. La
planta del spike no lo prueba aquí (no es Arch); la
verificación queda para Fase 6 de empaquetado.

### Otros Linux

- Debian / Ubuntu: `libwebkit2gtk-4.1-dev`, `build-essential`,
  `curl`, `wget`, `file`, `libssl-dev`,
  `libayatana-appindicator3-dev`, `librsvg2-dev`.
- Fedora: `webkit2gtk4.1-devel`, `openssl-devel`, `curl`,
  `wget`, `file`, `libappindicator-gtk3-devel`,
  `librsvg2-devel`, `gcc`.

### Windows

- WebView2 runtime (incluido en Windows 11; instalable en
  Windows 10). El bin necesita MSVC toolchain. El spike
  ya genera `main.rs` con
  `#![cfg_attr(not(debug_assertions), windows_subsystem =
  "windows")]`. WebView2 runtime **observado instalado** en
  el host Windows 11 MSYS de la corrida posterior — ver
  "Observaciones Windows 11 MSYS" más abajo. Esa corrida
  **no** verifica IPC end-to-end ni visualización real:
  enumera handles de ventana del proceso vía
  `Get-Process` y nada más.

## Pros y contras observables (D1 cerrado por elección del usuario)

**Pros** (evidencia directa de este spike):

- El lib `agenthd` ya expone todo lo que el spike necesita
  (`read_checkout`, `list_canonical_agents`, `Paths`,
  `Settings`, `Agent`, `Mode`) — la inversión del seam
  etapa 1 paga de inmediato.
- Angular 22 trae standalone components y signals por
  defecto; el spike es un solo `AppComponent` con dos
  `signal()` por panel.
- CSP estricta + capabilities mínimas dan una superficie
  muy pequeña de validar en términos de seguridad.
- `cargo check`, `cargo build`, `cargo test`, `cargo clippy`,
  `npm install`, `npm ci`, `npm run build` **todos verdes**
  en este host sin instalar nada.

**Contras** (evidencia directa):

- Plantilla Tauri incluye `tauri-plugin-opener` por defecto;
  mantener CERO plugins requiere editar `package.json`,
  `Cargo.toml` y `capabilities/default.json` en cada
  scaffold.
- Lockfile doble: `agenthd/Cargo.lock` raíz y
  `spikes/tauri-angular/src-tauri/Cargo.lock` (115 KB) +
  `spikes/tauri-angular/package-lock.json` (195 KB).
  Revisar conflictos cuando cambien versiones.
- Compilación pesada: `cargo build` del binario arrastra
  tauri + wry + webkit2gtk-sys solo para el spike (no
  contamina el workspace raíz — el spike es un crate
  independiente).
- Compatibilidad cross-platform no validada en Arch /
  Windows en este host (queda para Fase 6 de empaquetado;
  **NO es gate de D1** — el cierre de D1 observado fue
  por elección explícita del usuario apoyada en este
  spike (scaffold reproducible, 8/8 tests focales del
  backend, lockfiles generados y versionados con el
  spike, lockfile doble, dependencia cruzada por path,
  pros/contras observables) más las limitaciones
  honestas registradas, sin comparación factual con el
  resto de candidatos; Arch/Windows reales entran al
  gate de Fase 6, no al gate D1).

## Observaciones Windows 11 MSYS

Smoke parcial registrado tras la elección del usuario sobre
D1, en host **Windows 11 con MSYS** y toolchain Rust
`x86_64-pc-windows-msvc`. **No** es una verificación
end-to-end del runtime gráfico ni del IPC JS↔Rust — ver
"Limitaciones honestas" para el conjunto completo de lo que
**no** se demuestra. Esta sección registra lo atestiguado
en esa corrida sin reinterpretar lo del host Linux.

### Entorno verificado en este host

- **Host:** Windows 11 con MSYS.
- **Toolchain Rust:** `x86_64-pc-windows-msvc`.
- `node` **v24.16.0**, `npm` **12.0.1**, `cargo` **1.98.1**.
- **WebView2 runtime:** observado instalado en este host
  (Windows 11 incluye WebView2 Evergreen Runtime por
  defecto; en Windows 10 sería instalable por separado).

### Verificaciones frontend / backend (smoke parcial)

| Check | Comando | Resultado REAL |
|---|---|---|
| Lockfile reproducible | `npm ci --no-audit --no-fund` | **OK**: 218 paquetes instalados desde lockfile, con warnings de `install-scripts` blocked (observación documental, no afecta al smoke del frontend) |
| Build Angular | `npm run build` | **OK**: bundle **116.20 kB initial / 34.81 kB transfer** en `dist/agenthd-tauri-angular-spike/browser/`. Distinto del tamaño observado en Linux (209 KB initial / 57 KB gzipped) — la causa de la diferencia **no se establece** en este spike |
| Tests focales backend | `cargo test --lib` | **OK**: 8/8 tests pasan en Windows (target `x86_64-pc-windows-msvc`), mismos nombres y misma semántica que en Linux |

### Smoke de `npm run tauri dev`

- `npm run tauri dev` arrancó el dev server en
  `127.0.0.1:14720`, compiló el binario y lo lanzó.
- `PowerShell Get-Process` a los **12 s** del arranque
  observó para el proceso:
  - `MainWindowHandle = 656192`
  - título `agenthd spike (D1 / Tauri + Angular)` (título
    observado en esa corrida; el branding posterior
    renombró la ventana a `agenthd GUI`, pero la
    observación aquí registrada es la del momento del
    smoke y no se reescribe)
  - `PID 11624`
- La ejecución timed fue **interrumpida a los 55 s** (exit
  143 / SIGTERM). Durante esa ventana **no se observó
  contenido Angular ni respuesta IPC**: no se invocó
  `settings_status` ni `list_agents` desde el WebView2 con
  respuesta observable, no se inspeccionó el DOM, no se
  tomó screenshot.
- Un retry anterior **falló con `HRESULT 0x800700AA`
   (`ERROR_BUSY`, "resource in use")** tras un run timed
   previo; el run posterior creó el window handle documentado
   arriba. La causa del `ERROR_BUSY` en el retry inicial
   **no se establece** en este spike; queda como observación
   sin atribuir.

### Lo que este smoke **NO** demuestra

- **No** demuestra que Arch o Windows estén empaquetados
  (queda para Fase 6; el spike es un scaffold reproducible,
  no un artefacto firmado por plataforma).
- **No** demuestra IPC end-to-end: ningún `invoke(
  'settings_status')` ni `invoke('list_agents')` desde
  Angular con respuesta observable del backend fue
  registrado durante la ventana de 55 s.
- **No** demuestra visualización real **desde la
  perspectiva del agente**: `Get-Process` solo enumera
  el handle de ventana del proceso; el contenido del
  WebView2, los eventos de IPC, o que Angular cargara
  y mostrara Settings / Agents siguen **sin observación
  directa** por parte del agente (sin screenshot, sin
  inspección del DOM, sin captura de payloads IPC, sin
  apertura de devtools, sin inspección de red). El
  usuario, **por separado**, reportó manualmente que
  Settings y Agents aparecen y que Refresh both
  actualiza — esa confirmación está documentada en
  "Verificación manual user-reported" más abajo y
  **no** es instrumentación del agente.
- **No** declara la GUI lista para producción.

### Verificación manual user-reported (no instrumentada por el agente)

Tras el smoke registrado arriba, el **usuario** abrió
`npm run tauri dev` en un escritorio Windows 11
interactivo y reportó manualmente, en respuesta a la
solicitud del agente ("confirma visualmente Settings /
Agents y Refresh both"), lo siguiente sobre esa sesión:

- **Settings aparece** en la GUI.
- **Agents aparece** en la GUI.
- **Refresh both** (etiqueta del botón en el
  `AppComponent` del spike) **actualiza** ambos
  paneles.

Esta verificación es **user-reported, manual, no
independientemente instrumentada ni capturada por el
agente** durante esa corrida. El agente no operó la
ventana, no tomó screenshots, no inspeccionó el DOM,
no capturó payloads de IPC, no abrió devtools, no
observó tráfico de red, ni verificó el contenido
específico mostrado en cada panel. El reporte del
usuario es la única evidencia observable de esta
sesión interactiva para esos tres puntos.

**Lo que la confirmación user-reported NO establece:**

- **No** inspecciona payloads de IPC: los argumentos
  y resultados de `invoke('settings_status')` /
  `invoke('list_agents')` no fueron capturados ni
  observados.
- **No** inspecciona CSP: las directivas `csp` /
  `devCsp` configuradas en `tauri.conf.json` no fueron
  ejercitadas por el agente en runtime; su corrección
  sigue basada en documentación oficial, no en
  verificación end-to-end instrumentada.
- **No** inspecciona devtools, ni tráfico de red, ni
  consola del WebView2.
- **No** afirma contenido específico: el reporte
  del usuario cubre **aparición** de los paneles y la
  **actualización** ante "Refresh both", pero **no**
  describe los datos concretos mostrados (paths,
  estados de checkout, listas de agentes, etc.).
- **No** ejecuta el runtime empaquetado en otras
  plataformas (Arch / otros Linux): la confirmación
  es del host Windows 11 interactivo del usuario; los
  gates de Fase 6 (D4 pendiente) **siguen abiertos**.
- **No** declara la GUI lista para producción por sí
  sola: la entrada `agenthd gui` sigue rechazándose
  con exit `2` antes de cualquier efecto (D2
  cerrado) hasta que un agente con permisos de
  implementación integre la rama de arranque de la
  GUI en `src/main.rs`. La integración del comando
  `agenthd gui` en producción **puede proceder** una
  vez tomada la **decisión de arquitectura de
  launch/install** (cómo se arranca y distribuye la
  GUI) que este spike no prescribe; el slice de
  producción read-only de Fase 5 requerirá su propia
  validación al integrarse (tests runtime, contratos
  de IPC ejercitados, comportamiento de la TUI
  conservado, etc.) — esa validación es parte del
  trabajo del slice, **no** un bloqueo pendiente
  sobre el gate visual aquí satisfecho. El gate
  visual del primer slice de Fase 5 (Settings y
  Agents aparecen; Refresh both actualiza) queda
  **satisfecho** por la confirmación manual
  user-reported; un eventual probe automatizado
  end-to-end, si se ejecuta, sería validación
  adicional para profundizar confianza, no cierre
  formal obligatorio.

### Próximo paso explícito

La confirmación visual manual del usuario (Settings y
Agents aparecen; Refresh both actualiza — ver
"Verificación manual user-reported") **queda
registrada y satisface** el gate visual del primer
slice read-only de Fase 5: aparición de los paneles
Settings y Agents y refresco manual ante "Refresh
both" quedan atestiguados por el reporte manual del
usuario en una sesión `npm run tauri dev` interactiva
en Windows 11. La atribución es honesta — la sesión
no fue instrumentada por el agente (sin screenshots,
sin captura de payloads IPC, sin inspección del DOM
/ CSP / devtools / red, sin afirmación de contenido
específico mostrado) — pero **sí** establece lo que
el usuario atestiguó sobre esos tres puntos, y con
eso el gate visual del primer slice de Fase 5 queda
**satisfecho**.

Eso habilita el siguiente paso concreto: la
descripción e implementación del slice read-only de
Fase 5 (`compose_settings_status` +
`compose_agents_list` reusando el lib, sin
side-effects) por un agente con permisos de
implementación (ver `GUI_ROADMAP.md` → "Próxima
tarea estrecha → 5"). **Slice "binario acompañante"
ejecutado:** la rama `agenthd gui` en `src/main.rs`
ahora localiza el binario `agenthd-gui` adyacente al
ejecutable actual y, cuando está presente, lo lanza
con el mismo argv reenviando su código de salida;
cuando falta, sale con exit `2` y un stderr claro
**antes** de cualquier escritura de `settings.json`.
El binario del spike pasa a llamarse `agenthd-gui`
(vía `[[bin]]` en `src-tauri/Cargo.toml`), y el
branding "spike" se baja en `productName` /
`identifier` / título de ventana / `<h1>` Angular;
los comandos, capabilities, CSP y comportamiento
read-only se quedan como están. La integración de
`agenthd gui` en producción **puede proceder** una
vez tomada la **decisión de arquitectura de
launch/install** (cómo se arranca y distribuye la
GUI) que este spike no prescribe; el slice de
producción read-only **requerirá su propia
validación** al integrarse (tests runtime, contratos
de IPC ejercitados, comportamiento de la TUI
conservado, etc.) — esa validación es parte del
trabajo del slice, **no** un bloqueo pendiente sobre
este gate visual.

**Mientras tanto**, la rama actual de `src/main.rs`
rechaza `agenthd gui` con exit `2` cuando el
acompañante falta; cuando el acompañante está
presente, lo lanza y reenvía su código de salida (no
rechaza con exit `2` por "no implementado" como
antes — la rama GUI ya no es un placeholder). Esto
se mantiene intacto hasta que un agente con permisos de
implementación integre la rama de arranque de la
GUI. Los gates de Fase 6 (D4 pendiente — empaquetado
Arch / otros Linux / Windows) **siguen abiertos** y
son **separados** del gate visual del primer slice
de Fase 5 satisfecho aquí; un eventual probe
automatizado end-to-end, si se ejecuta en el futuro,
sería validación adicional para profundizar
confianza, **no** cierre formal obligatorio de este
gate.

### Relación con el resto del spike

- Las observaciones frontend / backend de Linux histórico
  (tablas anteriores de este documento) **siguen siendo
  válidas** para ese host; el smoke de Windows 11 MSYS no
  las reemplaza ni las contradice.
- El smoke automatizado del agente **no es** la
  verificación visual del primer slice de Fase 5:
  reduce incertidumbre sobre el arranque del binario
  y la presencia de WebView2, pero no sobre el
  contenido del WebView ni sobre el IPC end-to-end.
  **Sí** queda registrada la confirmación manual
  user-reported del usuario (Settings y Agents
  aparecen; Refresh both actualiza — ver
  "Verificación manual user-reported"): cubre
  **aparición de los paneles y actualización**, con
  atribución honesta — no es instrumentación
  automatizada del agente (no captura payloads IPC,
  no inspecciona CSP / devtools / red, no afirma
  contenido específico). El gate visual del primer
  slice de Fase 5 queda **satisfecho** por esta
  confirmación user-reported. Un eventual probe
  automatizado end-to-end, si se ejecuta, sería
  validación adicional para profundizar confianza,
  pero **no** es cierre formal obligatorio de este
  gate. El gate de empaquetado por plataforma (Fase
  6, D4 pendiente) **sigue abierto** y es
  **independiente** del gate visual satisfecho aquí.

## Limitaciones honestas

- **Intento de ejecución del binario en este host, sin
  prueba de visualización ni IPC/ventana.** El host tiene
  `DISPLAY=:0`; se ejecutó
  `timeout 12s ./target/debug/agenthd-tauri-angular-spike`
  dentro de `src-tauri/`. El proceso corrió hasta el
  timeout (`exit 124`), con `stderr` vacío. **Esto NO
  demuestra que la ventana se renderizara, que el IPC
  estuviera funcional, ni que los comandos Tauri
  respondieran**: solo prueba que el proceso no terminó
  por sí solo en 12s y que no escribió a stderr durante
  ese intervalo. El runtime gráfico (WebKitGTK, IPC JS↔Rust,
  `#[tauri::command]` dispatch, capabilities/CSP aplicadas)
  **queda sin validar en este host**. La verificación
  end-to-end de la UI sigue pendiente para `npm run tauri
  dev` en una sesión interactiva con display usable, o
  para CI con xvfb. La tarea de spike es producir el
  prototipo, no validar manualmente el runtime gráfico.
- **Los 8 tests del backend NO invocan los comandos Tauri
  directamente**; invocan los `compose_*` helpers que los
  comandos llaman. La equivalencia es 1:1 (cada comando es
  `Paths::from_env()` + helper), pero el dispatch real de
  Tauri (serialización JSON, argumentos nombrados, errores
  serializados) solo se ejercita en `npm run tauri dev`.
- **El runtime IPC sigue sin comprobarse.** La directiva
  `connect-src: ipc: http://ipc.localhost` en `csp` (y su
  extensión `ws://127.0.0.1:14720 http://127.0.0.1:14720` en
  `devCsp`) se configuró siguiendo la documentación oficial
  de Tauri v2 (<https://v2.tauri.app/security/csp/> y
  `SecurityConfig::devCsp` en el schema 2.12), pero **no
  se ha verificado que un `invoke('settings_status')` desde
  Angular llegue al backend**: eso requiere la ventana
  nativa ejecutándose, que este host no soporta. La
  corrección se basa en evidencia documental, no en
  evidencia de ejecución end-to-end.
- **Smoke Windows 11 MSYS registrado, no ejecución
  validada.** Una corrida posterior en Windows 11 con
  MSYS (ver "Observaciones Windows 11 MSYS") atestigua
  que `npm run tauri dev` arrancó el dev server, compiló,
  lanzó el binario y que `Get-Process` observó un
  `MainWindowHandle` con título `agenthd spike (D1 /
  Tauri + Angular)` a los 12 s; un run timed fue
  interrumpido a los 55 s (exit 143) sin que se
  observara contenido Angular ni respuesta IPC. **Esto
  reduce incertidumbre** sobre arranque del binario,
  WebView2 presente y creación de ventana nativa, **pero
  no demuestra** visualización real del WebView2, IPC
  end-to-end, ni que la GUI esté lista para producción.
  El retry inicial con `HRESULT 0x800700AA` (`ERROR_BUSY`)
  queda como observación sin causa establecida en este
  spike.
- **Confirmación manual user-reported (no
  instrumentada por el agente).** En una sesión
  interactiva posterior en el mismo host, el usuario
  reportó manualmente (sin screenshots, sin captura
  de payloads IPC, sin inspección del DOM / CSP /
  devtools / red) que Settings y Agents aparecen y
  que Refresh both actualiza. Esa confirmación está
  documentada en "Observaciones Windows 11 MSYS →
  Verificación manual user-reported" y **no**
  equivale a instrumentación del agente: **no**
  inspecciona payloads, **no** inspecciona CSP,
  **no** inspecciona devtools, **no** inspecciona
  red, **no** afirma contenido específico mostrado,
  y **no** equivale a Windows empaquetado validado
  (gate de Fase 6, D4 pendiente, **sigue abierto**
  y es **separado** del gate visual de Fase 5). Sí
  **satisface** el gate visual del primer slice
  read-only de Fase 5 (aparición de Settings y
  Agents, refresco manual ante "Refresh both"), con
  la atribución honesta ya registrada. No habilita
  por sí sola la integración de `agenthd gui` en
  producción: esa integración requiere además la
  **decisión de arquitectura de launch/install**
  (cómo se arranca y distribuye la GUI, fuera del
  scope de este spike) y la validación propia del
  slice de producción read-only al integrarse
  (tests runtime, contratos de IPC ejercitados,
  comportamiento de la TUI conservado) — esa
  validación es parte del trabajo del slice y **no**
  un bloqueo pendiente sobre el gate visual
  satisfecho aquí.

## Diagnóstico: colisión IPv4/IPv6 en 1420 y migración a 14720

El puerto por defecto que la plantilla `create-tauri-app`
genera para el dev server es `localhost:1420`. En este host
se confirmó una colisión: hay otro servicio escuchando
en `1420` que responde a `::1` (IPv6 loopback) cuando el
cliente resuelve `localhost` a `::1` antes que a `127.0.0.1`
(comportamiento por defecto de `getaddrinfo` en glibc cuando
`/etc/hosts` lista primero `::1 localhost`). Al intentar
arrancar el spike con la configuración de plantilla, el
dev server de Angular quedaba desplazado por el servicio
ajeno y el binario Tauri, al apuntar a `http://localhost:1420`,
cargaba el contenido del otro servidor (no el bundle de
Angular), haciendo imposible distinguir una falla real del
runtime de una falla de configuración. Diagnosticado esto,
se optó por (a) fijar la IP literal `127.0.0.1` (evita la
resolución a `::1`) y (b) mover el puerto a `14720` (evita
la colisión con el servicio en `1420`).

Cambios aplicados para reflejar esta decisión:

- `spikes/tauri-angular/angular.json` → `serve.host =
  "127.0.0.1"` y `serve.port = 14720`. Sin `localhost`, sin
  `1420`.
- `spikes/tauri-angular/src-tauri/tauri.conf.json` →
  `build.devUrl = "http://127.0.0.1:14720"` y
  `security.devCsp.connect-src = "ipc: http://ipc.localhost
  ws://127.0.0.1:14720 http://127.0.0.1:14720"` (los tres
  literales alineados con `angular.json`; `devUrl` y
  `devCsp` coinciden exactamente, condición necesaria para
  que el `WebView` no sea bloqueado por CSP ni redirija a
  otro origen).

**Lo que el cambio de puerto demuestra:** sortea la colisión
con el servicio en `1420` y alinea `devUrl`/`devCsp`/
`angular.json` con un origen inequívoco en `127.0.0.1:14720`.

**Lo que el cambio de puerto NO demuestra:** que el WebView
de Tauri renderice la UI ni que el IPC JS↔Rust funcione. El
runtime gráfico sigue sin validarse en este host (mismo
motivo que la sección "Limitaciones honestas": la ventana
nativa no se ha podido ejecutar en este entorno headless /
sin display usable). La migración a `14720` es una
**prerrequisito** para intentar la validación end-to-end
del runtime, no una prueba de que el runtime renderice.

## Cómo validar el spike end-to-end

```bash
cd spikes/tauri-angular
npm install        # ya hecho, lockfile generado y versionado con el spike
npm ci             # ya hecho, valida reproducibilidad
npm run build      # ya hecho, genera dist/

cd src-tauri
cargo test --lib   # 8/8 OK
cargo build        # genera binario
```

Para abrir la ventana nativa (requiere display usable:
en este host, `DISPLAY=:0` + `timeout 12s` produjo exit
124 con stderr vacío, lo cual **no** prueba que el
runtime gráfico funcione — ver "Limitaciones honestas".
En el host Windows 11 MSYS posterior, `npm run tauri dev`
sí arrancó el dev server, compiló y lanzó el binario, y
`Get-Process` a los 12 s observó un `MainWindowHandle`
con título `agenthd spike (D1 / Tauri + Angular)` — esto
**sigue sin ser** prueba de visualización ni de IPC, solo
atestigua que el proceso y la ventana existen; ver
"Observaciones Windows 11 MSYS"):

```bash
cd spikes/tauri-angular
npm run tauri dev
```

**Gate visual del primer slice de Fase 5.** El
usuario **ya reportó manualmente** que Settings y
Agents aparecen y que Refresh both actualiza (ver
"Verificación manual user-reported" en
"Observaciones Windows 11 MSYS"): esa confirmación
**es user-reported**, **no** es instrumentación
automatizada end-to-end (no captura payloads IPC,
no inspecciona CSP / devtools / red, no afirma
contenido específico), y **sí satisface** el gate
visual del primer slice de Fase 5 (aparición de los
paneles Settings y Agents; refresco manual ante
"Refresh both"). Un eventual probe automatizado
end-to-end, si se ejecuta en el futuro, sería
validación adicional para profundizar confianza,
pero **no** es cierre formal obligatorio de este
gate, que queda satisfecho por la confirmación
user-reported. La rama actual de `src/main.rs`
rechaza `agenthd gui` con exit `2` antes de
cualquier efecto (D2 cerrado) y eso se mantiene
intacto hasta que un agente con permisos de
implementación integre la rama de arranque de la
GUI. La integración de `agenthd gui` en producción
**puede proceder** una vez tomada la **decisión de
arquitectura de launch/install** (cómo se arranca
y distribuye la GUI) que este spike no prescribe;
el slice de producción read-only requerirá su
propia validación al integrarse. El gate de
empaquetado por plataforma (Fase 6, D4 pendiente)
**sigue abierto** y es **separado** del gate visual
de Fase 5 satisfecho aquí.

Estado de los lockfiles en git: ambos
(`spikes/tauri-angular/src-tauri/Cargo.lock` y
`spikes/tauri-angular/package-lock.json`) están
generados y versionados con el spike para garantizar
reproducibilidad.

## Defecto user-reported y fix acotado

- **Síntoma user-reported:** "Hmmm… can't reach this
  page" en la GUI `agenthd-gui` instalada por el
  usuario. La causa operativa propuesta es que el
  `cargo install --path src-tauri --locked --bin
  agenthd-gui` previo no pasó `--features
  custom-protocol`; sin la feature `tauri-macros` deja
  `dev = cfg!(not(feature = "custom-protocol"))` en
  `true` y el codegen cae en la rama de dev (no se
  observaron strings específicos de assets ni capturas
  que demuestren ausencia de contenido).
- **Fix:** `spikes/tauri-angular/src-tauri/Cargo.toml`
  añade `[features] custom-protocol =
  ["tauri/custom-protocol"]` sin `default`. README
  raíz paso 3 y spike README paso 3 pasan a requerir
  `--features custom-protocol` en `cargo install` /
  `cargo build` directos. Comandos históricos no
  reescritos.
- **Validación:** `cargo tree -e features` con la
  feature activa `tauri custom-protocol` y `tauri-
  macros custom-protocol`; sin la feature ninguna
  aparece. `cargo test --lib` 8/8 OK con y sin
  feature. `npm run build` regenera `dist/`. `cargo
  install --locked --bin agenthd-gui --features
  custom-protocol --root <temp>` produce el binario.
  `cargo fmt --check` FALLÓ por diffs preexistentes
  en `src/lib.rs` (no introducidos por este fix;
  código ajeno no tocado). Binarios en `~/.cargo/bin/`
  no reemplazados.
- **Gate visual PENDIENTE.** El fix elimina la causa
  operativa propuesta a nivel de codegen / comando;
  la confirmación visual de que la WebView sirve el
  bundle Angular desde el binario de producción
  instalado sigue siendo tarea aparte del usuario
  contra la configuración real compartida con la TUI.

## 2026-10-04 — D3 long-job registry + frontend events reducer (bloque actual)

Implementación de la decisión D3 aprobada por el usuario:
eventos cooperativos `agenthd-operation` en lugar de
polling / timers. Backend: nuevo módulo
`spikes/tauri-angular/src-tauri/src/jobs.rs` con el
`OperationRegistry` (mutex de id / token / latest /
reservation), el `CurrentView` snapshot
(`seq: String` + `Option<JobSnapshot>`), el
`OperationRequest` enum con `serde(tag = "kind",
deny_unknown_fields)`, los workers `std::thread`
para los cuatro workflows largos
(`plan_then_apply_agents_safe_controlled` /
`plan_then_apply_skills_controlled` /
`install_tool_controlled` /
`discover_models_controlled`), la reserva long/short
exclusiva, la emisión del evento `agenthd-operation`
fuera del lock del registry, y la retención del
terminal hasta que un `op_current` o `op_status` lo
relea. Sin auto-rollback: los outcomes parciales
quedan en `OperationReport::partial` tal cual
(skipped / failed rows preservados). Los nuevos
comandos Tauri son `op_start` / `op_current` /
`op_status` / `op_cancel` / `op_apply_checkout` /
`op_save_agent_edit`; los comandos previos del slice
editor (`load_agent_for_edit` / `save_agent_edit` /
`list_agents` / `settings_status` / `apply_checkout`)
siguen siendo el path síncrono corto y se conservan
intactos. Capabilities sin cambios: solo
`core:default` (el `listen` / `unlisten` que usa el
nuevo `agenthd-operation` ya viene en
`core:default`).

Frontend: nuevo `bootstrap()` que llama
`op_current` **primero** y registra el listener
`agenthd-operation` **después** (LISTENER-before-
current del contracto D3); un solo reducer
`applyCurrentView` con comparación `BigInt` de `seq`
para descartar eventos stale; generación incrementada
en `OnDestroy` para que un `listen()` que resuelva
tarde se desuscriba inmediatamente; `mutationsEnabled`
se desactiva a `false` cuando `subscribe` o el primer
`op_current` fallan, y solo se re-habilita con un
`retryConnection` explícito del usuario. P1 fix: el
bloque del editor (template `agent-editor`) está
hoisted **fuera** del gate de membresía
`agentsError/agents().length`; el draft se mantiene
visible aunque la lista quede vacía por un refresh
que la limpie. Conflicto dirty-draft: la primera
preferencia es `globalThis.confirm`; cuando no está
disponible (entorno sin DOM) cae a un arm
inline de dos pasos (auto-clear a 5s). Nueva UI:
botones Agent-Sync (OpenCode / Pi), Skills (OpenCode
only), Tools (picker sobre el catálogo del lib),
Discovery, panel derecho de progress + report
(`RowOutcome` para `sync_agents` / `install_skills`,
`ToolTerminal` para `install_tool`,
`DiscoveryTerminal` para `discover_models`). Gate
visual NUEVO del slice D3 (Save → sync → ver el
mismo archivo en TUI paired sobre mismo HOME)
**sigue PENDIENTE**; sigue siendo user-reported
contra la config real compartida, sin shadow copy,
sin instrumentación automatizada, mismo gate que
los slices previos.

### Conteos observados (fecha del bloque)

| Suite | Comando | Resultado REAL |
|---|---|---|
| Backend Rust (lib, --locked --offline) | `cd src-tauri && cargo test --lib` | **OK**: 54/54 tests (33 tests previos del slice editor + 21 tests nuevos en `jobs::tests` que cubren id monotonic, reserva long/long, long/short, short/long, short/short, cancel pre-terminal / terminal / unknown, vista con `seq` monotonic, lookup de catálogo conocido/desconocido, round-trip DTO de `dto_into_agent` con clave conocida y rechazo de clave desconocida, release de la reserva corta en error de save, serialización de `OperationRequest` con `kind` discriminant + `deny_unknown_fields`, `seq` como `String` en el wire) |
| Backend Rust (con custom-protocol) | `cargo test --lib --features custom-protocol` | **OK**: 54/54 tests |
| Backend Rust (clippy baseline) | `cargo clippy --lib --tests --offline` | **OK**: 0 warnings nuevos del spike (los 3 warnings pre-existentes del lib `agenthd`: `FakeSeamGuard` / `fake_npm_cli_guard` / `fake_npm_missing_guard` en `src/tools/mod.rs`) |
| Frontend (DTO + component) | `npm run test:unit` (= `node --experimental-strip-types --no-warnings --test editor-dto.test.mjs app-component.test.mjs`) | **OK**: 36/36 tests (21 DTO tests previos + 15 component tests nuevos que cubren subscribe-before-current / event-before-start-reply / terminal-before-old-running / new-event-before-old-null / old-job-after-new-job / registration-fail-then-retry / destruction-late-cleanup / uncertain-start-cancel-reconcile / gate-all-mutations / dirty-draft-list-empty / editor-save-error / discovered-models-don't-overwrite-model / terminal-dupe-one-refresh + static-template editor-hoisted assert) |
| Frontend (Angular build) | `npm run build` | **OK**: bundle 152.58 kB raw / 42.91 kB transfer (CSS 1.54 kB / 498 B), en `dist/agenthd-tauri-angular-spike/`. Tiempo ~1.0s |
| `cargo fmt --check` | `cd src-tauri && cargo fmt --check` | **OK**: limpio |

Los conteos anteriores del mismo spike (8/8 Tauri
backend, 21/21 DTO Node, etc.) — citados en
secciones previas de este EVIDENCE — siguen siendo
los del momento de su pase; **no se reescriben**.
Los conteos **actuales** son los de la tabla de
arriba (54 Tauri + 36 Node).

### Lo que este bloque **NO** demuestra

- **No** se ha ejecutado la GUI en una sesión real
  (no se lanzó `tauri dev`, no se observó el WebView,
  no se inspeccionaron payloads IPC contra el
  backend, no se tomó screenshot). El gate visual
  del slice D3 sigue PENDIENTE, mismo status que los
  slices previos.
- **No** se ha ejecutado el runtime contra el HOME
  real del host. Tests usan `tempfile::TempDir` con
  `Paths::resolve(...)` raw y no mutan `HOME` /
  `XDG_CONFIG_HOME`. El gate paired GUI↔TUI sigue
  PENDIENTE, mismo status que los slices previos.
- **No** se prueba la rama de `pi-psql` install
  contra un registry npm real: el harness del
  test `start_operation_publishes_running_snapshot_then_terminal`
  no tiene `opencode` en PATH, así que el worker
  reporta `Failed` (esperado) — el test fija la
  forma del snapshot terminal, no la ejecución
  real del install.
- **D4 (PKGBUILD Arch/CachyOS-first)** sigue
  pendiente: este bloque implementa la lógica del
  registry, no el empaquetado.
- **D5 (compatibilidad cross-platform)** sigue
  pendiente: schema y rutas sin migrar anunciado,
  pruebas en `CachyOS` / otros Linux / Windows
  pendientes. Solo se valida que el código
  compila + tests pasan en este host (Linux
  genérico con `target-dir` aislado).
- **CRUD pequeño (rename / create / delete)** y
  **PKG block** están explícitamente fuera de
  scope de este bloque; el scope del bloque es
  D3 GUI jobs + panels + original component
  state tests.

## 2026-10-04 — D3 long-job registry — correcciones del contracto

Bloque correctivo aplicado al slice D3 del 2026-10-04
previo. Las correcciones cierran las brechas del
contracto detectadas en la revisión: el backend
`jobs.rs` se reescribió para que `current_seq` viva
dentro del registry (no en una estática global
compartida entre fixtures), `op_current` /
`op_status` / `op_cancel` / `op_start` ahora
devuelven `CurrentView` (envelope común con el
evento `agenthd-operation`), el `apply` /
`save_agent_edit` se reusó con una sola guarda de
reserva corta (sin alias paralelos), se eliminó la
duplicación de `AgentEditDto` / `AgentEditContext`
/ `dto_into_agent` en `jobs.rs` (single source of
truth vía `compose_save_agent_edit` y
`dto_into_agent` en `lib.rs`), el catálogo de
herramientas y las permission keys son
backend-authoritativos (`tool_catalog_status` /
`permission_keys`), `CloseRequested` se conecta al
registry vía `on_window_event` y `should_block_close`,
y se añadió un seam inyectable para ejercitar
spawn-failures en tests sin agotar cuotas de
threads.

Frontend (`app.component.ts`): `bootstrap` se
invirtió al LISTENER-first (registra el listener
primero, después `op_current`); `mutationsEnabled`
arranca en `false` y solo se activa cuando ambas
subscripciones están sanas; el `seq` se compara con
regex decimal estricta y `BigInt` (sin overflow
silencioso de `u64`); el reducer consume el
`view` retornado por `op_start` / `op_current` /
`op_status` / `op_cancel` (mismo envelope que
`agenthd-operation`); `LongOutcome` cambió a
`{job_id, view: CurrentView}`; el `jobPending` se
eliminó y la UI deriva `activeJob()` /
`activeJobId()` de `currentView().job.phase` (nunca
asigna ids tardíos); `refreshAll` ya no hace
implicit discard del draft (preserva el editor
abierto); el catálogo y los known permission keys
vienen del backend vía `invoke("tool_catalog_status")`
e `invoke("permission_keys")` (no más hardcode).

Tests: el harness `DeferredInvoke` ahora matchea
handlers por command (no por índice) y maneja
invocaciones fire-and-forget (`tool_catalog_status`
+ `permission_keys`) sin colgarse; los tests
component-level cubren LISTENER-first, seq
monotonicidad estricta, `event-before-start-reply`
(reducer acepta el evento antes de la respuesta
de `op_start`), `terminal-dupe-one-refresh` (re-fetch
descartado por monotonicidad), `uncertain-start-
cancel-reconcile` (cancel idempotente consume
`CurrentView`), y `dirty-draft-list-empty` (refresh
no cierra el editor).

### Conteos observados (fecha del bloque)

| Suite | Comando | Resultado REAL |
|---|---|---|
| Backend Rust (lib, --locked --offline) | `cd src-tauri && cargo test --lib` | **OK**: 61/61 tests (33 tests previos del slice editor + 28 tests en `jobs::tests` que cubren seq monotonic, current_view sin bump, install_latest atómico, reservation long/short, cancel pre-terminal / terminal / unknown, `should_block_close` long-active / short-busy, spawn-failure injection, catalog lookup, wire projection pins, op_current / op_status, tool catalog status row, permission keys) |
| Backend Rust (con custom-protocol) | `cargo test --lib --features custom-protocol` | **OK**: 61/61 tests |
| Backend Rust (clippy baseline) | `cargo clippy --lib --tests --offline` | **OK**: 0 warnings nuevos del spike (los 3 warnings pre-existentes del lib `agenthd` siguen) |
| Frontend (DTO + component) | `npm run test:unit` (= `node --experimental-strip-types --no-warnings --test editor-dto.test.mjs app-component.test.mjs`) | **OK**: 36/36 tests (21 DTO tests + 15 component tests) |
| Frontend (Angular build) | `npm run build` | **OK**: bundle 153.18 kB raw / 42.93 kB transfer (CSS 1.54 kB / 498 B), en `dist/agenthd-tauri-angular-spike/`. Tiempo ~1.1s |
| `cargo fmt --check` | `cd src-tauri && cargo fmt --check` | **OK**: limpio |
| Root crate | `cd /home/lukateric/dev/agent-handler && cargo test --all-targets` | **OK**: 288+65+5+215 = **573 ok, 1 ignored** (no ediciones en el root) |

### Comandos Tauri registrados (post-corrección)

Read-only / corto:
- `settings_status` (existente)
- `list_agents` (existente)
- `load_agent_for_edit` (existente)
- `tool_catalog_status` (nuevo, backend-authoritativo)
- `permission_keys` (nuevo, backend-authoritativo)

Mutantes cortos (guarda `try_reserve_short` integrada en cada uno):
- `apply_checkout` (existente; usa `guarded_short` para reservar antes de `compose_apply_checkout`)
- `save_agent_edit` (existente; usa `guarded_short` para reservar antes de `compose_save_agent_edit`)

D3 long-job:
- `op_start(request)` → `Result<LongOutcome, OperationError>` donde `LongOutcome = { job_id, view: CurrentView }`
- `op_current()` → `CurrentView` (no-nullable, `seq:"0",job:null` en registry fresco)
- `op_status(job_id)` → `Result<CurrentView, OperationError>` (`UnknownJob` para ids desconocidos)
- `op_cancel(job_id)` → `Result<CurrentView, OperationError>` (idempotente: cancel pre-terminal y terminal son `Ok(CurrentView)`; `UnknownJob` solo para ids desconocidos)

Los aliases `op_apply_checkout` / `op_save_agent_edit`
del slice previo **se eliminaron**: el frontend usa
las versiones originales (`apply_checkout` /
`save_agent_edit`) que comparten la misma guarda
de reserva corta. Una sola ruta de escritura,
un solo path de validación, sin DTOs paralelos.

### Lo que este bloque **NO** demuestra

- **No** se ha ejecutado la GUI en una sesión real
  (no se lanzó `tauri dev`, no se observó el WebView,
  no se inspeccionaron payloads IPC contra el
  backend, no se tomó screenshot). El gate visual
  D3 sigue PENDIENTE, mismo status que los slices
  previos.
- **No** se ha ejecutado el runtime contra el HOME
  real del host. Tests usan `tempfile::TempDir` con
  `Paths::resolve(...)` raw y no mutan `HOME` /
  `XDG_CONFIG_HOME`. El gate paired GUI↔TUI sigue
  PENDIENTE, mismo status que los slices previos.
- **No** se prueba la rama de `pi-psql` install
  contra un registry npm real: el harness del
  test `start_operation_returns_view_with_running_snapshot`
  no tiene `opencode` en PATH, así que el worker
  reporta `Failed` (esperado) — el test fija la
  forma del snapshot terminal, no la ejecución
  real del install.
- **D4 (PKGBUILD Arch/CachyOS-first)** sigue
  pendiente.
- **D5 (compatibilidad cross-platform)** sigue
  pendiente.
- **CRUD pequeño (rename / create / delete)** y
  **PKG block** están explícitamente fuera de
  scope; el slice D3 sigue siendo
  cooperative-partial-no-rollback sobre los cuatro
  workflows largos pre-aprobados.
- **Settings / Agents / Editor** gate manual
  user-reported (sin shadow copy) sigue
  PENDIENTE; el read-only gate visual del primer
  slice de Fase 5 (anterior) sigue CERRADO según
  el user-reported del pase de Windows 11, y no
  se reabre.

## 2026-10-04 — GUI presentation redesign (sidebar + view nav)

Bloque de rediseño visual del frontend Angular del spike,
con alcance **limitado** a los archivos del scope acordado:
`src/app/app.component.{ts,html,css}`, `src/styles.css` y
`app-component.test.mjs`. Sin nuevas deps, sin cambios de
router / backend / schema / IPC / reglas de negocio.
Comportamiento del componente, mutaciones, gates, drafts,
confirmaciones, drains y reducer se preservan verbatim —
los métodos públicos (`refreshAll`, `refreshSettings`,
`refreshAgents`, `saveCheckout`, `openEditor`, `saveEditor`,
`deleteEditor`, `discardEditor`, `newAgent`, `startSyncAgents`,
`startInstallSkills`, `startInstallTool`, `startDiscoverModels`,
`cancelActiveJob`, `refreshJobStatus`, `retryConnection`,
`applyDiscoveredModel`, etc.) siguen siendo los originales
con la misma firma.

Decisiones del rediseño:

- **Sidebar fijo 210px** (`grid-template-columns: 210px 1fr`)
  con `<aside class="sidebar">` y un único `<ul class="sidebar-nav">`
  de nav nativo. Cada botón es un `<button type="button">` con
  `aria-current="page"` cuando `activeView() === id`. Sin
  router, sin componentes, sin `<nav>` con helpers.
- **`activeView = signal<ViewId>("agents")` es la única
  fuente de verdad del nav** (`ViewId = "agents" | "sync" |
  "tools" | "models" | "settings" | "job"`). `setView(id)` es
  el único mutador — no toca IPC, no toca draft, no arma
  confirmaciones. Sync y Plans agrupados en una sola entrada
  (`sync`) según la directiva.
- **Secciones ocultas con `[hidden]`** (no eliminadas del
  DOM); CSS fuerza `[hidden] { display: none !important }`
  para vencer layouts de grid/flex. Drafts del editor, planes
  read-only y panel Job se mantienen montados aunque el
  usuario cambie de vista.
- **Panel global compacto de operación + conexión**, siempre
  visible encima del contenido: badge del estado del listener
  (`registered` / `not registered`), badge de mutaciones
  (`enabled` / `disabled`), badge del job actual (idle /
  starting / running con `jobLabel()` / last-job terminal), y
  los botones `Refresh status` / `Cancel` / `Retry connection`
  — los tres quedan accesibles aunque la vista activa sea otra,
  incluso durante `startPending`, incluso con `mutationsEnabled
  === false`. El template usa `cancelActiveJob()` /
  `refreshJobStatus()` / `retryConnection()` directamente
  sobre la raíz del componente, fuera de cualquier panel
  `[hidden]`.
- **Warnings persistentes en el sidebar**: `viewBadgeWarning`
  expone errores de Settings / Agents / Job / Tools / Models
  como badges de aviso junto al nav, así una falla en un
  panel oculto no se pierde al navegar.
- **Header de la workspace** con `workspace-crumbs`
  (ruta breadcrumb), `h2` con el título de la vista activa,
  descripción corta, contador de items y badge semántico
  (`accent` / `warning` / `danger`) derivado del estado del
  componente.
- **Dark slate + acento azul**: paleta CSS-only sin assets
  remotos, fuentes del sistema, sin framework. `prefers-
  reduced-motion` reduce las animaciones (pulse del dot
  running). Focus-visible con outline azul en todos los
  controles. AA contrast verificado manualmente contra
  `--slate-bg` (#0f1419).
- **Responsive**: `>760px` sidebar 210px + workspace;
  `<760px` sidebar compacto en una fila horizontal arriba
  con scroll horizontal; `min-width: 0` en `.workspace`,
  `.workspace-content` y celdas de tabla para evitar
  overflow horizontal del body; tablas usan `.table-wrap`
  con `overflow-x: auto` para scroll interno en paths
  largos; `form-grid` y `.agent-editor .row` colapsan a
  una columna en mobile.
- **Formularios estructurados**: `<input>`, `<select>`,
  `<textarea>` nativos restilados; `<label>` asociados por
  `for=` / `[attr.for]`; `mode` / `permissions` / `tools` /
  `plan target` mantienen los vocabularios cerrados del
  backend; `model` sigue siendo free-text. El modelo
  descubierto aplica solo bajo acción explícita del
  usuario.
- **Cards planas con jerarquía**: `.card` (header + body +
  footer) en lugar de `<section>` gigantes con `<h2>`;
  cada vista agrupa sus cards. Header usa `h3` + meta;
  badges en `.card-actions`. Sin Tabs no-scrollbar,
  sin wrappers de "panel".

### Conteos observados (fecha del bloque)

| Suite | Comando | Resultado REAL |
|---|---|---|
| Frontend (DTO + component + view-nav) | `TMPDIR=/tmp/opencode npm_config_cache=/tmp/opencode/npm-validation-cache npm run test:unit` | **OK**: 92/92 tests |
| Frontend (Angular build) | `TMPDIR=/tmp/opencode npm_config_cache=/tmp/opencode/npm-validation-cache npm run build` | **OK**: bundle 195.57 kB raw / 51.88 kB transfer, CSS 13.73 kB / 2.66 kB transfer; build time ~1.124s |
| `git diff --check` (5 archivos del scope) | `git diff --check spikes/tauri-angular/src/app/app.component.{ts,html,css} spikes/tauri-angular/src/styles.css spikes/tauri-angular/app-component.test.mjs` | **OK**: clean |
| Contraste WCAG (AA) | texto blanco sobre botón primary `--primary` #1565c0 (default): **5.75:1** · texto blanco sobre `--primary-hover` #2479c8 (hover): **4.52:1** — ambos ≥4.5 AA | **OK** (cálculo numérico WCAG 2.x, `app-component.test.mjs`); visual **unverified** (no `tauri dev` / screenshot) |
| Permission label review | `aria-label` key-specific en cada `<select>` de permisos dentro del editor (`'Permission mode for ' + key` en `app.component.html`), distingue filas para screen readers; tests = **3 regresiones accesibilidad** (permission `<select>` aria-label, primary button background pair WCAG, primary button rule tokens) en `app-component.test.mjs`; **no** la presentación renderizada | **fixed** in code; visual **unverified** (gate visual user-reported pendiente) |

Logs en `/tmp/opencode/gui-redesign-a11y-{unit,build}.log`.

### Lo que este bloque **NO** demuestra

- **No** se ha ejecutado la GUI en una sesión real (no
  `tauri dev`, no screenshot, no inspección IPC contra el
  backend, no afirmación visual). El gate visual del slice
  rediseñado queda user-reported pendiente, mismo status
  que los slices previos (no se reabre por sí solo). Las
  92 tests cubren el comportamiento de la nav a nivel de
  estado del componente, **no** la presentación real
  renderizada. El review de labels primarios del sidebar
  (sin nombres de funciones del backend, sin prosa larga
  de tests) está aplicado en código y queda **visual
  unverified** hasta el gate visual.
- **No** se introdujeron nuevas deps, frameworks CSS,
  helpers de class management, component tree, router,
  mock de `document` / `window` / timers. La nav es una
  signal + un `<ul>` + `[hidden]`.
- **No** se modificaron las reglas de negocio: redacción
  de los mensajes (canonical-only, non-atomic, partials,
  stale hash warnings, action target paths) se conserva
  en `help`/`details`. Solo se removieron los nombres de
  funciones del backend y la prosa larga de tests de los
  labels primarios de los headings del sidebar.
- **No** se introdujo auto-discard, auto-save, ni IPC
  trigger en navegación. `setView` solo flipa la signal
  de UI; los drafts, las confirmaciones armadas, el job
  panel, el discovery cache y el tool picker se preservan
  intactos.
- **No** se modificaron Rust / Tauri / Cargo / window
  config / package.json / package-lock.json / install /
  network / GUI launch. El slice es frontend-only.
- **D4 / D5** siguen pendientes, mismo status que los
  slices previos.
