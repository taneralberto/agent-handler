# Reusable API coverage (factual inventory)

Short, factual inventory of the callable agent / store / tools / models /
workflows / launcher surface in this tree, plus the tests that exercise each
item today. The point of this document is to make the API shape and its
existing coverage discoverable from a single file.

What this document is: a signed, row-level inventory. Each row pairs a
symbol with its kind, its coverage verdict, and the test(s) that drive it
(or `none` for untested). The signed rows below name the actual `fn`
names and file paths that exist today.

What this document is **not**: it is not a full runtime or platform
audit, it is not proof the reusable layer is complete, and it does not
assert tests that were not run. Coverage here reflects only the test
surface wired into the Linux build today.

## Scope and platform

- Linux test surface only. Windows runtime (MoveFileW, npm-via-node CLI
  shim, `cfg(windows)` branches) is not validated here; cross-target
  `cargo build` from Linux only verifies the code compiles, not that it
  works on a real Windows machine.
- Source of the surface: `src/agent.rs`, `src/store/{mod,canonical,
  settings,skills,sync}.rs`, `src/tools/{mod,pi_psql/mod}.rs`,
  `src/models.rs`, `src/workflows.rs`, `src/launcher.rs`.
- Source of the tests: inline `#[cfg(test)] mod tests` in each module,
  plus `src/store/skills_tests.rs` (separate-file test module) and
  `src/tools/tests.rs` (separate-file test module) and the ignored
  network smoke `tests/tools_install_smoke.rs`.

## How to read the matrix

Each row carries:

- **Symbol**: the public type or function name as exported.
- **Signature / kind**: the Rust type signature, abbreviated.
- **Coverage**: `covered` (≥1 focused test today), `partial` (tested
  indirectly via another function or only some inputs), `untested`
  (no test that exercises this surface today), or `ignored` (only the
  ignored network smoke test covers it).
- **Test(s)**: the test function name(s) and the file they live in.
  File paths are relative to the repo root.

The test names below are the actual `fn` names — they exist in the
file today. A blank test cell means the surface is exercised only via
another unit's test, or not at all.

---

## `agent` — `src/agent.rs`

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `Mode` | enum (subagent / primary / all) | covered | `rejects_invalid_mode_and_action`, `round_trip_with_explicit_model_and_all_modes`, `mode_cycle_round_trip` *(app/mod.rs)* |
| `Mode::as_str` | `fn(&self) -> &'static str` | partial | exercised transitively via `render()` in `round_trip_preserves_all_fields` and `round_trip_with_explicit_model_and_all_modes` *(agent.rs)* — all three variants (`subagent` / `primary` / `all`) reach `as_str()` through render; no focused test asserts the return value directly |
| `Mode::parse` | `fn(&str) -> Result<Self>` | covered | `rejects_invalid_mode_and_action` *(agent.rs)* |
| `Mode::next` | `fn(self) -> Self` | covered | `mode_cycle_round_trip` *(app/mod.rs)* |
| `PermissionAction` | enum (Allow/Ask/Deny) | covered | `rejects_invalid_mode_and_action`, `new_default_user_agent_has_safe_defaults` *(store/tests.rs)*, `round_trip_preserves_all_fields` |
| `PermissionAction::as_str` | `fn(&self) -> &'static str` | partial | exercised transitively via `render()` in `round_trip_with_explicit_model_and_all_modes` *(agent.rs)* — all three variants (`allow` / `ask` / `deny`) reach `as_str()` through render; no focused test asserts the return value directly |
| `PermissionAction::parse` | `fn(&str) -> Result<Self>` | covered | `rejects_invalid_mode_and_action` |
| `Agent` | struct (name/description/mode/model/prompt/permissions) | covered | every render / parse / validate test below |
| `Agent::validate_name` | `fn(&str) -> Result<()>` | covered | `rejects_invalid_name`, `accepts_kebab_case_names` |
| `Agent::validate_model` | `fn(&str) -> Result<()>` | covered | `rejects_invalid_model` |
| `Agent::validate_model_opt` | `fn(&Option<String>) -> Result<()>` | partial | exercised only indirectly via `apply_model_value` from `apply_model_validates_value` *(app/mod.rs)*; only the `Some(_)` arms (invalid + valid) are exercised, the `None` arm is not |
| `Agent::new_default` | `fn(String) -> Result<Self>` | covered | `new_default_user_agent_has_safe_defaults` *(store/tests.rs)* |
| `Agent::validate` | `fn(&self) -> Result<()>` | partial | covered transitively by save flows; no single-fn test |
| `Agent::render` | `fn(&self) -> String` | covered | `render_emits_permission_and_omits_tools`, `render_quotes_yaml_unsafe_text`, `round_trip_preserves_all_fields`, `round_trip_with_explicit_model_and_all_modes` |
| `Agent::render_pi` | `fn(&self) -> String` | covered | `render_pi_uses_pi_frontmatter_and_tools`, `pi_sync_renders_pi_subagents` *(store/tests.rs)* |
| `Agent::parse` | `fn(name: &str, source: &str) -> Result<Self>` | covered | `round_trip_preserves_all_fields`, `parse_normalizes_crlf_document_endings`, `parse_trims_model_before_storing_it`, `rejects_unknown_frontmatter_field`, `rejects_unknown_permission_key`, `rejects_invalid_mode_and_action`, `rejects_missing_frontmatter` |
| `Agent::read` | `fn(&Path) -> Result<Self>` | covered | `apply_safe_update_when_target_matches_last_installed`, `safe_removal_only_when_target_unchanged`, `save_canonical_rejects_collision_when_prior_is_none`, `save_canonical_rejects_stale_write` *(store/tests.rs)* |
| `canonical_path` | `fn(&Path, &str) -> Result<PathBuf>` | partial | exercised indirectly via `save_canonical` / `delete_canonical` / `rename_canonical` tests; no direct unit test |
| `STARTERS` (test-only) | `pub use starter_fixture::STARTERS` | covered | every `round_trip_preserves_all_fields`-style test |
| `starter_agent` (test-only) | `pub use starter_fixture::starter_agent` | covered | every render / parse / round-trip test |

## `store` — `src/store/mod.rs`

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `Paths` | struct (agenthd_root/canonical_dir/state_file/target_dir/pi_target_dir/skills_dir/settings_file) | covered | `paths_resolve_agenthd_root_follows_home_only`, `paths_resolve_target_falls_back_to_home_config`, `paths_resolve_does_not_pull_agenthd_under_xdg`, `paths_reject_empty_xdg`, `paths_reject_missing_home`, `paths_reject_empty_home`, `settings_default_canonical_dir_is_local_default`, `with_settings_repoints_canonical_dir`, `with_settings_rejects_missing_checkout`, `ensure_dirs_does_not_create_canonical`, `ensure_dirs_does_not_create_checkout_agents`, `startup_does_not_create_agenthd_agents` *(all in store/tests.rs)* |
| `Paths::resolve` | `fn(xdg, home) -> Result<Self>` | covered | see `Paths` row above |
| `Paths::from_env` | `fn() -> Result<Self>` | untested | no test exercises `Paths::from_env` directly; the delegated `Paths::resolve` body is covered by the `paths_resolve_*` / `paths_reject_*` tests in `store/tests.rs`, but the env-reading part (`env::var("XDG_CONFIG_HOME")` / `env::var("HOME")`) is never driven through `from_env` — the only call site in production code is `src/main.rs:49` |
| `Paths::with_settings` | `fn(self, &Settings) -> Result<Self>` | covered | `with_settings_repoints_canonical_dir`, `with_settings_rejects_missing_checkout`, `with_settings_repoints_canonical_dir` *(app/mod.rs)*, `ensure_dirs_does_not_create_checkout_agents` |
| `Paths::ensure_dirs` | `fn(&self) -> Result<()>` | covered | `ensure_dirs_does_not_create_canonical`, `ensure_dirs_does_not_create_checkout_agents`, `startup_does_not_create_agenthd_agents` |
| `State` | struct (installed / pi_installed / installed_skills) | covered | `state_recovery_when_source_and_target_match`, `stale_manifest_entries_are_cleaned_up`, `malformed_state_is_reported`, `state_load_compat_with_legacy_json_missing_installed_skills` *(skills_tests.rs)*, `state_round_trips_installed_skills` *(skills_tests.rs)*, `state_load_ignores_legacy_plugin_hash_field` *(app/mod.rs)*, `gated_setup_preserves_existing_ownership_state` *(app/mod.rs)* |
| `State::load` | `fn(&Path) -> Result<Self>` | covered | `malformed_state_is_reported`, `state_recovery_when_source_and_target_match`, `state_load_compat_with_legacy_json_missing_installed_skills` *(skills_tests.rs)*, `state_load_ignores_legacy_plugin_hash_field` *(app/mod.rs)* |
| `hash_file` | `fn(&Path) -> Result<Option<String>>` | partial | exercised by every `save_canonical_rejects_*` and `apply_safe_*` test; no isolated test |
| `write_target` | `fn(&Path, &[u8]) -> Result<()>` | covered | `atomic_write_target_replaces_file` |
| `load_canonical` | re-export from `store::canonical` | covered | `load_canonical_collects_invalid_files`, `load_canonical_accepts_empty_agents_directory`, `load_canonical_rejects_missing_canonical_dir`, `load_canonical_rejects_symlinked_source`, `load_canonical_on_valid_empty_dir_does_not_create_agenthd_agents` *(store/tests.rs)* |
| `save_canonical` | re-export | covered | `canonical_crud_and_rename`, `save_canonical_rejects_collision_when_prior_is_none`, `save_canonical_rejects_stale_write`, `save_canonical_does_not_recreate_missing_checkout`, `save_canonical_into_empty_but_existing_agents_dir_succeeds` |
| `rename_canonical` | re-export | covered | `canonical_crud_and_rename`, `rename_canonical_rejects_missing_source` |
| `delete_canonical` | re-export | covered | `canonical_crud_and_rename`, `delete_canonical_rejects_missing_source` |
| `load_settings` | re-export | covered | `load_returns_none_when_missing`, `save_and_load_round_trip` *(store/settings.rs)*, `settings_round_trip` *(store/tests.rs)*, `malformed_settings_is_an_error` *(store/settings.rs)* |
| `save_settings` | re-export | covered | `save_and_load_round_trip`, `settings_round_trip` |
| `settings_file_path` | re-export | covered | `settings_file_path_lives_under_root` *(store/settings.rs)* |
| `validate_checkout_path` | re-export | covered | `validate_rejects_missing_path`, `validate_rejects_non_directory`, `validate_rejects_missing_agents_subdir`, `validate_accepts_empty_agents_dir`, `validate_accepts_agents_dir_with_non_md_files`, `validate_rejects_relative_path`, `validate_accepts_a_real_checkout`, `validate_rejects_symlinked_root` *(all in store/settings.rs)* |
| `canonical_dir_from` | re-export | covered | `canonical_dir_uses_checkout_agents_subdir`, `canonical_dir_rejects_missing_path`, `canonical_dir_rejects_empty_checkout_path` *(store/settings.rs)*, `canonical_dir_from_rejects_missing_checkout`, `canonical_dir_from_rejects_empty_checkout_path` *(store/tests.rs)*, `startup_fails_closed_when_configured_checkout_missing` / `startup_fails_closed_when_checkout_lacks_agents_dir` *(app/mod.rs)* |
| `find_checkout_root_from` | re-export (`#[cfg_attr(not(test), allow(dead_code))]`) | covered | `find_checkout_root_finds_ancestor_with_agents`, `find_checkout_root_returns_none_when_no_ancestor_matches` *(store/settings.rs)*; also exercised via `settings_first_run_prefills_cwd_ancestor_hint_without_persisting` *(app/mod.rs)* |
| `apply_skills` / `plan_skills` | re-export from `store::skills` | covered | `plan_and_apply_adopt_byte_identical_unowned_tree` … `install_fails_closed_when_scratch_root_cannot_be_derived` *(store/skills_tests.rs, 45 tests)* |
| `OwnedSkill` / `SkillAction` / `SkillOutcome` / `SkillPlanItem` | re-export from `store::skills` | covered | same `skills_tests.rs` file |
| `apply_safe` / `compute_plan` / `plan_for` / `force_install` | re-export from `store::sync` | covered | `compute_plan_is_read_only`, `apply_safe_fresh_install`, `pi_sync_renders_pi_subagents`, `apply_safe_noop_when_up_to_date`, `apply_safe_update_when_target_matches_last_installed`, `safe_sync_preserves_unowned_target`, `safe_sync_preserves_externally_modified_owned_target`, `safe_removal_only_when_target_unchanged`, `force_install_overwrites_conflict`, `stale_manifest_entries_are_cleaned_up`, `state_recovery_when_source_and_target_match`, `canonical_crud_and_rename`, `rejects_non_regular_target`, `save_canonical_rejects_collision_when_prior_is_none`, `save_canonical_rejects_stale_write`, `ghost_manifest_entry_has_distinct_reason`, `force_install_adopts_when_target_now_matches_canonical`, `plan_includes_manifest_only_target_as_unowned`, `plan_includes_targets_with_duplicate_unowned_state`, `source_hash_matches_render`, `load_canonical_collects_invalid_files`, `load_canonical_accepts_empty_agents_directory`, `load_canonical_rejects_missing_canonical_dir`, `load_canonical_rejects_symlinked_source`, `load_canonical_on_valid_empty_dir_does_not_create_agenthd_agents`, `plan_for_rejects_missing_source`, `apply_safe_with_precomputed_remove_does_not_delete_target_after_source_removal`, `apply_safe_rejects_install_or_update_after_source_removal`, `save_canonical_does_not_recreate_missing_checkout`, `delete_canonical_rejects_missing_source`, `rename_canonical_rejects_missing_source`, `force_install_rejects_missing_source`, `save_canonical_into_empty_but_existing_agents_dir_succeeds`, `apply_safe_skips_update_when_target_changed_externally`, `apply_safe_skips_remove_when_canonical_reappears`, `apply_safe_skips_install_when_target_appeared_after_plan`, `apply_safe_continues_after_source_disappears_no_panic`, `apply_safe_skips_install_when_source_changed_after_plan`, `apply_safe_isolates_failed_rows_per_target`, `apply_safe_continues_successes_after_failed_row`, `apply_safe_rejects_stale_update_available_on_changed_target`, `apply_safe_skips_when_canonical_is_symlink_after_plan`, `apply_safe_skips_when_target_is_symlink_after_plan` *(all store/tests.rs)* |
| `SyncStatus` / `SyncTarget` / `SyncItem` / `ApplyOutcome` | re-export from `store::sync` | covered | every `apply_safe_*` test above |
| `SyncStatus::label` | method | covered | `apply_safe_fresh_install` *(store/tests.rs)* asserts the `"not installed"` label directly; other variants' `.label()` return values reach tests transitively through `apply_safe` *(store/sync.rs)* but are not asserted via `.label()` — direct enum match (`matches!` / `assert_eq!`) is what tests use for the other six variants |
| `SyncStatus::is_safe_action` | method | partial | exercised transitively through every `apply_safe_*` test (the planner checks the method at `store/sync.rs:405`); no focused test asserts the return value |
| `SyncStatus::requires_confirm` | method | untested (dead code) | the method is `#[allow(dead_code)]` and is not called by any production code or test |
| `SyncTarget::label` | `fn(self) -> &'static str` | untested | no test asserts the `"OpenCode"` / `"Pi"` return values; only production code (`src/app/install_update.rs`) calls `.label()` |
| `SyncTarget::dir` | `fn(self, &Paths) -> &Path` | partial | invoked transitively by every `apply_safe_*` test — the planner builds `SyncItem::target_path` via `target.dir(paths)` in `src/store/sync.rs` and `apply_safe` calls `target.dir(paths)` again at `src/store/sync.rs:840`; both variants (`OpenCode` / `Pi`) are exercised because the planner iterates `[SyncTarget::OpenCode, SyncTarget::Pi]`; no focused unit test asserts the return value directly |
| `SyncItem::reason` | method | covered | `ghost_manifest_entry_has_distinct_reason` |
| `require_canonical_source` | `pub(in crate::store)` | partial | exercised transitively through every missing-source safety test (the function is called from `canonical.rs`, `skills.rs`, and `sync.rs` in production code; tests like `plan_for_rejects_missing_source`, `with_settings_rejects_missing_checkout`, `apply_safe_*_after_source_removal` drive the production paths but never call `require_canonical_source` directly); no focused test asserts the function in isolation |

## `tools` — `src/tools/{mod,pi_psql/mod}.rs`

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `NodeMin` | struct | covered | `preflight_node_at_or_above_min_proceeds`, `preflight_node_below_min_returns_prerequisites_missing` |
| `ToolCatalogEntry` | struct | covered | `catalog_pin_matches_verified_remote`, `npm_program_resolves_to_platform_specific_launcher`, every `install_tool_*` |
| `DEFAULT_CATALOG` | `&[ToolCatalogEntry]` | covered | every catalog-aware test |
| `ToolStatus` | enum | covered | `status_label_for_each_variant`, every `install_tool_*` and `tool_status_*` test |
| `ToolStatus::label` | method | covered | `status_label_for_each_variant` |
| `ToolItem` | struct | covered | `tool_status_reads_destination_directory`, `tool_status_treats_file_or_symlink_at_destination_as_conflict` |
| `ToolOutcome` | struct | covered | every `install_tool_*` and `preflight_*` test |
| `SpawnSpec` / `SpawnOutput` / `SpawnRunner` / `RenameRunner` | types | covered | every `install_tool_*`, `linux_rename_noreplace_*`, `windows_rename_movefilew_*`, `npm_program_*`, `preflight_and_npm_ci_use_npm_program_as_their_program_field` |
| `destination_for` | `fn(&Paths, &ToolCatalogEntry) -> PathBuf` | partial | called transitively by every `tool_status_*` / `install_tool_*` / `linux_rename_noreplace_*` / `windows_rename_movefilew_*` test (and called directly in `tools/tests.rs` setups as a helper to compute the destination path), but no focused test asserts the `paths.skills_dir.join(entry.destination_subpath)` contract directly |
| `staging_for` | `fn(&Paths, &ToolCatalogEntry) -> PathBuf` | partial | invoked transitively by every `install_tool_with` test (`preflight_missing_git_returns_prerequisites_missing`, `preflight_node_at_or_above_min_proceeds`, `preflight_node_below_min_returns_prerequisites_missing`, `preflight_missing_node_returns_prerequisites_missing`, `preflight_missing_npm_returns_prerequisites_missing`, `preflight_node_unparseable_output_returns_prerequisites_missing`, `preflight_returns_prerequisites_missing_when_npm_cli_js_not_discoverable`, `pinned_sha_mismatch_is_install_failed_and_cleans_up_staging`, `npm_ci_failure_is_install_failed_and_cleans_up_staging`, `conflict_when_target_already_exists`, `conflict_does_not_run_a_preliminary_exists_check`, …) and called directly by the rename-platform tests (`linux_rename_noreplace_*`, `windows_rename_movefilew_*`) — but no focused unit test asserts the `.staging-{pid}-{counter}` format, so coverage is partial |
| `tool_status` | `fn(&Paths, &ToolCatalogEntry) -> Result<ToolItem>` | covered | `tool_status_reads_destination_directory`, `tool_status_treats_file_or_symlink_at_destination_as_conflict` |
| `install_tool` | `fn(&Paths, &ToolCatalogEntry) -> Result<ToolOutcome>` | partial | covered by unit tests with a mocked spawn runner (`install_tool_with` / `install_tool_at`); end-to-end against a real `git`/`node`/`npm` is only the ignored network smoke `real_install_pi_psql_against_remote` *(tests/tools_install_smoke.rs)*, so the mocked unit coverage is partial — not full coverage of the live installer path. |
| `install_tool_with` / `install_tool_at` | `fn` | covered | `preexisting_dir_at_staging_path_is_preserved_and_install_fails`, `preexisting_file_at_staging_path_is_preserved_and_install_fails`, `preexisting_symlink_at_staging_path_is_preserved_and_install_fails`, `conflict_when_target_already_exists`, `conflict_does_not_run_a_preliminary_exists_check` |
| preflight, ls_remote, stage, verify_pinned_sha, identity_check, npm_ci | internal helpers | covered | `preflight_*`, `parse_shas_*`, `ls_remote_path_argv_uses_tag_not_sha`, `staging_argv_matches_init_remote_fetch_checkout_revparse`, `pinned_sha_mismatch_is_install_failed_and_cleans_up_staging`, `identity_mismatch_*`, `npm_ci_argv_is_omit_dev_and_ignore_scripts_and_runs_in_staging`, `npm_ci_failure_is_install_failed_and_cleans_up_staging` |
| rename primitive (`rename_no_replace`) | internal + Linux / Windows / other (fail-closed) branches | covered | `linux_rename_noreplace_*`, `windows_rename_movefilew_*`, `other_platforms_fail_closed` |
| `parse_skill_name` | `pub(crate)` helper | covered | `identity_check_accepts_quoted_and_unquoted_names`, `plan_rejects_skill_name_mismatch` *(skills_tests.rs)* |
| `pi_psql::ENTRY` | const | covered | every `catalog_*` and `install_tool_*` test |

## `models` — `src/models.rs`

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `Discovery` | enum (Found/Empty/Failed) | covered | `status_text_for_each_variant`, `models_accessor` |
| `Discovery::models` | `fn(&self) -> &[String]` | covered | `models_accessor` |
| `Discovery::status_text` | `fn(&self) -> String` | covered | `status_text_for_each_variant` |
| `discover_models` | `fn() -> Discovery` | untested (spawns `opencode models`) | none — runs an external process; no test injects a runner |
| `parse_models` | `fn(&str) -> Vec<String>` | covered | `parses_basic_output`, `deduplicates_and_drops_invalid`, `strips_carriage_return`, `ignores_blank_and_comment_lines`, `accepts_punctuation_in_ids`, `empty_input_yields_empty`, `malformed_input_yields_empty` |
| `validate_identifier` | private helper | partial | exercised transitively by every `parse_models_*` test (the function is called from `parse_models` at `src/models.rs:92`); no focused test asserts the contract directly |

## `workflows` — `src/workflows.rs` (UI-independent layer)

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `CheckoutStatus` | enum (Ready/Stale/Empty) | covered | every workflow test below |
| `read_checkout` | `fn(&Path) -> Result<CheckoutStatus>` | covered | `read_checkout_valid_persisted_path_returns_ready`, `read_checkout_missing_file_returns_empty`, `read_checkout_invalid_persisted_path_returns_stale_with_raw_text`, `read_checkout_malformed_settings_is_an_error` *(workflows.rs)* |
| `ApplyError` | enum (Empty/NotAbsolute/Invalid/Save/Revalidate) | covered | `apply_checkout_*` tests *(workflows.rs)* |
| `ApplyError::message` | `fn(&self) -> String` | partial | `apply_checkout_empty_input_is_rejected` *(workflows.rs)* asserts the `Empty` message string and `apply_checkout_relative_input_is_rejected_with_exact_prefix` *(workflows.rs)* asserts the `NotAbsolute` message string — only 2 of 5 variants are asserted via `err.message()`; the `Invalid`, `Save`, and `Revalidate` variants reach `err.message()` only in production code (`src/app/settings.rs:428`), with no Settings-screen test asserting the displayed string |
| `apply_checkout` | `fn(&mut Paths, &str) -> Result<PathBuf, ApplyError>` | covered | `apply_checkout_empty_input_is_rejected`, `apply_checkout_relative_input_is_rejected_with_exact_prefix`, `apply_checkout_missing_path_is_rejected_without_persisting_or_mutating`, `apply_checkout_valid_path_persists_repoints_canonical_dir_and_does_not_create_dirs`, `apply_checkout_preserves_paths_on_failure` *(workflows.rs)* |
| `list_canonical_agents` | `fn(&Paths) -> Result<Vec<Agent>>` | covered | `list_canonical_agents_sorts_by_name_regardless_of_on_disk_order`, `list_canonical_agents_empty_checkout_returns_empty_vec`, `list_canonical_agents_fails_closed_when_checkout_missing` *(workflows.rs)* |

## `launcher` — `src/launcher.rs` (UI-independent boot path)

| Symbol | Kind | Coverage | Test(s) |
|---|---|---|---|
| `resolve_checkout_path` | `fn(&Paths, &[String]) -> Result<ResolveOutcome>` | covered | `resolve_persisted_valid_returns_ready_without_override_flag`, `resolve_persisted_stale_returns_banner_with_exact_prior_suffix`, `resolve_persisted_absent_returns_first_run`, `resolve_persisted_malformed_settings_propagates_error`, `resolve_repo_override_takes_precedence_over_persisted_settings`, `resolve_repo_override_relative_path_errors_without_writing`, `resolve_repo_override_does_not_rewrite_settings_when_unchanged` *(launcher.rs)* |
| `ResolveOutcome` | enum (Ready/StaleCheckout/FirstRun) | covered | same tests as above |
| `parse_repo_override` | `fn(&[String]) -> Result<Option<PathBuf>>` | covered | `parse_repo_override_missing_value_is_an_error`, `parse_repo_override_ignores_positional_arguments`, plus the `resolve_repo_override_*` tests above |

## Network smoke (ignored)

| Test | File | Notes |
|---|---|---|
| `real_install_pi_psql_against_remote` | `tests/tools_install_smoke.rs` | Runs `git ls-remote` / `git fetch` / `npm ci` against `https://github.com/taneralberto/pi-psql.git`. Gated `#[cfg(target_os = "linux")]` (the test binary does not even compile on Windows builds) and `#[ignore]` — does NOT run in `cargo test` on Linux. Network-dependent, so the smoke is opt-in: `cargo test --test tools_install_smoke -- --ignored --nocapture`. It is the only test that covers the live `install_tool` path end-to-end. |

## Summary of partial / untested rows

Exhaustive list of every `partial` and `untested` row in the matrix above.

- `Agent::validate_model_opt` — partial (exercised only indirectly via `apply_model_value` from `apply_model_validates_value` *(app/mod.rs)*; only the `Some(_)` arms covered, the `None` arm is not).
- `Agent::validate` — partial (no single-fn test; covered via `save_canonical` flows).
- `canonical_path` — partial (no direct test).
- `hash_file` — partial (covered transitively).
- `Paths::from_env` — **untested** (no test exercises `Paths::from_env`; the delegated `Paths::resolve` is covered, but the env-reading part `env::var("XDG_CONFIG_HOME")` / `env::var("HOME")` is never driven).
- `staging_for` — partial (invoked transitively by every `install_tool_with` test and called directly by the rename-platform tests; no focused unit test asserts the `.staging-{pid}-{counter}` format).
- `install_tool` — partial (covered by unit tests with a mocked spawn runner via `install_tool_with` / `install_tool_at`; the end-to-end live installer path is only the ignored network smoke `real_install_pi_psql_against_remote`).
- `destination_for` — partial (called transitively by every `tool_status_*` / `install_tool_*` / `linux_rename_noreplace_*` / `windows_rename_movefilew_*` test and called directly as a setup helper in `tools/tests.rs`; no focused test asserts the `paths.skills_dir.join(entry.destination_subpath)` contract directly).
- `validate_identifier` — partial (exercised transitively by every `parse_models_*` test; no focused test asserts the contract directly).
- `SyncStatus::is_safe_action` — partial (exercised transitively through every `apply_safe_*` test via `store/sync.rs:405`; no focused test asserts the return value).
- `SyncTarget::dir` — partial (invoked transitively by every `apply_safe_*` test — the planner builds `SyncItem::target_path` via `target.dir(paths)` and `apply_safe` calls `target.dir(paths)` again at `store/sync.rs:840`; both variants exercised; no focused unit test asserts the return value directly).
- `Mode::as_str` — partial (exercised transitively via `render()` in `round_trip_preserves_all_fields` and `round_trip_with_explicit_model_and_all_modes`; all three variants reached; no focused test asserts the return value directly).
- `PermissionAction::as_str` — partial (exercised transitively via `render()` in `round_trip_with_explicit_model_and_all_modes`; all three variants reached; no focused test asserts the return value directly).
- `ApplyError::message` — partial (only the `Empty` and `NotAbsolute` variants are asserted via `err.message()`; `Invalid`, `Save`, and `Revalidate` reach `err.message()` only in production code at `src/app/settings.rs:428` with no Settings-screen test asserting the displayed string).
- `require_canonical_source` — partial (exercised transitively through every missing-source safety test via production code in `canonical.rs` / `skills.rs` / `sync.rs`; no focused test calls the function in isolation).
- `SyncTarget::label` — **untested** (no test asserts the `"OpenCode"` / `"Pi"` return values; only production code calls it).
- `discover_models` — **untested** (no fake-runner seam for the `opencode models` spawn).
- `SyncStatus::requires_confirm` — **untested** (the method is `#[allow(dead_code)]` and never called or exercised by any test).

## Cross-cutting invariants (already pinned by tests, listed for orientation)

- **Validated-source-at-startup**: `with_settings_rejects_missing_checkout`, `validate_rejects_*`, `load_canonical_rejects_*`, `plan_for_rejects_missing_source`, every `apply_safe_*_after_source_removal` test.
- **SHA-256 + stale-write guard on `save_canonical`**: `save_canonical_rejects_stale_write`, `save_rejects_external_edit_during_editor_session` *(app/mod.rs)*, `save_rename_rejects_stale_source_without_moving_it` *(app/mod.rs)*.
- **Per-target sync isolation**: `apply_safe_isolates_failed_rows_per_target`, `sync_targets_opencode_and_pi_with_per_target_ownership` *(app/mod.rs)*.
- **Skills safety (no force-overwrite, scratch outside `skills_dir`)**: `pi_psql_at_destination_is_invisible_when_not_in_source`, `install_does_not_leave_staging_dir_under_skills_dir`, `update_success_does_not_leave_staging_or_backup_under_skills_dir`, `skills_o_key_is_declined_with_explanatory_message` *(app/mod.rs)*.
- **Settings first-run / recovery / save semantics**: every `settings_*` test in `app/mod.rs`.

## Linux-only — not validated

- `cfg(target_os = "windows")` branches (`MoveFileW`, `npm-cli.js` discovery seam, `windows_const_*`). The Linux-only symlink arm of `tool_status_treats_file_or_symlink_at_destination_as_conflict` (`#[cfg(unix)]`) is a Unix-only test branch, not a Windows-specific path — its file branch is platform-agnostic and runs on Windows. Cross-target build verifies the code compiles; runtime behavior on a real Windows host is not exercised here.
