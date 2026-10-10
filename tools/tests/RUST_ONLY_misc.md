# Rust-only and adjusted test coverage (misc ports)

This file records what was not carried over when these Rust test files were ported to
`tools/tests/test_*.py`:

- `tests/native_runtime_leaks.rs`, `tests/native_ffi_memory.rs`, `tests/owned_strings.rs`,
  `tests/object_visibility.rs`, `tests/malformed_source.rs`, `tests/vec_of_enum.rs`,
  `tests/tuple_destructuring.rs`, `tests/syntax_extensions.rs`, `tests/range_values.rs`,
  `tests/optional_field.rs`.

Every dropped or adjusted piece is recorded below. Tests not listed here are ported with their full
Rust assertions (the only rename is `guard_emits_scope_cleanup_and_runtime_rejects_invalid_utf8_slices`,
see below). Nothing was dropped silently.

## Frontend-comparison cases dropped

The Rust tests ran these programs with both the default frontend and `--frontend rust`. The `ryn` CLI
has no `--frontend` flag, so only the default-frontend case is ported. The Rust-frontend case is
dropped for every test below.

| Rust test | Python test | Dropped |
| --- | --- | --- |
| `vec_of_enum_clones_sets_and_pops_elements` | `test_vec_of_enum_clones_sets_and_pops_elements` | `--frontend rust` run |
| `vec_of_enum_first_last_get_loop_take_and_insert` | `test_vec_of_enum_first_last_get_loop_take_and_insert` | `--frontend rust` run |
| `vec_of_enum_survives_repeated_clone_and_drop_cycles` | `test_vec_of_enum_survives_repeated_clone_and_drop_cycles` | `--frontend rust` run |
| `vec_of_option_holds_optional_values_and_nested_options` | `test_vec_of_option_holds_optional_values_and_nested_options` | `--frontend rust` run |
| `tuple_destructuring_binds_names_and_skips_underscores` | `test_tuple_destructuring_binds_names_and_skips_underscores` | `--frontend rust` run |
| `struct_destructuring_binds_fields_with_renames_nesting_and_rest` | `test_struct_destructuring_binds_fields_with_renames_nesting_and_rest` | `--frontend rust` run |
| `struct_destructuring_releases_owned_fields_on_every_iteration` | `test_struct_destructuring_releases_owned_fields_on_every_iteration` | `--frontend rust` run |
| `inclusive_ranges_include_their_end_and_never_wrap` (syntax_extensions) | `test_inclusive_ranges_include_their_end_and_never_wrap` | `--frontend rust` run |
| `defer_runs_on_every_exit_in_reverse_order` (syntax_extensions) | `test_defer_runs_on_every_exit_in_reverse_order` | `--frontend rust` run |
| `question_mark_runs_pending_defer_blocks_on_its_early_return` (syntax_extensions) | `test_question_mark_runs_pending_defer_blocks_on_its_early_return` | `--frontend rust` run of the option and result programs |
| `range_values_hold_bounds_and_iterate_without_overflow` | `test_range_values_hold_bounds_and_iterate_without_overflow` | `--frontend rust` run |
| `optional_field_chains_through_options_and_option_payloads` | `test_optional_field_chains_through_options_and_option_payloads` | `--frontend rust` run |

## Whole test dropped

- `tests/syntax_extensions.rs::bootstrap_frontend_matches_the_expected_output`: the test exists only to
  run `pipe_and_coalesce.ryn` with `--frontend rust`, so it has no default-frontend case. Dropped entirely.
  The default-frontend case of the same program is kept as
  `test_pipe_and_coalesce_run_with_the_expected_output`.

## Parts of tests dropped (library-only checks)

- `tests/owned_strings.rs::guard_emits_scope_cleanup_and_runtime_rejects_invalid_utf8_slices`: the first half
  calls `ryn::check` and inspects the lowered IR (`local_types` for `OwnedPtr`, and `IrStatement::Drop` in the
  loop body). The `ryn` CLI has no IR dump that exposes this, so that half is dropped. The second half, the
  runtime rejection of an invalid UTF-8 slice, is kept as `test_runtime_rejects_invalid_utf8_slices`. The
  name no longer says "emits scope cleanup" because the IR half is gone.
- `tests/owned_strings.rs::generated_cleanup_releases_every_allocation_after_returns_and_loop_exits`: the
  live String and Vec counters are read through a `rustc`-built wrapper around the emitted object, and that
  path uses `ryn::codegen::emit_object` and `src/runtime_shim.rs`. Those are library APIs. Dropped: the
  `live_strings=0` and `live_vectors=0` assertions. Kept: the program passes `ryn check` and `ryn build` and
  runs to exit status 0.
- `tests/malformed_source.rs::full_check_pipeline_does_not_panic_on_deterministic_malformed_sources`: the Rust
  test calls three in-process entry points per case, `ryn::check`, `ryn::check_recovering` and
  `ryn::parser::parse_recovering`. The CLI exposes only `ryn check`, so only that entry point runs. A crash
  is detected as a negative (signal) exit code or a `panicked` message on stderr. The generator, the fragment
  list, the separators, the seed and the 4,096 cases are the same as in Rust.

## Assertions adjusted

- `tests/native_runtime_leaks.rs::the_live_allocation_counter_is_declined_in_a_libc_program`: the Rust test
  expects the build to fail with exit status `1`. The self-hosted native backend reports a construct it lacks
  (R0905) with exit status `3`, as documented in `docs/self-hosting-plan.md` ("status 3"). The Python test
  asserts `3`, and the R0905 code and message are asserted unchanged. Reverting this to `1` would be the
  Rust-era behaviour.
- `tests/owned_strings.rs`: diagnostic spans are not exposed by the CLI as values. The Rust assertion
  `error.span.end > error.span.start` is replaced by checking that the rendered diagnostic has a caret (`^`).
- `tests/owned_strings.rs`: `ryn::check(...).expect_err(...)` is replaced by a nonzero `ryn check` exit and
  the expected `error[R....]` code on stderr. Only the first diagnostic in Rust was asserted; the CLI prints
  all of them, so the test checks that the expected code is among them.
