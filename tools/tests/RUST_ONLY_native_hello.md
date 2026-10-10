# Rust-only tests in tools/tests/test_native_hello.py

`tests/native_hello.rs` has 76 `#[test]` functions. `test_native_hello.py` ports all 76
one-to-one. Five of them depend on something the cargo-free CLI does not provide, so they
are skipped with a reason and are not run:

| Test (`NativeHelloTests.test_...`) | Reason |
| --- | --- |
| `dynamic_library_symbols_can_be_called_with_a_declared_c_signature` | Windows-only (`#[cfg(windows)]`). Skipped unless the host is Windows (`skipUnless`). |
| `borrowed_raw_pointer_field_casts_to_a_function_pointer` | Windows-only (`#[cfg(windows)]`). Skipped unless the host is Windows. |
| `ryn_functions_coerce_to_function_pointers_and_receive_native_callbacks` | Windows-only (`#[cfg(windows)]`). Skipped unless the host is Windows. |
| `failed_linker_removes_its_temporary_build_directory` | Rust-only: uses `--backend cranelift` and `RYN_LINKER=rustc` (the external Cranelift link driver). The native backend writes the executable itself. |
| `failed_link_preserves_an_existing_output_file` | Rust-only: calls the Rust library API `ryn::codegen::link_object` and `ryn::codegen::build_native`, and re-executes the Rust test binary as a child process. |

The other 71 tests run against `RYN_TEST_BIN`. None of them uses `--frontend rust`,
`RYN_SEMA`, `RYN_DUMP_SEMA` or `RYN_SEMA_STRICT`.
