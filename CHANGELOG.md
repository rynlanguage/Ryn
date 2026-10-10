# Changelog

All notable changes to Ryn are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses a `0.x` version
sequence: minor releases may change language details while the compiler is pre-1.0.

## [Unreleased]

This section lists what was built and checked in the working tree on
`release/v0.1.6-native-first`. Nothing here is a published release: there is no tag, no release
archive and no commit for 0.1.6 yet. Items marked "not verified" were not run in this work.

### Native compiler is the default

- `ryn build`, `ryn run` and `ryn check` use the Ryn-written native compiler (`rync`) by default on
  Linux x86-64. `--backend cranelift` selects the Rust/Cranelift backend, which remains available for
  comparison and for other targets.
- `ryn build` and `ryn run` with the native backend now run the same analysis as `ryn check` before
  the native compiler. Before this, `build` accepted programs that `check` rejects: a mutating
  `String` method on an immutable `self` was compiled and mutated the value. Such programs are now
  rejected with the same diagnostic as `check`, and no executable is written.
- `ryn bootstrap --native` rebuilds the native compiler from its own sources and requires stage 2 and stage 3 to be byte-identical. The last run in this work gave a fixpoint of 9,084,928 bytes (sha256 `d10afe33…`), the same bytes as the cargo-free bootstrap from the committed seed.
- `ryn run` of a single file without `-o` builds the program into a scratch directory that is removed
  afterwards. Before, every run left an executable beside the source.

### Diagnostics: the Ryn analysis takes over rules

- The Ryn analysis (`ryn-frontend --sema`) reports `R0204` for a mutating `String` method on a shared
  reference, and `R0234` for `Map.clone` of values with a custom destructor, with the same text as the
  Rust analysis.
- The Ryn analysis alone is the default for `ryn check`, `ryn build` and `ryn run`; the bootstrap (Rust) analysis is not run on that path. A program the Ryn analysis does not handle is an error, not a reason to fall back to the Rust analysis. `RYN_SEMA=both` runs both analyses and reports a difference as `R0904`; `RYN_SEMA=rust` runs the Rust analysis alone.
- `tools/ryn-diagnostic-coverage.sh` compares the Ryn analysis with the Rust one on every invalid program of the corpus. `tests/fixtures/ryn_sema_coverage.tsv` records the outcomes, and `tests/ryn_sema_ratchet.rs` fails on any change, so a rule that starts or stops matching has to be recorded. The Ryn checks run in the compiler's order: constant checks (`--comptime`), then the analysis (`--sema`). Current outcomes over 195 programs: 143 exact, 33 declined by the Ryn analysis, 14 with a different code, 1 with different text, 4 accepted. Four of the accepted programs are module-level checks (`R0420`, `R0421`), which a single-file measurement cannot see; the module tests pass under the Ryn analysis alone.
- The Ryn analysis also reports the bootstrap's messages for: an unknown function (`R0210`) and an
  unknown variable (`R0203`) with a did-you-mean hint, an unknown field in an assignment (`R0225`),
  an integer literal outside its type's range (`R0206`), and a `Vec` of a set (`R0234`), whether
  written as a type or built with `Vec<Set<..>>()`.
- Several independent diagnostics are reported together, as the bootstrap reports them. Declarations are checked in its three groups (names declared twice; structure declarations and their cycles; signatures), and a group with errors ends the analysis; each function is then analyzed on its own. Inside a function a failed statement is recorded, and the statements after it are analyzed as if it had not been lowered. A failed `let` also ends its block, so later uses of its name are not reported again. A `for` whose name is declared already still checks its bounds and body. The mode is `ryn-frontend --sema-recover`, which writes one `diagnostic` line per error in source order; `--sema` still stops at the first. Tests: `tests/ryn_sema_recovery.rs` runs five such programs under both analyses, and `tests/diagnostics.rs` covers the multi-error cases.
- New rules in the Ryn analysis: `R0220` for a structure declared twice, and `R0238` for an enum declared twice or named like a structure (before, only the bootstrap reported them); `R0248` for a reference returned from a call, which is rejected whatever its arguments are, as in the bootstrap; `R0225` for an unknown tuple element such as `_2` in a destructuring.
- With the Ryn analysis alone, the full test suite passes: 405 tests in the default `cargo test` run and 405 in the `RYN_SEMA=ryn-only` run.

### Windows target (experimental)

- `rync --target windows` writes a PE32+ executable; `ryn.exe` and `rync.exe` are built that way and run under Wine
  (`ryn new`, `check`, `build`, `run` with the program's exit status). See `docs/native-compiler.md`. Verified under Wine
  only, not on Windows; threads, sockets, dynamic libraries, keyboard input and C callbacks are not available there yet.

### Language and runtime fixes

- `extend String` methods with `mut self` changed the caller's value unreliably. A statement-level
  `String` method on a `&mut String` receiver passed the reference instead of the String handle, so
  `push` and `append` corrupted memory. This affected both backends and predates 0.1.6; it is fixed in
  the Rust and Ryn frontends and in the native backend.
- Cranelift now drops a discarded value that holds a destructor in a field, and frees owned fields of
  a discarded structure. Before, a bare call or a bare `take` of such a value was never dropped.
- Native programs support, among others: file handles and `read_file_result`, `String::repeat`, Linux
  keyboard input, slices of structures, enum and array payloads, `echo` of structures and enums,
  structure equality, `repr(C)` updates through methods and raw pointers, structure keys in maps,
  vectors of destructor values with `set`, `take` and consuming iteration, maps whose values have a
  destructor (insert, replace, remove, clear, scope end), and vectors and maps of structures whose
  destructor sits in a field.
- Maps and vectors whose values have destructors still reject `get`, `values` and `clone` with
  `R0234` in both frontends.
- The Ryn analysis and the Rust bootstrap analysis agree on programs that use `std::process`. A vector
  passed to a borrowing system builtin (`__process_capture`) is lowered as a clone in both, because
  the native runtime function consumes the vector it is given. Before, the consistency check reported
  `R0904` for these programs under `ryn check` and under every Cranelift build; the native path did
  not run the check, so the difference was not visible there.
- Float records: the runtime shims for `f32` and `f64` records (`ryn_ffi_float_pair_sum`,
  `ryn_ffi_make_float_pair`, `ryn_ffi_double_value`) are now built for every x86_64 target, and the
  native backend implements the same three functions in assembly. Before, they existed only on
  Windows, so `examples/ffi_float_records.ryn` failed to link on Linux. Verified on Linux x86_64
  (see Verification below).
- The live-allocation counter `ryn_native_live_allocations` is declined with `R0905` in a program
  that links the C library. Its counter covers only the freestanding allocator, so the symbol used to
  be left unresolved at run time.
- A struct literal whose fields are written out of declaration order now compiles natively when it is a
  `return` value, a call argument or another value that is not a `let` initializer. Before, the native
  compiler refused it with `R0905` (a structure value out of field order). The fields are evaluated in the
  order they are written into temporaries and then passed in declaration order, so the evaluation order
  is the same as on Cranelift. Three suite tests cover it: one that records the evaluation order, one with
  an owned field (`String`) that checks the printed values, and one that builds such a literal as a
  return value and as a call argument 1000 times and checks that the live-allocation count is unchanged.

### Build and tooling

- The crate version is 0.1.6 (`Cargo.toml`, `Cargo.lock`).
- `bootstrap/rync-seed` (digest in `bootstrap/rync-seed.sha256`) is the native compiler that
  `tools/bootstrap-native.sh` starts from, so the bootstrap needs no Cargo or rustc. The seed
  reproduces itself from the sources. The release archive ships it as `rync`. How to replace it is in
  `docs/native-compiler.md`.
- `build.rs` lists the Ryn sources of the native compiler and the standard library in
  `native_sources.rs`, so the compiler can write them out and build `rync`, and it compiles the
  runtime shim object for the build target.
- `install.sh` reads the release archive listing in full before searching it, so a `pipefail` shell no
  longer reports a killed `tar` instead of the match. It also checks that extracting the executable
  succeeded.
- `tools/test-native-sources.py` compiles suite programs directly with `rync`, without the Rust
  frontend, with a 1 GiB address-space limit per process.
- `.github/workflows/ci.yml` has a new Linux job `native`: it builds the release compiler, runs
  `ryn bootstrap --native`, and runs the native suite, runtime leak tests and backend comparison
  tests. This job was not executed here.

### Known differences and limits in 0.1.6

- The Windows CI job fails on 7 suite programs, unchanged from `main` at v0.1.5:
  `ffi/sixteen_byte_float_record_to_and_from_c.ryn`, `ffi/sixteen_byte_integer_record_from_c.ryn`
  and `ffi/w10_ffi_libm_pow_floor_and_div_records.ryn` are rejected with `R0247`, because
  `#[repr(C)]` records of 9 to 16 bytes are accepted on non-Windows targets only
  (`c_abi_packed_record_layout` in `src/sema.rs`); `ffi/complex_float_record_to_and_from_c.ryn`,
  `ffi/libc_integer_functions.ryn` and `ffi/w4_libc_abs_labs_nested.ryn` compile but produce wrong
  values (`labs(-9000000000)` prints `410065408`, the 32-bit truncated magnitude); and
  `raii/r2_lead_stack_overflow_reports_error.ryn` terminates with access violation `0xC0000005`
  instead of the documented status 1.
- `realpath(path, NULL)` returns `NULL` in native binaries, with `errno` 22, while the same call
  succeeds under the Cranelift backend. The standard library passes a 4096-byte buffer instead. Ruled
  out in this work: the NULL argument itself (`strtol` with a NULL end pointer, `getcwd(NULL, 0)`,
  and `realpath` with the NULL passed through a function parameter all behave correctly natively),
  `malloc` and `sysconf` and `pathconf` values, and absolute versus relative paths. The cause is not
  found. What would settle it: tracing the glibc calls made inside `realpath` in a native binary,
  which needs a tracer that this machine does not have.
- Thread ids are pthread ids on Linux.
- Keyboard input state (`key_down`, `key_pressed`, `key_released`) is not thread-safe.
- TCP and UDP `read` do not replace invalid UTF-8 with replacement characters.
- `ryn_native_live_allocations` and `ryn_native_live_bytes` count only freestanding native programs;
  programs that use file, process, thread or socket operations are linked against libc and are
  declined for these counters.
- The Rust analysis is no longer run on the default path, but the Rust code is still used: by `RYN_SEMA=rust` and `RYN_SEMA=both`, by the Cranelift backend (`--backend cranelift`), by the constant evaluation and pattern desugaring that run before the Ryn analysis, by the CLI, and by the library's unit tests. Removing the Rust frontend and the Cranelift backend needs those uses moved first, so nothing was removed in this work, and the cargo dependencies are still needed.
- Windows float records were not re-run in this work; the shims are now compiled for all x86_64
  targets.
- Native declines: `Map.get`, `Map.values` and `Map.clone` of destructor values are rejected by the
  frontends, so programs that reach the native backend never use them.

### Verification in this work

Run in this work, on `release/v0.1.6-native-first`. Nothing is pushed, tagged or published.

- `cargo test` (all test targets) exits 0 with the default analysis (405 tests, 41 targets) and, in an earlier run before the code generator change, with `RYN_SEMA=ryn-only` (405 tests); the Ryn analysis alone is the default, so the two runs are the same configuration. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass.
- The three new suite tests for out-of-order struct literals fail with the previous seed (`R0905`) and pass with the new one: the evaluation order printed is `4 3 3 4 2 1 1`, the owning-field test prints `two 2 one 1`, and the loop test ends with 0 live allocations.
- Old seed against new seed: all 97 entries of `examples/` compile to byte-identical executables with both. Of the 631 `tests/suite` entries, 628 give the same result with both (identical executable bytes, or the same rejection status and message); the other three are the new out-of-order tests, which the old seed refuses with `R0905`. So the change does not alter the code generated for programs the previous seed accepted. This includes `realtime_input`, whose differing output in the example sweep therefore comes from the keyboard, which the program reads from `/dev/input/event*`.
- `tests/fixtures/ryn_sema_coverage.tsv` regenerated with `tools/ryn-diagnostic-coverage.sh`: two rows changed from DECLINE to EXACT (R0248 for a reference returned from a call; R0225 for a tuple element), and the ratchet test passes.
- `ryn bootstrap --native`: stage 2 and stage 3 byte-identical (9,084,928 bytes, sha256 `d10afe33…`).
- Cargo-free bootstrap: `tools/bootstrap-native.sh` with the committed `bootstrap/rync-seed` and `PATH=/usr/bin:/bin` completes; stage 2 and stage 3 are byte-identical and equal to the seed (9,084,928 bytes). The seed was rebuilt from the current sources, and the archive's own bootstrap reproduces it. The previous seed (`f0e02a3d…`) cannot compile the current sources, because they contain struct literals written out of declaration order, which it refuses with `R0905`; the new seed was therefore built with `ryn bootstrap --native`, which starts from the bootstrap compiler, and not from the old seed.
- Examples: all 97 entries of `examples/` (87 files and 10 project directories) build with the native
  default and with `--backend cranelift`. The two runs agree on stdout, stderr and exit status for
  96 entries. `realtime_input` reads the real keyboard, so its output depends on the keys pressed
  while it runs (one run printed `First key: D`, the other `First key: L`); it cannot be compared
  without a controlled input device, and this run does not show whether the backends agree on it. For
  `network_smoke`, an earlier run timed out on both backends, so that run showed only that both behave
  the same, not that the output is correct.
- Distribution without cargo: the release archive built from the seed was extracted into an empty
  directory. Its `rync`, run with `PATH=/usr/bin:/bin`, compiled and ran all 631 entries of
  `tests/suite` through `tools/test-native-sources.py` (474 that must run, 157 that must be
  rejected); all 631 pass. Its build of `examples/vec.ryn` prints the same output as `ryn run`. The
  archive's own `tools/bootstrap-native.sh`, run from the extracted directory with the same `PATH`,
  gives stage 2 and stage 3 byte-identical to the seed. The sweep used the repository's copy of the
  standard library, which is byte-identical to the archive's.
- Runtime leak tests: file handles, processes, threads and sockets keep the open descriptor count
  unchanged over 100 to 200 rounds. The descriptor count is read from `/proc/self/fd`.
- `ryn check`, `ryn build` and `ryn run` work with an empty `PATH`, so the standard path does not call
  `rustc`, `cc` or `ld`.
- Not verified: the CI workflows (not executed), Windows (not re-run), macOS (not supported).

## [0.1.5] — 2026-10-09

Deterministic ownership in the native backend and a verified self-hosted compiler bootstrap. No
garbage collector was introduced. Published as `v0.1.5` with Linux and Windows archives plus a
standalone native compiler archive.

### Native ownership

- Deterministic cleanup for `String`, `Vec`, `Map`, `Option`, `Result` and owning enum payloads:
  temporaries, replaced values, loop exits and early `?` propagation release their resources.
- Nested enum payloads and unborrowed reference fields are cloned independently; moved owners are
  invalidated so a repeated free is impossible, and the freestanding allocator traps on one.
- Scratch owners of temporaries are zeroed and included in early-return cleanup, so an early return
  releases them too.
- Added live allocation and live byte counters (`ryn_native_live_allocations`,
  `ryn_native_live_bytes`) with a 30,000,000-iteration regression that keeps both unchanged under a
  1 GiB address-space limit.
- Verified bootstrap: seed → stage2 → stage3 byte-identical (SHA-256
  `2e396bbdb65b1c6a0d058c0bcbe05e85ae9d3e1b32e70f1f49014a6a544f8da4`), 374,388 KiB peak RSS; the
  lowerer driver compiles natively in 158,100 KiB. Measurements: `docs/native-ownership-validation.md`.

### Compiler and language

- Consolidated the Ryn front end and middle end under `selfhost/src/`, with the native runtime, ELF
  writer, x86-64 generator and compiler driver, plus Rust analyzer parity checks.
- Output functions `print`, `eprint` and `write`; top-level `const` with a compile-time evaluator
  (`comptime`, `static_assert`, `field_count`, `variant_count`, `type_name`, `has_field`,
  `#[derive(Default)]`); literal, tuple, structure and nested `choose` patterns; generic `extend<T>`
  blocks; `#[repr(C)]` records; `extern "C"` callback parameters; `std::arena`.
- 571-program language suite in `tests/suite/`, native ownership and compiler project parity tests.

### Freestanding float text

- The native backend embeds the typed IR of the Ryn float runtime, so shortest round-tripping
  formatting and direct-width parsing (binary32 and binary64) need no libc. `std::math` runs on SSE2
  and x87: `sqrt`, rounding, logarithms, trigonometry with `fprem1` reduction, and `pow` with full
  special-case classification.
- The 17 previously known freestanding float-text declines all pass; the typed-IR sweep is 424/424.
  Measurements: `docs/native-float-validation.md`.
- `tools/bootstrap-native.sh` performs the Cargo-free bootstrap, `tools/test-native-sources.py` runs
  suite programs through `rync` only, and the Linux `rync-0.1.5-linux-x86_64` archive ships the
  self-hosted compiler with its sources.

## [0.1.3] — 2026-10-08

- Destructuring declarations for tuples and structures: `(a, mut b, _) := pair` and
  `Point { x, y: mut py, .. } := point`, nestable in any combination.
- Range values: `span := 5..20` and `span := lo..=hi` build a structure with `start`, `end` and
  `inclusive`; `for item in span` loops over one.
- Optional field access `option?.field`, where a chain stops at the first `None`. `?.` on a `Result`
  is rejected. `Option::Some(value)` without an expected type takes its payload type from the value.
- `defer { ... }` blocks run on normal exit, `return`, `break` and `continue`, in reverse order; an
  early return from `?` runs pending blocks first.
- Pipe operator `value |> f`, null-coalescing `option ?? fallback`, and inclusive ranges
  `for i in start..=end`.
- Text encoding of the typed IR with a round-trip test, checked by a Ryn program.
- Enum elements in `Vec` with clone and drop callbacks; `?.` and `(expr?).field` disambiguation after
  `?` propagation.

## [0.1.2] — 2026-10-07

- Version dependencies from the [pods](https://pods.ryn-lang.xyz) registry with Cargo-style
  requirements (`^`, `~`, `=`, `*`).
- `ryn lock` pins each package version and its archive SHA-256; the first build without `ryn.lock`
  creates it, and downloads are verified before unpacking.
- `ryn login <token>` and `ryn publish [project-dir]` upload packages; published versions are
  immutable.
- `ryn.yaml` accepts `description`, `license`, `repository`, `homepage` and `keywords`.

## [0.1.1] — 2026-10-07

Correctness update.

- Copyable method receivers used in a `when` / `else when` condition, a `for` range bound, or a
  `for ... in` collection are evaluated before the condition. Previously the temporary was emitted
  into the first statement of the branch or loop body, so a condition such as
  `when first(V::ZERO.add(V::ONE)) != 1` could read an uninitialized value.
- Raw pointers and function pointers can be compared with `==` and `!=`.
- An integer literal range start takes the end bound's type, so `for i in 0..items.count()`
  type-checks when `count()` returns `u64`.

## [0.1.0] — 2026-10-07

- Self-hosted compiler frontend: the Ryn lexer, parser and generic specializer are used by
  `check`, `build` and `run`; `ryn bootstrap` verifies a three-stage fixpoint.
- The Rust lexer and parser remain as the bootstrap and comparison implementation
  (`--frontend rust`).

## [0.0.5] — 2026-10-06

- Interactive console IO: `read_line()`, `ask(prompt)`, `stdin_read()`, direct stdout/stderr writes and
  explicit flushes.
- Expanded `String`, `Vec<T>`, `Map<K,V>`, `Set<T>`, `Option<T>` and `Result<T,E>` APIs, including
  parsing, conversions and owned-value operations.
- Filesystem and Path helpers, environment access, time and duration suffixes, math, process
  management, threads, TCP/UDP sockets and DNS.
- Realtime keyboard APIs on Windows and Linux (`Key`, `key_down`, `key_pressed`, `key_released`,
  `read_key`); Linux reads evdev devices under `/dev/input/event*` and needs OS permissions.
- `Path` parent-directory creation.

## [0.0.4] — 2026-10-06

- First Ryn-written type-checking pass in the self-hosted frontend, reachable through
  `ryn run examples/self_hosted_project -- --check <file.ryn>`. It checks primitive value classes,
  `Vec<T>` signatures, local declarations and assignments, simple function signatures and calls, and
  boolean control-flow conditions. It remains an experimental prototype outside the production
  compiler pipeline.
- Serialized self-hosted frontend integration tests.

## [0.0.1] — 2026-10-05

First bootstrap release.

- Lexer, parser, semantic checks, Cranelift native code generation and the `ryn` project CLI.
- Process arguments and the first examples and tests.
- Linux and Windows x86_64 release archives with installers.

[Unreleased]: https://github.com/rynlanguage/Ryn/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/rynlanguage/Ryn/compare/v0.1.3...v0.1.5
[0.1.3]: https://github.com/rynlanguage/Ryn/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/rynlanguage/Ryn/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/rynlanguage/Ryn/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/rynlanguage/Ryn/compare/v0.0.5...v0.1.0
[0.0.5]: https://github.com/rynlanguage/Ryn/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/rynlanguage/Ryn/compare/v0.0.1...v0.0.4
[0.0.1]: https://github.com/rynlanguage/Ryn/releases/tag/v0.0.1
