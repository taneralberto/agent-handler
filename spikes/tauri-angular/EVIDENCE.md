# Spike D1 / Tauri v2 + Angular — Evidencia observada

Recopilado durante la etapa 2 del spike D1, en este host
(Linux genérico, no Arch específica). **Esta es la verdad
ejecutada, no una proyección de lo que debería pasar.** Las
verificaciones que terminan con `OK` se corrieron de verdad;
las que terminan con `N/A` no se aplicaron a este host.

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
| Compilación binario | `cargo build` | **OK** (2.64s; binario 193 MB debug en `target/debug/agenthd-tauri-angular-spike`) |
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
  "windows")]`.

## Pros y contras observables (D1 sigue abierto)

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
  **NO es gate de D1** — D1 se cierra con la comparación
  de evidencia + limitaciones honestas registradas + la
  aceptación del usuario; Arch/Windows reales entran al
  gate de Fase 6, no al gate D1).

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
runtime gráfico funcione — ver "Limitaciones honestas"):

```bash
cd spikes/tauri-angular
npm run tauri dev
```

Estado de los lockfiles en git: ambos
(`spikes/tauri-angular/src-tauri/Cargo.lock` y
`spikes/tauri-angular/package-lock.json`) están
generados y versionados con el spike para garantizar
reproducibilidad.
