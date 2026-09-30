# Spike D1 / Tauri v2 + Angular — Evidencia observada

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
