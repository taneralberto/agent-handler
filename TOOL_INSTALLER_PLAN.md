# Tool Installer Plan

## Progress (2026-09-23)

Implementation is present in the repository; validation remains partial
and acceptance is not complete. The v1 contract and test/acceptance
criteria below remain authoritative; nothing here implies the feature
is complete.

**Implemented (code present, partial validation done):**

- **Verified immutable pin** recorded in the bundled catalog: tag
  `opencode-2026-09-23` at `https://github.com/taneralberto/pi-psql.git`
  with peeled commit SHA
  `0dba366061911f0ec389f4a78cc46fd6d6a19d41`. This unblocks the prior
  "Hard prerequisite" — the four OpenCode-adaptation files
  (`README.md`, `SKILL.md`, `package.json`, `package-lock.json`) are
  baked into the tagged commit, so the `C:\dev\pi-psql` working tree is
  no longer the source of truth.
- **Separate `src/tools.rs` module** holding `ToolCatalogEntry`,
  `NodeMin`, `DEFAULT_CATALOG` (the `pi-psql` entry above),
  `ToolStatus`, `ToolItem`, `ToolOutcome`, `install_tool`,
  `install_tool_with`, `install_tool_at`, `tool_status`,
  `destination_for`, `staging_for`, `spawn_command`, and the
  Linux/Windows FFI shims (`renameat2`/`MoveFileW`). The catalog moved
  out of `src/store.rs`.
- **`Screen::Tools` in `src/app.rs`** with `MainItem::Tools`,
  `open_tools`, `refresh_tools`, `install_selected_tool`,
  `handle_tools_key`, and render/key-dispatch arms. `i` always
  dispatches into the installer (the OS no-replace primitive is the
  single source of truth for "already there" state).
- **`Paths.skills_dir`** added in `src/store.rs`, populated from
  `opencode_root.join("skills")`, ensured by `ensure_dirs`, and
  overridden in unit tests where needed.
- **No-replace publish**:
  - **Linux**: raw `renameat2` syscall via `libc::SYS_renameat2` with
    `RENAME_NOREPLACE`. `EEXIST` → `Conflict`, `EXDEV` →
    `InstallFailed` (no copy fallback).
  - **Windows**: raw `MoveFileW` via `windows-sys` (raw Win32, not
    `MoveFileExW`, not `std::fs::rename`). `ERROR_ALREADY_EXISTS` →
    `Conflict`, `ERROR_NOT_SAME_DEVICE` → `InstallFailed`.
  - **Other platforms**: fail closed `InstallFailed`; v1 does not
    invent a no-replace primitive for them.
- **Command timeout fixes**: every external spawn goes through
  `run_with_timeout` with a `Duration` per `SpawnSpec` (30 s for
  `git --version` / `ls-remote` / preflight / staging,
  120 s for `npm ci`). Reap loop tries `try_wait` until the child
  exits or the timeout elapses; on timeout the child is killed and
  reaped (no zombie), and the failure surfaces as a typed error.
- **Staging ownership fix**: `stage` reserves the staging path via an
  exclusive `fs::create_dir` (any pre-existing dir/file/symlink
  triggers `AlreadyExists` and the run bails without touching the
  path). `cleanup_staging` is only called after the installer has
  taken ownership, so it cannot clobber an unrelated preexisting
  sibling at the computed staging path.
- **`README.md`** documents `Tools`, the install-only contract, the
  eight-step flow, the Linux/Windows publish behavior, the Windows
  path caveat, the credential non-interference rule, and clarifies
  that remote catalog discovery remains out of scope.
- **Unit tests in `src/tools.rs`** (verified subset; see "Test gaps"
  below): preflight missing `git` and `node` below `node_min`
  (`preflight_missing_git_returns_prerequisites_missing`,
  `preflight_node_below_min_returns_prerequisites_missing`,
  `preflight_node_at_or_above_min_proceeds`); `ls-remote` parses
  peeled SHA from both annotated and lightweight tags; staging
  argv (`init` / `remote add` / `fetch --depth=1 origin <pin_tag>` /
  `checkout FETCH_HEAD`); `rev-parse HEAD == expected_sha` positive
  and mismatch; SKILL.md identity match / mismatch; `npm ci
  --omit=dev --ignore-scripts` argv and non-zero exit; **Linux**
  publish absent + same FS / target exists / cross-device
  (`linux_rename_noreplace_*`); **Windows** publish (cfg-gated):
  target-absent + same drive succeeds (`windows_rename_movefilew_succeeds_when_target_absent`)
  and target-exists returns `ERROR_ALREADY_EXISTS`
  (`windows_rename_movefilew_returns_already_exists_when_target_exists`);
  other-platform fail-closed paths; "no separate exists check before
  the primitive" assertion; "no shell interpolation" assertion; npm
  timeout constant.

  **Test gaps (not yet covered, must be closed before full
  acceptance):** Windows publish on a different drive (no
  `windows_rename_movefilew_*` test injects a staging on a
  separate volume; the `ERROR_NOT_SAME_DEVICE` → `InstallFailed`
  branch is only reachable through the unit's general
  error-code dispatch, not a dedicated test); preflight missing
  `node` and missing `npm` (only `git` missing and node-under-min
  are tested); preflight missing `node` with `node_min` already
  satisfied (no test for the case where `node` exits non-zero
  for reasons other than version comparison).
- **Smoke test** at `tests/tools_install_smoke.rs`
  (`#[ignore]`, runs `git`/`node`/`npm` against the real
  `pi-psql` remote under a temp `XDG_CONFIG_HOME`/`HOME`, asserts
  install success + second-run Conflict).

**Pending (known gaps, must be resolved before this feature ships):**

- **Windows `MoveFileW` existing-target error-code mapping needs
  runtime validation.** The Windows publish step currently maps
  `ERROR_ALREADY_EXISTS` → `Conflict` and every other
  `GetLastError()` value → `InstallFailed`. Whether
  `MoveFileW` against an already-present target directory can
  surface any other error code on a real Windows host is **not
  yet verified at runtime** — `cargo check` / `cargo clippy`
  pass under the Windows target but the code has not been run on
  Windows. If a real run returns a non-`ERROR_ALREADY_EXISTS` code
  for an existing-directory target, that must be reported here
  rather than worked around by adding a destination-presence
  precheck (which would violate the no-precheck plan: the OS
  primitive is the single source of truth for "already there"
  state). **Linux is verified**: the `renameat2` `EEXIST` →
  `Conflict` mapping is exercised by the `conflict_when_target_already_exists`
  unit test and by the `real_install_pi_psql_against_remote`
  smoke test's second-run assertion.
- **Full `cargo test` is blocked by old interactive Vim tests in a
  headless shell.** When validation runs in this environment those
  tests hang and must be skipped (e.g. `cargo test -- --skip
  <vim_tests>`); the tools-related unit + smoke tests should still
  run.
- **Manual TUI verification on Linux and Windows is not yet done.**
  Acceptance still requires a real-keyboard run on both OSes with a
  temp `XDG_CONFIG_HOME` / `%USERPROFILE%\.config`.

## Goal

Add a **Tools** menu to `agenthd` that installs third-party OpenCode skills from a small, bundled **static** catalog. First entry: `pi-psql`. The catalog and the flow are the single source of truth — adding another tool means one catalog entry plus a tool-specific validation step **only if** the default `SKILL.md` `name:` identity check is insufficient. No per-tool screen, manager, factory, planner, lock, or update machinery.

**v1 is install-only.** It never updates, replaces, force-installs, adopts unowned targets, or removes anything that already exists at the destination. Any existing target — regular directory, regular file, or symlink — is a **conflict that refuses to install**, a no-op.

This is an **approved proposal** for a new capability — a bundled static catalog whose entries are baked into `src/tools.rs::DEFAULT_CATALOG`. It does **not** introduce remote catalog discovery (which remains out of scope per the existing `README.md` "Out of scope" line) and does **not** introduce project-local targets (also out of scope). Partial implementation is on disk as of 2026-09-23 — see the progress section above; the v1 contract and acceptance criteria below remain the source of truth and the feature is **not yet complete**.

## Hard prerequisite

A **verified tag plus its expected immutable commit SHA** that bundles the four OpenCode-adaptation files (`README.md`, `SKILL.md`, `package.json`, `package-lock.json`) must be chosen and recorded as a `const` in `src/tools.rs::DEFAULT_CATALOG` **before any source edits ship**. The plan does not invent a pin. The plan does not ship a placeholder (raw SHA lookup, branch, `main`, env var, runtime fetch) that would make the feature permanently non-functional. As of 2026-09-23 the verified pin is recorded: tag `opencode-2026-09-23` (peeled commit `0dba366061911f0ec389f4a78cc46fd6d6a19d41`) at `https://github.com/taneralberto/pi-psql.git`, so this prerequisite is satisfied and the prior `C:\dev\pi-psql` working tree is no longer the source of truth.

## Design

- New screen `Screen::Tools { entries, selected, status, installing }` in `src/app.rs` (the `installing: bool` field blocks key dispatch while an install is running, so the user cannot queue a second run or race the spawn loop). **Separate from the agent `Install/Update` screen**, which has different semantics. Tools has only Install and Refresh.
- Main menu is the `MainItem` enum in `src/app.rs` (`Agents`, `InstallUpdate`, `Plugin`, `Exit`). Adding `Tools` is one variant, one label, one `match` arm in `render_main` and `handle_main_key`. `src/main.rs` does not edit menu text.
- One `ToolCatalogEntry { repo, skill_name, destination_subpath, pin_tag, expected_sha, node_min }`, one `DEFAULT_CATALOG` array, one shared `install_tool(paths, entry)`.

## Install path (v1)

1. **Pre-flight** — `git --version`; `node --version` parsed semver against `entry.node_min` (full tuple compare); `npm --version`. Missing or `node < node_min` → `PrerequisitesMissing`. Menu unaffected.
2. **Resolve tag → SHA** — two `git ls-remote` calls, not one.
   First `git ls-remote <repo> <pin_tag>` verifies the tag is found; for
   a **lightweight** tag that line's SHA is already the commit SHA.
   Then `git ls-remote <repo> <pin_tag>^{}` asks explicitly for the
   peeled commit ref; for an **annotated** tag that returns the
   commit SHA (and is empty for a lightweight tag). The peeled
   ref's SHA wins if both calls return one; otherwise fall back to
   the tag-ref SHA. Neither call alone covers both tag kinds, so the
   implementation issues both. The authoritative pin check is
   step 4 (`HEAD == expected_sha`); ls-remote is only a defense-in-
   depth sanity check that the tag still resolves to the recorded
   SHA. Refusal → `InstallFailed`. `git ls-remote <repo> <sha>` is
   **not** used (most servers do not expose arbitrary commit SHAs
   through refs).
3. **Stage on the same filesystem / volume as the destination**, adjacent to the target: `git init <staging>`; `git -C <staging> remote add origin <repo>`; `git -C <staging> fetch --depth=1 origin <pin_tag>`; `git -C <staging> checkout FETCH_HEAD`. Fetch by **tag**, not SHA: many servers refuse `fetch origin <unreachable-sha>`.
4. **Verify pinned SHA** — `git -C <staging> rev-parse HEAD` must equal `entry.expected_sha`. Mismatch (tag moved, repo compromised, or wrong constant) → remove staging, `InstallFailed`. This is the immutable-pin guarantee; if upstream re-tags, install fails closed.
5. **Identity check** — `<staging>/SKILL.md` frontmatter `name: == entry.skill_name`. Mismatch / malformed → remove staging, `IdentityMismatch`.
6. **Install deps** — `npm ci --omit=dev --ignore-scripts` inside staging. Both flags mandatory: third-party post-install scripts run with the user account and must not be trusted implicitly. Non-zero exit → remove staging, `InstallFailed`.
7. **Publish, no-clobber via OS no-replace primitive**:
   - **Linux**: `renameat2` with `RENAME_NOREPLACE` (via `libc::SYS_renameat2` or the `renameat2` crate); returns `EEXIST` if `<target>` exists.
   - **Windows**: `MoveFileW` from `<staging>` to `<target>` via the `windows-sys` or `winapi` crate (raw Win32 call, **not** `MoveFileExW`). Per Microsoft's docs, `MoveFileW` requires the destination not to exist (returns `FALSE` with `ERROR_ALREADY_EXISTS`) and source / destination must be on the same volume (returns `ERROR_NOT_SAME_DEVICE` otherwise). `std::fs::rename` is **not** used on Windows either — it lowers to `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`, which replaces unconditionally. Staging adjacent to `<target>` on the same drive satisfies the same-volume rule.
   - **Other platforms** (macOS, BSDs, etc.): fail closed `InstallFailed` with a message that safe no-clobber publish is not yet proven on this OS. No non-atomic copy fallback.

   The primitive's "exists" error (`EEXIST` / `ERROR_ALREADY_EXISTS`) → `Conflict`, no-op, staging removed. There is **no separate "exists" check before the primitive** — no TOCTOU window. "Same-device" error (`EXDEV` / `ERROR_NOT_SAME_DEVICE`) → `InstallFailed`, staging removed. Any other error → `InstallFailed`, staging removed.
8. **Cleanup** — on every failure path, remove the staging dir created this run. Never touch the destination, `.key`, `connections.enc`, or a sibling tool.

All external invocations use `std::process::Command::new(...).args(...)` with **explicit argv** — no shell, no string interpolation. Each spawn has a timeout (30 s for `git ls-remote` / fetch, 120 s for `npm ci`) and surfaces stderr tail (last ~20 lines) on failure.

## Node version

`pi-psql`'s transitive `yargs@^18` declares `engines.node: ^20.19.0 || ^22.12.0 || >=23`. Recording a `min_node_major` alone is insufficient — `22.x` below `22.12` is installed but yargs 18 will not load. The catalog stores `node_min: NodeMin { major: u32, minor: u32, patch: u32 }` (e.g. `(22, 12, 0)` for `pi-psql`, a conservative bound that covers the `^22.12` leg of yargs 18's engines); pre-flight parses `node --version` and compares the full tuple. Future entries can tighten or loosen the field without restructuring the flow.

## Statuses

`NotInstalled`, `Installed`, `PrerequisitesMissing`, `IdentityMismatch`, `InstallFailed`, `Conflict` (target already exists). No `UpToDate`, `UpdateAvailable`, `Unowned`, or force-install in v1.

## Out of scope (v1)

Updating, replacing, force-installing, adopting unowned targets, or removing existing skill directories. Pi-agent skill install. Project-local targets (`.opencode/skills/...`). **Remote catalog discovery** (the catalog is a static Rust `const`, not a fetched remote list — distinct from the existing `README.md` out-of-scope line, which still excludes remote catalog discovery). Signed refs, runtime pin selection, advisory locks, automatic retries. Publish on platforms other than Linux and Windows (fail closed for v1 — no proven no-replace primitive).

## Source file edit points

- `src/tools.rs` — `DEFAULT_CATALOG: &[ToolCatalogEntry]` (one entry, `pi-psql`, with verified `pin_tag` and `expected_sha`); `ToolCatalogEntry { repo, skill_name, destination_subpath, pin_tag, expected_sha, node_min }`; `install_tool(paths, entry) -> Result<ToolOutcome>` following the eight steps above (returns a typed `ToolOutcome { status, detail }` rather than `Result<(), _>`, so the UI can render the outcome without a separate `Result` unwrap); the publish step calls the OS no-replace primitive directly (`renameat2` with `RENAME_NOREPLACE` on Linux, raw `MoveFileW` on Windows) and fails closed on other platforms. (The catalog and `install_tool` were originally scoped to `src/store.rs`; the partial implementation as of 2026-09-23 lives in the new `src/tools.rs` module instead, with `src/store.rs` only contributing `Paths.skills_dir`.)
- `src/app.rs` — `Screen::Tools { entries, selected, status, installing }` (`installing: bool` blocks key dispatch while an install is running); render + `handle_key`; add `MainItem::Tools` to `MainItem::all()` and its label / dispatch arms.
- `src/main.rs` — declares `mod tools;` (the partial implementation already adds this); no menu edits; pass any new constructor argument the screen needs.
- `README.md` — document `Tools`, the install-only contract, the Linux / Windows publish behavior, the Windows path caveat (`%USERPROFILE%\.config\opencode\skills\<name>\`), the credential non-interference rule, and clarify that **remote catalog discovery** remains out of scope while the bundled static catalog is in.

## Tests

Use `tempfile` temp dirs (already a dev-dep). Inject a thin module-private runner closure at test time so tests assert argv exactly without a real `git` / `node` / `npm`. Mock the OS no-replace call at the FFI / syscall layer similarly (`renameat2` result code on Linux; `MoveFileW` return value + last error on Windows). Do not introduce a `CommandRunner` trait, lock file, or planner unless a concrete test demands one. **No real credentials; no connection-manager exercised.** Coverage tests the **actual proposed flow**:

- pre-flight missing `git` / `node` / `npm`, or `node < entry.node_min` → `PrerequisitesMissing`
- `git ls-remote <repo> <pin_tag>` parses peeled commit SHA from both annotated (two lines) and lightweight (one line) tags; `git ls-remote <repo> <sha>` is **not** exercised as a code path
- staging clone argv matches `init` / `remote add` / `fetch --depth=1 origin <pin_tag>` / `checkout FETCH_HEAD` (fetch by tag, not SHA)
- post-checkout `rev-parse HEAD == entry.expected_sha` (positive) and SHA mismatch (tag moved / wrong constant) → remove staging, `InstallFailed`
- `SKILL.md` identity match / mismatch (missing, wrong `name:`, malformed frontmatter) → `IdentityMismatch`, staging removed
- `npm ci --omit=dev --ignore-scripts` argv; non-zero exit → `InstallFailed`, staging removed, no destination write
- **Linux publish**: target absent + same filesystem → `renameat2 RENAME_NOREPLACE` succeeds → `Installed`, staging absent
- **Linux publish**: target already exists (regular dir, file, symlink) → `renameat2` returns `EEXIST` → `Conflict`, no-op, staging removed, target bytes unchanged; **no separate "exists" check before the primitive** (assertion in tests)
- **Linux publish**: cross-device staging → `EXDEV` → `InstallFailed`, staging removed, no copy fallback attempted
- **Windows publish**: target absent + same drive → `MoveFileW` returns non-zero → `Installed`, staging absent
- **Windows publish**: target already exists (regular dir, file, symlink) → `MoveFileW` returns `FALSE` / `ERROR_ALREADY_EXISTS` → `Conflict`, no-op, target bytes unchanged; **no separate "exists" check before the primitive** (assertion in tests)
- **Windows publish**: staging on a different drive → `MoveFileW` returns `FALSE` / `ERROR_NOT_SAME_DEVICE` → `InstallFailed`, staging removed, no copy fallback attempted
- publish: any other primitive error → `InstallFailed`, staging removed
- publish: other-platform paths are **not** asserted to succeed — tests do not invent a no-replace primitive for those platforms; the implementation fails closed there
- no `Command::arg` ever receives a shell-interpolated string (test assertion; no `sh -c` in the codebase)

Manual TUI verification on **both Linux and Windows** (other platforms are fail-closed for v1), with `XDG_CONFIG_HOME` / `%USERPROFILE%\.config` pointed at a temp dir, **after the verified `pin_tag` and `expected_sha` are set in `DEFAULT_CATALOG`**: `Tools` lists `pi-psql`; first install creates `<xdg|userprofile>/.config/opencode/skills/pi-psql/` with `SKILL.md` byte-equal to the staged copy and `node_modules` present; creating a stray file or symlink at the destination and re-running shows `Conflict` and leaves the target untouched. **No real credentials are added; the connection-manager UI is not exercised.**

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo build --release` all clean.
- Every test above passes; manual TUI verification succeeds on **both Linux and Windows**.
- `README.md` reflects `Tools`, the install-only contract, the Linux / Windows publish behavior, the Windows path caveat, the credential non-interference rule, and clarifies that remote catalog discovery remains out of scope.
- The four OpenCode-adaptation files (`README.md`, `SKILL.md`, `package.json`, `package-lock.json`) at `https://github.com/taneralberto/pi-psql.git` are reachable at the verified tag `opencode-2026-09-23` (peeled SHA `0dba366061911f0ec389f4a78cc46fd6d6a19d41`) without any local `C:\dev\pi-psql` checkout. The prior acceptance line "`C:\dev\pi-psql` is unmodified" is superseded: the local working tree is no longer the source of truth.

## Deferred decisions (do not invent)

- The verified `pin_tag` and `expected_sha` for `pi-psql`. The plan does not pick them; the orchestrator does, after upstream OpenCode-adaptation changes are published. (As of 2026-09-23 this is resolved: `opencode-2026-09-23` /
  `0dba366061911f0ec389f4a78cc46fd6d6a19d41`.)
- Project-local `.opencode/skills/` targets.
- Any update / force-install / uninstall action.
- Publish on platforms other than Linux and Windows (currently fail closed for v1).
- Any per-tool hook beyond `SKILL.md` `name:` identity; only add a catalog entry's validation when a concrete second tool needs one.
