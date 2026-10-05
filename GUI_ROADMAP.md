# Hoja de ruta GUI — estado actual y handoff

Actualizado: 2026-10-04. Este documento sustituye el historial duplicado del
plan: `[x]` significa implementado o validado en el alcance indicado; `[ ]`
significa pendiente. Implementación, pruebas automatizadas y aceptación
visual nativa son evidencias distintas; ninguna sustituye a las otras.

## Objetivo y fuente de verdad

GUI Tauri + Angular como segundo cliente de los workflows de `agenthd`, sin
reescribir la base ni duplicar reglas de negocio; la TUI sigue operativa.
Las fases 1–5 están implementadas. Quedan aceptación nativa de los controles
nuevos, un paquete actualizado y validación externa de la fase 6.

- Código publicado: `6e09de054b90115a65d2d10a60cc1872f5db180e`
  (`feat: complete GUI workflows and modernize agent editing`), incluye
  `90110c5`. HEAD y la referencia local `origin/main` coinciden al revisar.
  El push ya fue realizado; no hay un commit de implementación pendiente.
- Evidencia histórica detallada: `spikes/tauri-angular/EVIDENCE.md`.
  Matriz de empaquetado: `packaging/arch/VALIDATION.md`.
  Esas referencias pueden conservar UI/conteos anteriores: este roadmap
  manda para el estado actual, no certifica nuevos resultados de pruebas.
- `PLAN.md` y `TOOL_INSTALLER_PLAN.md` se retiraron deliberadamente;
  no restaurarlos ni usarlos como referencias canónicas.

## Decisiones e invariantes que no se reabren

- **D1 — cerrada:** Tauri + Angular, por elección explícita del usuario.
- **D2 — cerrada:** `agenthd` y `agenthd tui` abren la TUI;
  `agenthd gui` busca `agenthd-gui(.exe)` adyacente y reenvía argumentos
  y código de salida. Si falta, falla con exit 2 antes de escribir.
  `--repo` aplica a ambos modos; precedencia: override > `settings.json`
  > default. El override válido persiste con save-if-changed; primer
  `--repo` gana, sin reescribir si coincide con el valor guardado.
- **D3 — aprobada e implementada:** jobs asíncronos con eventos y snapshots
  retenidos; cancelación cooperativa solo en checkpoints seguros.
  La unidad en curso termina antes del siguiente checkpoint. Sin rollback
  automático: conservar y reportar cambios/resultados parciales.
- **D4 — aprobada, validación parcial:** Arch/CachyOS primero, PKGBUILD paired
  CLI + GUI. Fuentes desde snapshot del working tree actual, no `git archive`.
  No publicación automática en AUR ni formatos de otras plataformas asumidos.
  Builds/instalaciones de validación aislados bajo `/tmp/opencode`.
- **D5 — contrato conservado:** schema y rutas actuales, sin migración
  anunciada; no afirmar compatibilidad multiplataforma antes de validarla.
  Todo acceso canónico usa el checkout Ready persistido y paths scoped.
  El contexto abierto no sustituye esa autoridad; prior hash y checkout
  se revalidan al guardar para rechazar cambios externos o cambio de repo.
- Validar checkout antes de leer/escribir; fail-closed para origen canónico,
  paths/symlinks/archivos inválidos, estado malformado y concurrencia.
  CRUD solo canónico; rename **no atómico** (rename-then-save), sin promesa
  de rollback. Sync/Skills no escriben al checkout ni fuerzan overwrite;
  conservar guards de ownership y confirmaciones explícitas de conflictos.
- Pins de modelos y dependencias sin cambios por esta revisión.
  Licencia CLI + GUI: MIT, `Copyright (c) 2026 taneralberto`.

## Checklist de entrega

### Fase 1 — Caracterizar comportamiento: completada

- [x] Inventario y matriz de APIs/tests en `REUSABLE_API_COVERAGE.md`.
- [x] Contratos observables, invariantes y cobertura de regresión identificados;
  matriz revisada como base para la extracción compartida.

### Fase 2 — Workflows compartidos: completada

- [x] `read_checkout`, `apply_checkout`, `list_canonical_agents` reutilizados.
- [x] Orquestación Skills y agent safe-install por target extraída y probada.
- [x] Save del editor compartido, con prior hash y rename-then-save existentes.
- [x] Tools screen y dispatch/estado/render TUI evaluados como UI-only;
  no se extraen artificialmente `TerminalGuard`, panic hook ni editor TUI.

### Fase 3 — Launcher: completada

- [x] Parser/resolución de checkout separados de TUI y panic.
- [x] D2, persistencia/no-rewrite y fallos sin efectos cubiertos por tests
  en `src/launcher.rs` y `tests/cli_launch.rs`.

### Fase 4 — Forma GUI: completada

- [x] Spike Tauri + Angular y elección D1 cerrada.
- [x] Companion y receta de producción con `custom-protocol` para embeber
  el frontend; el render suelto ya tuvo confirmación manual histórica.

### Fase 5 — Cliente GUI y operaciones: implementación completada

- [x] DTOs y comandos read-only Settings/Agents, refresh y errores del lib.
- [x] Gate read-only paired GUI↔TUI en Linux sobre el mismo HOME **cerrado**
  por el usuario: «Sí, todo coincide». No se reabre ni certifica controles nuevos.
- [x] Settings editable: validar/guardar checkout y refrescar Settings/Agents.
- [x] Editor de agentes con descripción, modo, modelo, prompt y permisos;
  permisos desconocidos preservados, draft separado y protección ante stale.
- [x] CRUD create/rename/delete canónico con reserva corta exclusiva;
  confirmaciones ligadas al snapshot DTO, sin shadow copy ni rename atómico.
- [x] D3 para Sync OpenCode/Pi, Skills, Tools y Discovery; progreso, cancelación,
  Close, reportes parciales y recuperación mediante current/status.
- [x] Listener-first `agenthd-operation`, secuencia monotónica y guardas de
  generación/conexión; recovery IPC fallido mantiene mutaciones fail-closed.
- [x] Discovery acotado y limpieza de Tools; catálogo y permission keys
  autoritativos desde backend, no reglas duplicadas en frontend.
- [x] Plans/inventory read-only: paths, hashes, etiquetas y razones del root;
  snapshot advisory, no ejecutable/atómico; cada `op_start` replantea.
  Refresh al bootstrap/manual/terminal; errores conservan filas como STALE.
- [x] Sidebar Agents, modales New/Edit y Activity permanente inferior;
  fixes de presentación y serialización descritos abajo.

### Fase 6 — Empaquetado: validación parcial, no entrega actual completa

- [x] Instalador fuente paired `scripts/install.mjs`, GUI custom-protocol,
  fixtures y validación real con instalación en raíz temporal aislada.
- [x] PKGBUILD + generador de snapshot working-tree/SHA-256 y fixtures;
  matriz automatizada publicada en `packaging/arch/VALIDATION.md`.
- [x] Build offline real Arch/CachyOS y artefacto paired extraído: permisos,
  licencia y dependencias dinámicas comprobados, sobre fuentes anteriores.
- [ ] Regenerar snapshot y reconstruir paquete del **commit actual**, con
  LICENSE, CLI, GUI custom-protocol y dist actualizado, bajo almacenamiento
  aprobado en `/tmp/opencode`; comprobar espacio antes de generar/build.
- [ ] Dependency checks completos y clean chroot. El build anterior usó
  `--nodeps`: toolchain local disponible, pero paquete pacman `cargo` ausente.
- [ ] Aceptación humana nativa de la UI y del paquete actualizado.
- [ ] Build/runtime nativos actuales en otros Linux y Windows; cerrar matriz
  por plataforma sin extrapolar desde CachyOS o un smoke histórico.

## UI actual y límites de su evidencia

Agents vive en la sidebar; New y Edit abren el mismo editor modal. El modelo
se puede buscar, introducir manualmente o heredar. Modelos discovered son
advisory, no registro de proveedores. **New crea un agente**: el selector
no crea providers/models; no hay tarea pendiente de registro de modelos.

Activity permanece visible abajo incluso idle e incluye estado de cancelación.
El modal duplica esa actividad cuando el fondo está inert. ESC, X y click
fuera conservan la confirmación antes de descartar un draft sucio.
Footer sticky, cuerpo desplazable; Angular 22 usa `afterRenderEffect` y
`viewChild` signal para corregir el primer click/foco al montar el modal.

La prueba visual más reciente usó **DOM Angular real en Chromium con IPC
Tauri mock**, no WebKit nativo ni IPC Rust real. Captura de referencia:
`/tmp/opencode/modal-dom/compact.png`, viewport 900×700, modal de altura
430 px frente a los 496 anteriores. En al menos cuatro viewports el footer
quedó contenido; siblings con blur 4 px, diálogo sin blur y X de 42 px con
contraste comprobado. Es evidencia de layout, **no aceptación nativa**.
El usuario no ha confirmado la última UI nativa después de la actualización.

El bug reportado de cancelación se corrigió en
`spikes/tauri-angular/src-tauri/src/jobs.rs`: `JobReport` serializa variantes
terminales como struct con nombre; `None` conserva JSON `null` y el reporte
nested `InstallTool` coincide con el contrato frontend. Regresión incluida.

## Validación ya realizada (no reejecutada para esta revisión documental)

| Alcance | Última evidencia | Límite |
| --- | --- | --- |
| Frontend unit | 135 passed | Última tarea de commit; no runtime nativo |
| Installer + packaging fixtures | 45 passed | Última tarea de commit; no paquete actual |
| Tauri lib default / custom-protocol | 105 passed cada variante | Antes de cambios finales solo de presentación |
| Root `--all-targets` | 576 passed, 1 ignored | Última verificación; ignored = smoke Tools con red |
| Angular producción | PASS | Warning CSS componente: 6.87 kB > 4 kB; bajo límite error 8 kB, budgets intactos |

El artefacto histórico es
`/tmp/opencode/agenthd-arch-remapped/agenthd-0.1.0-1-x86_64.pkg.tar.zst`.
Fue construido **antes** de la instalación global posterior del usuario y
de las últimas mejoras UI/serialización/modal. Su snapshot tampoco contiene
las últimas revisiones documentales. No representa `6e09de0` ni el manifest
actual completo; hashes/logs y procedimiento están en la matriz de packaging.
No se publicó en AUR ni se instaló como paquete del host.

El usuario sí ejecutó `npm run install:global` y actualizó sus binarios CLI/GUI.
No afirmar que nunca hubo cambios en binarios del host: nuestras validaciones
de build/install fueron aisladas. La precedencia del instalador es
`--root > CARGO_INSTALL_ROOT > CARGO_HOME > HOME/.cargo`; para validación
aislada usar `--root` explícito según el README, no ejecutar global por comodidad.

## Pendientes humanos y siguiente secuencia

1. **Aceptación nativa del usuario con binario actualizado.** Abrir `agenthd gui`
   con el checkout guardado; usar `agenthd gui --repo /abs/checkout` solo si
   se desea cambiar/persistir ese checkout. No resetear configuración real.
   - [ ] Settings Save/refresh y paridad con TUI; New/Edit/rename/delete,
     permisos, stale draft y confirmaciones ESC/X/fuera en ventana nativa.
   - [ ] Activity idle/progreso, Cancel/Close, resultados parciales y recovery
     fail-closed; validar también Plans/inventory y Skills sin false-green.
   - [ ] Discovery con OpenCode real y Tools remoto, con autorización de red;
     fixtures/stubs no certifican esos servicios.
2. **Paquete del código actual en entorno aislado.** Inventariar con
   `node scripts/package-arch.mjs --source-root /abs/checkout
   --output-dir /tmp/opencode/<directorio-nuevo> --list`; después de aprobar
   inventario y revisar espacio, generar sin `--list`, build y validar el pair.
   No sobrescribir el artefacto histórico ni instalar paquetes del host.
3. **Entorno limpio y matriz nativa externa.** Validar dependencias completas,
   clean chroot y plataformas restantes; registrar solo pruebas realmente
   observadas. Sin AUR automático ni aceptación visual inferida de unit tests.

## Comandos de referencia para desarrollo (no ejecutados aquí)

Requieren caches/toolchains existentes; no autorizan instalación ni acceso
de red. Para aislamiento completo, preparar un `CARGO_HOME` temporal con
caches copiados y exportarlo antes; usar `RUSTUP_HOME` con toolchains preparados
fuera de HOME y de solo lectura. Estos comandos no sustituyen esa preparación
ni garantizan por sí solos ausencia de escrituras en HOME.

```bash
export TMPDIR=/tmp/opencode
export CARGO_TARGET_DIR=/tmp/opencode/cargo-target
export npm_config_cache=/tmp/opencode/npm-validation-cache
cargo test --all-targets --locked --offline
node --test scripts/install.test.mjs scripts/package-arch.test.mjs
cd spikes/tauri-angular
npm run test:unit
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --lib --locked --offline
cargo test --manifest-path src-tauri/Cargo.toml --lib --locked --offline --features custom-protocol
```

`--offline` requiere caches completos. No actualizar pins/locks para sortear
un entorno incompleto.
Seams: `src/workflows.rs`, `src/launcher.rs`, `src/store/`,
`spikes/tauri-angular/src-tauri/src/{lib,jobs}.rs`, `src/app/` del spike.
Recetas: `packaging/arch/README.md`, `scripts/install.mjs` y README raíz.
