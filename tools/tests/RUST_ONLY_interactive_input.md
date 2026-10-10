# Rust-only coverage: tests/interactive_input.rs

`tools/tests/test_interactive_input.py` ports all 48 `#[test]` functions of
`tests/interactive_input.rs` one-to-one, using the same method names (`test_<rust name>`),
the same Ryn programs, the same stdin bytes and the same assertions.

## Skipped tests

None. No test in the Rust file uses `--backend cranelift`, `--frontend rust`, `RYN_SEMA`,
`RYN_SEMA_STRICT`, `RYN_LINKER`, `RYN_DUMP_SEMA`, or the Rust library API, so no test is skipped.

## Platform branches

These tests branch on the target OS inside the test body. The Rust file uses `cfg!(windows)` and
`#[cfg(windows)]` for this. The Python port keeps both branches and selects one with
`IS_WINDOWS`, so each test still runs on its own platform:

- `test_process_builder_configures_and_waits_for_a_child_process` (`cmd.exe` / `sh`)
- `test_process_kill_stops_a_running_child` (`ping.exe` / `sleep`)
- `test_process_run_capture_returns_exit_code_and_owned_stdout_stderr` (`cmd.exe` / `sh`)

The Windows branches have not been run on Linux.

## Cargo-specific mechanisms replaced

| Rust mechanism | Python replacement |
| --- | --- |
| `env!("CARGO_BIN_EXE_ryn")` | `RYN_TEST_BIN`, set by `tools/run-tests.py <ryn-binary>` |
| `std::env::temp_dir()` and `std::process::id()` directories | `harness.TempDir` (unique, removed on exit) |
| `std::net::TcpListener`, `TcpStream`, `UdpSocket` | `socket` module |
| `std::thread::spawn` socket peers | `threading.Thread` peers, with exceptions re-raised by `join()` |
| `Command::output()` with no stdin | stdin is an empty byte string, which the child reads as EOF |
| `String::from_utf8_lossy` | `bytes.decode("utf-8", errors="replace")` |
