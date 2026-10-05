# Current validation matrix — 2026-10-04

This is automated/code and artifact evidence, **not signed human acceptance**.
Settings, editor/CRUD, cooperative jobs and Plans/inventory are implemented;
remaining review is partial validation, not pending root implementation.
D5 schemas and routes are unchanged; cross-platform validation remains open.

## Sources and artifacts

The parent-approved working-tree source archive (not a commit) is
`/tmp/opencode/agenthd-arch-remapped/agenthd-0.1.0.tar.gz`, SHA-256
`8188cc343e0b8da48210059521419ffe4d5fd1c87451fafce7ae2e77f66567b7`.
Its package is `agenthd-0.1.0-1-x86_64.pkg.tar.zst` in the same directory,
SHA-256 `ef721fde48ed4f84ec221c5864af24606d6da4287b79264000768dc35c0fa380`.
Approved code and the latest encoded-Rustflags recipe match; this immutable
archive predates this documentation update, not bit-identical to the latest tree.
The old `agenthd-arch-final` archive (SHA-256
`d71d31ccfefb61ec335289294a4160746d0b417dd807b8c4a7d878639cafe71f`)
and `.prepared` predecessor remain historical and unchanged.

| Scope | Evidence | Status / limit |
| --- | --- | --- |
| Root code, last reported run | 576 passed, 1 ignored | PASS; not rerun for this recipe/docs change |
| Tauri lib, default + custom-protocol | 95 passed each, `gui-plans-cargo-{default,custom}.log` | PASS; last verified, not rerun here |
| Frontend unit/build | Node 84 passed; Angular build PASS, `gui-plans-{unit,angular-build}.log` | PASS; last verified, not rerun here |
| Installer/package fixtures, current recipe | 29 installer + 14 package = 43 passed; `arch-remap-fixture-tests.log` | PASS, parent-verified; supersedes 29 + 10 baseline |
| Bash recipe syntax | `bash -n packaging/arch/PKGBUILD.in`; generated fixture syntax also tested | PASS, parent-verified |
| CachyOS remapped archive/package | Offline isolated `makepkg --nodeps --noconfirm`; `arch-remap-package-build.log` | SUCCESS, parent-verified; release CLI + GUI custom-protocol; no `$srcdir` warning |
| Extracted remapped package | `/tmp/opencode/arch-remap-extracted/usr/bin`: pair 755; license 644 + cmp PASS; ldd no missing libraries | PASS, parent-verified |
| Remapped packaged CLI negative launch | Empty HOME/XDG under `/tmp/opencode/arch-cli-negative-Nyfcgx`; `agenthd gui --repo missingcheckout` | PASS: exit 1, stderr `agenthd: resolve configured checkout`, no HOME/XDG children written; stdout/stderr files there |
| Isolated paired installer | `/tmp/opencode/agenthd-paired-install`; two cargo installs, GUI custom-protocol; `paired-install-final.log` | SUCCESS; no host installation |
| Historical arch-final GUI process | Fixture `/tmp/opencode/agenthd-gui-smoke`; launch + termination reported by parent via SSH shell notification | Process launch only; log 0 bytes, no screens/DOM/IPC inspection or manual UI acceptance; remapped GUI NOT launched |
| Latest remap recipe/new package | Parent review, new source/hash, isolated build and binary strings inspection | DONE; no patched binaries or warning filters |
| Full dependency checks / clean chroot | `--nodeps` used; local cargo executable exists but pacman package `cargo` is missing | PENDING external provisioned environment; no prerequisite installation here |
| Other Linux / Windows, current tree | No current-tree native run | UNVALIDATED; historical gates are not reopened or promoted to current evidence |

Logs above are under `/tmp/opencode/`. No binary reproducibility or all-platform
native claim. Three root Tools dead-code warnings remain baseline.

## Source-path warning

The historical arch-final package emitted the real makepkg warning that `usr/bin/agenthd-gui`
contains `$srcdir`. Parent `strings` inspection identified four Rust diagnostic
code-location references under
`/tmp/opencode/agenthd-arch-final/src/agenthd-0.1.0/src/`:
`models.rs`, `store/sync.rs`, `agent.rs`, `tools/mod.rs`.
The recipe now appends `--remap-path-prefix=$srcdir=/usr/src/debug/$pkgname`
as one encoded compiler argument for **both** CLI and GUI. Existing encoded
flags retain Cargo precedence (including an explicitly empty value); otherwise
plain RUSTFLAGS are whitespace-split into encoded arguments. RUSTFLAGS remains
unchanged. Unit-separator encoding preserves paths/encoded arguments with spaces.
No Rust source strings, compiled binaries or warning filters were patched.
The new remapped build has **no `Package contains reference to $srcdir` warning**.
Parent `strings` inspection found zero
`/tmp/opencode/agenthd-arch-remapped/src` references in **both** binaries;
the GUI has four expected references under
`/usr/src/debug/agenthd/agenthd-0.1.0/src/` to the same files above.
Real compiler/package inspection closes warning removal; fixture fake cargo/npm
captures exported flags, not live IPC behavior.
Source SHA verification, deterministic generation, no-SKIP and atomic no-clobber
publication remain unchanged and fixture-tested.

## Remaining gates

- **PENDING manual UI:** Settings Save + refresh; new/rename/delete; stale draft;
  Plans/Skills; Cancel/Close. “Continue” is authorization to work, not “works”.
- **PENDING external integration:** real OpenCode registry Discovery and remote
  Tools installation. The fixture is only a local models stub with a 10s delay,
  not registry/remote-install evidence. No downloads/fetches/installs here.
- **PENDING external platform/environment:** full dependency checks, clean
  chroot, other Linux and Windows native checks. Network, privileges or another
  machine cannot be substituted with fixtures.
- **CLOSED historical read-only Linux GUI↔TUI gate:** prior user confirmation
  remains closed; it does not accept the new controls or certify current binaries.

Headless Tauri mocks do not certify production `AppHandle<Wry>` live IPC; no new
Tauri test feature is required or added. MIT copying is authorized for the pair.
This docs-only update performs no artifact regeneration, host changes, GUI
launch, network access, installation or commit. Parent build checks left root
Settings `95b6…`, state `e4e8…` (user-authorized model updates), and installed
host binaries `9f51…` / `e59d…` unchanged; no claim that original state `978…`
is unchanged. No further code changes are needed for the agreed automated
scope; next steps are manual acceptance and externally provisioned validation.
