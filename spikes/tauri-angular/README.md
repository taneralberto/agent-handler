# Spike D1: Tauri v2 + Angular sobre `agenthd`

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

Tres comandos Tauri, dos estrictamente read-only y uno
mutante (Fase 5 — slice vertical de mutación pequeña
síncrona aprobado). Todos compuestos sobre el lib `agenthd`
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
   síncrona) → envuelve `agenthd::workflows::apply_checkout`
   (el mismo workflow que llama la pantalla Settings del TUI)
   vía un helper `compose_apply_checkout(&Paths, &str) ->
   Result<String, String>`. El comando Tauri reenvía esa
   `Result` literal: en `Ok` el frontend recibe el path
   absoluto (trimmed) que el helper acaba de escribir; en `Err`
   recibe como rejection el texto de `ApplyError::message()`
   intacto (mismas cadenas que el TUI). Después de un save
   exitoso el frontend re-invoca `settings_status` +
   `list_agents` para recomponer Settings + Agents; si la
   revalidación post-escritura falla (puede pasar incluso
   cuando el write tuvo éxito), el lib persiste pero devuelve
   el error para que la GUI lo muestre sin rollback (el TUI
   hace lo mismo).

**Lo que NO hace el prototipo** (decisiones explícitas):

- No expone `prompt` ni `permissions` en la proyección
  de agentes.
- No llama a `Paths::ensure_dirs`, no escribe en
  `canonical/`/`targets/`/`skills/`/`state.json` desde
  esta superficie. El único write es
  `apply_checkout` → `save_settings` → `write_target`
  sobre `settings.json`, y solo cuando el path validó.
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
cargo test --lib   # 16/16 OK (8 read-only + 8 apply_checkout)
```

Los tests usan `tempfile::TempDir` para construir `Paths`
aislados (mismo patrón que los tests del lib `agenthd`); no
mutan `HOME` ni `XDG_CONFIG_HOME`. Cada test invoca los
composition helpers (`compose_settings_status`,
`compose_agents_list`) que los comandos Tauri envuelven
tras `Paths::from_env()`.

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
  URLs externas, ni lanzar procesos. Solo invoca los dos
  comandos definidos aquí.

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
