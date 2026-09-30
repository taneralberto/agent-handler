# Spike D1: Tauri v2 + Angular sobre `agenthd`

Prototipo aislado para evaluar Tauri v2 + Angular como candidato
a la forma GUI del proyecto `agenthd` (decisión D1 de
`GUI_ROADMAP.md`).

**No es** una GUI para el binario de producción; es un spike
opt-in que vive en `spikes/tauri-angular/` y depende del lib
`agenthd` por path. El binario `agenthd` sigue rechazando
`agenthd gui` con exit code 2 — este prototipo no toca esa
ruta, no se acopla a la CLI, y no marca D1 como cerrado.

## Qué demuestra

Dos comandos Tauri read-only, ambos compuestos sobre el lib
`agenthd` (single source of truth):

1. **`settings_status`** → clasifica `settings.json` en
   `Empty` / `Ready` / `Stale` / `Error` usando
   `agenthd::workflows::read_checkout`. Misma función que
   la pantalla Settings del TUI; misma salida textual.
2. **`list_agents`** → re-lee el checkout configurado vía
   `read_checkout` + `Paths::with_settings` +
   `workflows::list_canonical_agents`, y proyecta
   `name` / `mode` / `model` / `description`.

**Lo que NO hace el prototipo** (decisiones explícitas):

- No expone `prompt` ni `permissions` en la proyección
  de agentes.
- No acepta ningún argumento del frontend (los comandos
  no tienen parámetros).
- No llama a `Paths::ensure_dirs`, no `save_*`, no
  `delete_*`, no escribe nada en el host desde esta
  superficie.
- No incluye plugins (`fs`, `shell`, `dialog`, `opener`,
  `http`, ...). Solo `core:default` en capabilities.
- No está acoplado al binario `agenthd`. No lee argv. No
  reemplaza a la TUI. La configuración del checkout sigue
  siendo exclusivamente TUI-only por ahora.
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

Comprobado durante la inspección previa al spike:

- `node` 22.22.3
- `npm` 12.0.2
- `cargo` 1.96.0 (edition 2021)
- `pkg-config` 3.0.7
- `webkit2gtk-4.1` presente vía pkg-config (las GTK nativas
  que Tauri necesita en Linux están disponibles sin instalar
  paquetes de sistema)

Si tu host no tiene las nativas GTK/WebKitGTK, el comando
`cargo build` de `src-tauri/` fallará con un error de
`pkg-config` o de cabecera. **No instales paquetes de
sistema sin permiso del usuario**; el spike debe reportar
el bloqueo exacto.

## Cómo ejecutar el spike

Desde el directorio `spikes/tauri-angular/`:

```bash
# 0) Ya hecho: `npm install` (218 paquetes, 45s) generó
#    `package-lock.json` (195 KB, generado y versionado
#    con el spike).
#    `npm ci` (218 paquetes, 2s) confirma reproducibilidad.
#    `npm run build` generó `dist/agenthd-tauri-angular-spike/`.

# 1) Re-verifica el frontend (idempotente, usa el lockfile):
npm ci
npm run build

# 2) Modo desarrollo (levanta Angular dev server en
#    127.0.0.1:14720 y compila/ejecuta el binario Tauri).
#    Requiere un display usable con WebKitGTK inicializado.
#    Nota: en este host, `DISPLAY=:0` +
#    `timeout 12s ./target/debug/agenthd-tauri-angular-spike`
#    produjo exit 124 con stderr vacío, lo cual NO
#    demuestra que el runtime gráfico funcione. Ver
#    `spikes/tauri-angular/EVIDENCE.md` → "Limitaciones
#    honestas".
npm run tauri dev

# 3) Build de producción (Angular a dist/, Tauri a bundle).
npm run tauri build
```

Para los tests focales del backend Rust (no necesitan
nativas GTK porque son tests puros — `cargo test --lib` no
inicializa WebKit):

```bash
cd src-tauri
cargo test --lib   # 8/8 OK
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

**D1 sigue sin cerrarse.** Este spike es una pieza de
evidencia técnica: ejercita el seam del lib y deja los
lockfiles generados y versionados con el spike
para que el siguiente agente pueda reproducir el
build sin surprises. **No** valida el runtime gráfico
(ver `EVIDENCE.md` → "Limitaciones honestas": en este
host `DISPLAY=:0` + `timeout 12s` produjo exit 124 con
stderr vacío, lo cual NO demuestra visualización ni IPC/
ventana funcional — esta limitación se **registra**, no
convierte la ejecución gráfica en gate de D1) y **no**
se ejecutó en Arch ni en Windows (queda para Fase 6 de
empaquetado; **NO es gate de D1**). La decisión final
Tauri vs GTK4-rs vs egui vs iced vs web local se cierra
tras (a) comparar pros/contras factuales contra los
otros candidatos tomando como entrada lo que el spike
produce de verdad (scaffold reproducible, 8/8 tests
focales del backend, lockfiles generados y versionados
con el spike, lockfile doble, dependencia
cruzada por path, pros/contras observables), más las
limitaciones honestas registradas (runtime gráfico no
demostrado en este host, tests del backend no invocan
el dispatch Tauri), y (b) la aceptación explícita del
usuario. El próximo paso es esa comparación de
pros/contras, no invertir en el slice vertical de Fase
5 (Fase 5 viene después de cerrar D1).
