# Rust-only test coverage

`tools/run-tests.py` ports the four Rust test files that drive the `ryn` CLI:
`tests/cli_projects.rs`, `tests/diagnostics.rs`, `tests/compile_cases.rs` and
`tests/registry.rs` (54 `#[test]` functions in total).

## Ported tests that depend on Rust-only features

None. None of the 54 tests in these four files uses `--backend cranelift`,
`--frontend rust`, `RYN_SEMA`, `RYN_SEMA_STRICT`, `RYN_LINKER` or `RYN_DUMP_SEMA`.

Cargo-specific mechanisms were replaced without changing what is asserted:

| Rust mechanism | Python replacement |
| --- | --- |
| `env!("CARGO_BIN_EXE_ryn")` | `RYN_TEST_BIN`, set by `tools/run-tests.py <ryn-binary>` |
| `env!("CARGO_MANIFEST_DIR")` | paths relative to the repository root |
| `env!("CARGO_PKG_VERSION")` (`--version` test) | `RYN_EXPECTED_VERSION`, else the `version` line of the repo's `Cargo.toml` |
| `ryn::registry::sha256_hex` (library call) | `hashlib.sha256` |

## Platform-specific tests

These are ported but skipped on the platform they do not target:

- `CliProjectTests.test_clean_refuses_a_build_junction_and_preserves_its_target`
  is Windows-only (`mklink /J`). It is skipped on Linux.
- `CliProjectTests.test_clean_refuses_a_build_symlink_and_preserves_its_target`
  is POSIX-only and is skipped on Windows.

## Other Rust test files not covered by this harness

These are outside the four files above and are not ported. Markers checked by grep:

- `tests/optional_field.rs`, `tests/range_values.rs`, `tests/tuple_destructuring.rs`,
  `tests/vec_of_enum.rs`, `tests/syntax_extensions.rs`: run programs with `--frontend rust`.
- `tests/native_backend.rs`: sets `RYN_DUMP_SEMA`.
- `tests/native_hello.rs`: sets `RYN_LINKER=rustc`.
- `tests/ryn_sema_diagnostics.rs`: sets `RYN_SEMA_STRICT` and uses the Rust `ryn::frontend` API.
- `tests/ryn_ast_roundtrip.rs`, `tests/ryn_ir_roundtrip.rs`, `tests/ryn_sema_projects.rs`,
  `tests/ryn_sema_subset.rs`: use the Rust `ryn::frontend` API.
- All other `tests/*.rs` files were not examined in this port.
