# Local Arch / CachyOS paired package preparation

## Latest executed evidence (2026-10-04)

Current matrix and remaining gates: [VALIDATION.md](VALIDATION.md). Compiler
source-path remapping is **built and inspected successfully**; the new approved
artifacts below are immutable. Current focused
fixture run: **43 PASS** (29 installer + 14 packaging), log
`/tmp/opencode/arch-remap-fixture-tests.log`; Bash syntax and diff checks PASS.
All validation below is parent-verified; no builds/tests rerun for this docs update.

This note supersedes preparation-only claims below. Approved current snapshot:
`/tmp/opencode/agenthd-arch-remapped/agenthd-0.1.0.tar.gz`, SHA-256
`8188cc343e0b8da48210059521419ffe4d5fd1c87451fafce7ae2e77f66567b7`.
The older `arch-final` and `.prepared` artifacts are historical predecessors.
This archive includes the approved encoded-Rustflags recipe,
GPT pins and Plans code, from uncommitted working-tree sources, not a commit.
It predates these documentation updates: approved code matches, subsequent docs
are not archived; no regeneration or bit-identical latest-working-tree claim.

Real offline CachyOS `makepkg --nodeps --noconfirm` **SUCCESS**, producing
`agenthd-0.1.0-1-x86_64.pkg.tar.zst` in that directory, SHA-256
`ef721fde48ed4f84ec221c5864af24606d6da4287b79264000768dc35c0fa380`; log
`/tmp/opencode/arch-remap-package-build.log`. Source verification **Passed** from
the correct recipe cwd. `pacman -T cargo` reports a missing package despite
a local cargo executable and rustc 1.96 being available; other dependencies were satisfied. Dependency
checks and clean-chroot builds are **not validated**. Cargo/npm caches were
separate copies under `/tmp/opencode`, with no network. The historical arch-final
GUI `$srcdir` warning is **absent in the new build**; three root Tools dead-code
warnings remain baseline. Both binaries have zero new build-source references;
the GUI has four expected `/usr/src/debug/agenthd/agenthd-0.1.0/src/` references.
No binary patching or warning filters. No compiled-binary reproducibility or
native all-platform claim. Extracted pair `/tmp/opencode/arch-remap-extracted/usr/bin`
has mode 755, license mode 644, license cmp PASS and ldd with no missing libraries.

Actual orchestrated installer **SUCCESS** (two cargo installs, GUI
custom-protocol) into `/tmp/opencode/agenthd-paired-install/bin`; log
`/tmp/opencode/paired-install-final.log`. No host installation/AUR publication.
Latest automated counts: root **576 passed, 1 ignored**, Tauri **95** each
default/custom-protocol, Node **84**, Angular build PASS, installer **29** +
packaging **10** = **39** at the prior baseline; the current focused run above
supersedes only the installer/package counts.
Isolated fixture `/tmp/opencode/agenthd-gui-smoke` is prepared with fresh HOME/XDG,
approved snapshot agents+skills and a local models-only 10s stub; packaged-pair
commands/manual procedure are in its README. Parent reported historical arch-final GUI process launch
and termination via SSH shell notification; redirected log was 0 bytes.
No screenshots/DOM/live IPC inspection or manual UI acceptance; current
visual gates pending. Stub Discovery is not real registry validation; remote
Tools installation remains pending. The new remapped GUI was **not launched**.
Its packaged CLI negative test `agenthd gui --repo missingcheckout` exited 1
with `agenthd: resolve configured checkout` and no HOME/XDG children written;
evidence: `/tmp/opencode/arch-cli-negative-Nyfcgx` stdout/stderr files.
Prior read-only Linux gate stays closed.

D4 priority is one `agenthd` package containing **both** `/usr/bin/agenthd`
and `/usr/bin/agenthd-gui`, adjacent as required by the CLI launcher. MIT
license: `/usr/share/licenses/agenthd/LICENSE`. Angular assets are embedded
in the companion with `custom-protocol`, not installed separately. There is
no remote source URL, release publishing, AUR submission, desktop entry,
tray/appindicator feature, hook, Tauri bundler, or `cargo install` step.

## Review first, then create a local snapshot

From the repository, with Git, Node, GNU tar, GNU mv and gzip available:

```sh
node scripts/package-arch.mjs \
  --source-root /home/lukateric/dev/agent-handler \
  --output-dir /tmp/opencode/agenthd-arch-prepared --list

# Only after the parent has approved this list and current-tree review is done:
node scripts/package-arch.mjs \
  --source-root /home/lukateric/dev/agent-handler \
  --output-dir /tmp/opencode/agenthd-arch-prepared
```

Both paths must be explicit and absolute. Output must be strictly under
`/tmp/opencode`, outside the source tree, without symlink redirection.
`--list` validates and prints the sorted file list without creating output.
Generation requires an absent output directory; existing directories and their
contents are never overwritten. Listing may use an existing output directory.
Review again if files change between listing and generation; keep the tree
quiescent while generating. The parent-approved remapped snapshot above already
exists; this docs-only update does not regenerate it. Any later snapshot needs
a new absent output directory, not either existing artifact directory or the
historical `.prepared` path in the example above.

The generator enumerates with `git ls-files --cached --others
--exclude-standard`, then copies **working-tree bytes** (including modified
tracked and approved new uncommitted files). It does not use `git archive`
or read content from HEAD. Source scope includes root manifests/locks,
LICENSE and Markdown docs, `src`, `tests`, `agents`, `skills`, `scripts`,
`packaging`, `docs`, and the required Angular/Tauri sources, configs,
icons, capabilities and locks. Unsupported candidates fail for review rather
than silently omitting a new production input. Ignored untracked files are
not candidates. Explicit exclusions remove Git/build/dependency caches,
dist, logs, `.env*`, credential/secret paths, private-key files and temporary
files, even if tracked. This is not a content-based secret scanner: review
the file list and source contents. Symlink/special-file candidates fail
closed, including symlink ancestors; required inputs must be present.

Outputs (published together into a new output directory):

- `agenthd-0.1.0.tar.gz` — top-level `agenthd-0.1.0/`.
- `PKGBUILD` — local source filename and actual archive SHA-256, never `SKIP`.
- `agenthd-0.1.0.tar.gz.sha256` — checksum sidecar.

Only `PKGBUILD.in` is archived, not the generated hashed recipe (no circular
checksum). Staging is temporary in a sibling directory under the output parent.
All three artifacts are prepared before the source staging tree is removed and
the directory is published with one no-clobber rename. Preparation errors remove
staging and leave no output directory, so generation can be retried. GNU tar sorts
entries, fixes mtime to epoch and uid/gid to zero, normalizes directories to
755 and files to 644; `gzip -n` omits timestamp/name. Identical source bytes
give identical **source snapshots**, not a claim of reproducible binaries.
Dependencies are locked, not vendored; a clean offline build may lack
Cargo/npm caches and require network access.

## Build contract and remaining validation

The template is version **0.1.0**, pkgrel **1**, **x86_64**. A provisioned
Arch/CachyOS build environment needs `base-devel` as a prerequisite (not a
runtime dependency), plus `cargo`, `nodejs`, `npm`, `pkgconf`. Runtime
dependencies are `gtk3`, `webkit2gtk-4.1`, `dbus`, `glibc`, `gcc-libs`.
No tray/appindicator dependencies are needed by the current Tauri features.

Current Angular requires Node **^22.22.3 || ^24.15.0 || >=26.0.0**;
check the selected Node on PATH rather than assuming any Node 22 works.
This host is confirmed CachyOS (`/etc/os-release`), with GNU tar, gzip,
makepkg available, selected Node 22.22.3 and rustc 1.96.0. Rust below 1.87
has a known compile issue in the pinned dependency graph. The GUI lock pins
yanked `yoke-derive` 0.8.3: Cargo honors pinned versions with `--locked`;
the yank itself is **not** a reason to rewrite the lock or claim locked
builds cannot work. Missing caches/network can still block clean builds.

`build()` runs `npm ci --include=dev`, `npm run build`, verifies
`dist/agenthd-tauri-angular-spike/browser/index.html`, then builds the CLI
and GUI with `cargo build --release --locked`; the GUI explicitly enables
`custom-protocol`. Both use an explicit target directory under `$srcdir`.
Both receive `--remap-path-prefix=$srcdir=/usr/src/debug/$pkgname` through
`CARGO_ENCODED_RUSTFLAGS`, retaining caller flags with Cargo's encoded-over-plain
precedence and preserving spaces in the remap argument. See VALIDATION.md for
the historical warning and successful rebuilt-package inspection.
`package()` uses only `install -Dm755` for binaries and `install -Dm644`
for LICENSE under `$pkgdir`. No dependency resolution upgrade is intended.

Historical preparation checks included Node syntax, fixture recipe Bash syntax
and manifest JSON parsing. The historical arch-final GUI process was launched
as reported above; that is not manual UI acceptance. The new remap recipe has
parent review, fixtures, a new approved source/hash, a successful isolated build
and binary inspection. Preserve all artifacts. No further code changes needed
for the agreed automated scope; next steps are manual acceptance and external
environment validation. Keep HOME/XDG and Cargo/npm/build caches isolated
there. Do not run `makepkg -s`, `makepkg -i`, `sudo`, `pacman`, host installation
or AUR publication. Missing prerequisites must be reported, not installed on
the host. Package build validation does not close visual acceptance: Settings,
editor, D3 and CRUD visual checks remain pending, as does D5 cross-platform.
