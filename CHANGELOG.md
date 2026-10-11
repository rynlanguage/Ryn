# Changelog

All notable changes to Ryn are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses a `0.x` version
sequence: minor releases may change language details while the compiler is pre-1.0.

## [0.1.7] — 2026-10-11

"Native Power & Benchmarks". The version strings (`VERSION`, `ryn --version`, `rync --version`, the registry header, the CI
archive names) say 0.1.7.

### Nested structures

- A path of fields is stored through a reference: `self.q.p.y = 1` and `l.a.x += 1` in a `mut self` method or through a
  `&mut` parameter (the native backend declined these as "assignment through this reference"). The store goes to the
  address of the nested field (`PlaceAddress` in the IR), so the old value is dropped and the new one moved in once.
- A `mut self` method is called on a structure nested in another one (`self.b.shift(1)`, `l.b.shift(1)`), in place, with the
  mutability of the root checked (`R0204` on an immutable local). It was `R0235` ("method receivers must be a local
  variable").
- A field of a structure nested in the result of a call is read (`make().q.p.x`), and the rest of the result is dropped.
- A method is called on a `String` field of a call result (`make().label.len()`, `make().q.p.name.clone()`): the string moves
  into a hidden local that lives as long as the enclosing block, so nothing leaks or is freed twice (it was R0905). A `let`
  whose statement needed such a temporary no longer drops the bound name at the end of the statement.
- A structure stored through a pointer as a whole (`self.seen = ...` of a `Vec`, a `String` or a nested structure) works
  for the native backend; it was declined as "a structure field of a structure type". This is what stopped programs with
  a collection field changed through `self` inside loops from compiling.
- C calls with more than six integer arguments pass the rest on the stack, for named functions and for calls through a
  function pointer (scalar arguments only; records and the freestanding and Windows targets still decline).
- The help of `R0905` no longer points at the removed `--backend cranelift`.
- Tests: `tests/suite/structs/nested_struct_*`, `vec_field_mutated_through_self_in_loops`,
  `raii/nested_destructor_runs_once_per_resource`, `ffi/foreign_call_with_stack_arguments`.

### `ryn bench`

- `ryn bench <file.ryn|project-dir>` compiles the program, times the compiler (`--compile-runs N`), runs the program
  `--warmup N` times without counting and then `--iterations N` times with its output captured, and prints the minimum,
  median, mean, maximum and standard deviation. `--json` prints the report as JSON, `--save-baseline FILE` writes it,
  `--baseline FILE --threshold PERCENT` compares medians (default 10%) and exits 1 on a regression. A project with a
  `benches/` directory times every `.ryn` file in it by name. Written in Ryn (`selfhost/src/driver/bench.ryn`).
- The peak resident memory of the compiler and of the program is reported and written to the JSON (`peak_memory_kb`), and a
  baseline that has it makes memory growth past the threshold a regression too. `std::process::ProcessResult` has a new
  field `peak_memory_kb` (the child's `ru_maxrss` from `wait4`; 0 on Windows, where it is not measured).
- Tests: `tools/tests/test_bench.py`.

## [0.1.6] — 2026-10-11

"Native First". The `ryn` command, the native compiler and the runtime are written in Ryn, and the Rust implementation and
the Cranelift backend are removed. There is also an experimental Windows target. Items marked "not verified" were not run.


### The Rust implementation is gone

- The tree has no Rust. The `ryn` command (`selfhost/src/ryn.ryn`), the native compiler `rync`, the runtime and the
  Windows target are written in Ryn. `src/`, `Cargo.toml`, `Cargo.lock`, `build.rs`, `benches/` and the 34 Rust test
  files were deleted. Everything stays in the git history; the commit before the removal is the last that builds with
  Cargo (the local branch `rust-reference-0.1.6` points at it).
- Removed with the Rust code: the Cranelift backend (`--backend`), `--frontend`, `RYN_SEMA`, `RYN_LINKER`,
  `RYN_DUMP_SEMA`, `ryn ast`, `ryn bootstrap`, the `ryn-frontend` comparison tests, and `tools/ryn-diagnostic-coverage.sh`.
  Programs, projects, diagnostics and manifests that worked with the native backend work the same.
- The compiler is built by itself from the committed seed: `tools/bootstrap-native.sh` (stage 2 must equal stage 3,
  and the seed equals both). `tools/make-dist.py` builds the Linux and the Windows archives with that compiler.
  Compiler changes that the seed cannot compile are built in two steps (see `CONTRIBUTING.md`).
- `ryn` now also does `lock`, `login` and `publish` (registry, Git and path dependencies, SHA-256, `ryn.lock` text
  identical to what the Rust command wrote); they use the system `curl`, `tar` and `git`.
- `rync` reports every independent error, rejects invalid UTF-8 sources and a project manifest without `name`, and
  writes the executable through a temporary file renamed over the output, so an output that is a link to a source file
  never overwrites the source (the old Rust CLI refused such an output; now the link is replaced).
- Ownership: moving a field out of a local makes the whole value unusable (`R0240`) while the other fields stay
  usable; a store into the field makes it available again. An argument that moves the receiver of the method call is
  `R0242`. These were rules of the Rust guard that the Ryn analysis did not have; the Python tests that ported the Rust
  tests found them.
- Recovery: after an invalid `for` range start, the end and the body are still analyzed. `String::clear` works natively.
- Tests: the Rust tests were ported to Python (`tools/tests`, run by `tools/run-tests.py`; 241 tests, 6 skipped because they are Windows-only or need a Windows host), and
  `tools/tests/REMOVED_RUST_TESTS.md` lists, for each Rust test file, what it checked and why it was not ported (it used
  the Rust implementation as an oracle or called its library API). The diagnostics of the corpus are checked against the
  code and message the Rust analyzer produced (`test_diagnostic_corpus.py`, 195 rows). Not kept: the exact spans, help
  texts and suggestions of the Rust analyzer, and the AST/IR round-trip tests of the Rust codecs.

### Windows target (experimental)

- `rync --target windows` writes a PE32+ executable; a `rync` that runs on Windows targets Windows by default.
  `ryn.exe` and `rync.exe` are built that way (`tools/make-dist.py` cross-compiles them on Linux). See
  `docs/native-compiler.md`.
- Under Wine 10 (not on Windows itself): `ryn.exe new/check/build/run` work and pass the program's exit status; the
  Windows `rync.exe` compiles and runs 618 of 631 suite entries; 629 of 636 entries pass when compiled for Windows by the
  Linux compiler and run under Wine.
- Not available on Windows yet, and declined when compiling: threads, sockets, `pipe`/`fork`, keyboard input, records
  passed to or returned from C by value. The CI workflow has a job that runs `ryn.exe` on a real Windows runner; it has
  not been run.

### Diagnostics and ownership

- `ryn build` and `ryn run` run the same analysis as `ryn check` before code generation; before, `build` accepted
  programs that `check` rejected (a mutating `String` method on an immutable `self` was compiled and mutated the value).
- The Ryn analysis reports the original messages for, among others: unknown function (`R0210`) and unknown variable
  (`R0203`) with a did-you-mean hint, unknown field in an assignment (`R0225`), integer literal out of range
  (`R0206`), `Vec` of a set (`R0234`), `R0204`, `R0220`, `R0238`, `R0248`, and the `R0240`/`R0241`/`R0242` ownership
  rules (moving a field makes the whole value unusable; an argument that moves the receiver of a method call is `R0242`).
- Several independent errors are reported together (declarations in three groups, then each function and each
  statement); after an invalid `for` range start the end and the body are still analyzed.
- `tests/fixtures/ryn_sema_coverage.tsv` records the code and message the original analyzer gave for 195 invalid
  programs; `tools/tests/test_diagnostic_corpus.py` requires `rync check` to give the same (all match).


### Language and runtime fixes

- `extend String` methods with `mut self` changed the caller's value unreliably. A statement-level
  `String` method on a `&mut String` receiver passed the reference instead of the String handle, so
  `push` and `append` corrupted memory. This affected both backends and predates 0.1.6; it is fixed in
  the native compiler (the Rust frontends it was also fixed in were removed later in this release).
- Native programs support, among others: file handles and `read_file_result`, `String::repeat`, Linux
  keyboard input, slices of structures, enum and array payloads, `echo` of structures and enums,
  structure equality, `repr(C)` updates through methods and raw pointers, structure keys in maps,
  vectors of destructor values with `set`, `take` and consuming iteration, maps whose values have a
  destructor (insert, replace, remove, clear, scope end), and vectors and maps of structures whose
  destructor sits in a field.
- Maps and vectors whose values have destructors still reject `get`, `values` and `clone` with
  `R0234` in both frontends.
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

### Build, tooling and distribution

- `tools/bootstrap-native.sh <seed> <dir>` builds the compiler from the committed seed; `bootstrap/rync-seed` (digest in
  `bootstrap/rync-seed.sha256`) is a fixed point of the committed sources. `tools/make-dist.py` builds the Linux archive
  and the cross-compiled Windows archive. `tools/list-suite.py` lists the suite entries, `tools/test-native-sources.py`
  runs them (`RYN_TEST_TARGET=windows` for Windows under Wine), and `tools/run-tests.py` runs the Python tests.
- `VERSION` holds the version. `install.sh` and `install.ps1` install the whole archive directory, since `ryn` finds
  `rync` and `stdlib` next to itself. `install.sh` reads the archive listing in full before searching it, so a
  `pipefail` shell no longer reports a killed `tar` instead of the match.
- `.github/workflows/ci.yml` bootstraps to a fixpoint, runs the suite and the Python tests, runs the Windows target under
  Wine, and runs `ryn.exe` on a Windows runner; `release.yml` builds both archives. Not executed before the tag.
- `ryn run` of a single file without `-o` builds the program into a scratch directory that is removed afterwards.


### Known differences and limits

- **Windows was run under Wine only, never on Windows itself.** Threads, sockets, `pipe`/`fork`, keyboard input and
  records passed to or returned from C by value are not available there and are declined when compiling. A thread-safe
  allocator would be needed first for threads.
- The GitHub workflows (CI and release) were rewritten for the Cargo-free build and had not been run when 0.1.6 was
  tagged.
- Linux x86-64 is the only supported native host; macOS is not supported.
- The exact spans, help texts and suggestions of the removed Rust analyzer are no longer pinned by tests; codes and
  messages are.
- An output that is a link to a source file is replaced by the executable (the source is not touched); the removed Rust
  CLI refused such an output.
- `realpath(path, NULL)` returns `NULL` in native binaries, with `errno` 22. The standard library passes a 4096-byte
  buffer instead. The cause is not found; ruled out: the NULL argument itself, `malloc`/`sysconf`/`pathconf` values, and
  absolute versus relative paths.
- Thread ids are pthread ids on Linux. Keyboard input state (`key_down`, `key_pressed`, `key_released`) is not
  thread-safe. TCP and UDP `read` do not replace invalid UTF-8 with replacement characters.
- `ryn_native_live_allocations` and `ryn_native_live_bytes` count only freestanding native programs; programs that use
  file, process, thread or socket operations are linked against libc and are declined for these counters.
- Native declines: `Map.get`, `Map.values` and `Map.clone` of destructor values are rejected by the frontends, so
  programs that reach the native backend never use them.
- The two files `tests/native/ownership_30m.ryn` and `tests/native/ownership_paths.ryn` have no `// test:` header and
  are not run by the suite runner.


### Verification after the Rust removal

Run from a clean `git clone` of the commit that removed Rust, with `PATH=/usr/bin:/bin` (no `cargo`, `rustc`, `cc` or
`ld` on it):

- `sha256sum --check bootstrap/rync-seed.sha256`: ok.
- `tools/bootstrap-native.sh bootstrap/rync-seed <dir>`: stage 2 and stage 3 byte-identical, and equal to the committed
  seed (sha256 `1fe730f4...`).
- The suite plus `tests/native` through that compiler: 639 of 641 pass; the other two (`tests/native/ownership_*.ryn`)
  have no `// test:` header and are skipped by design.
- `tools/make-dist.py` builds `ryn-0.1.6-linux-x86_64.tar.gz` and `ryn-0.1.6-windows-x86_64.zip`; `tools/run-tests.py`
  against the extracted Linux `ryn`: 241 tests pass, 6 skipped. The extracted Windows archive runs `ryn.exe run` under Wine.
- Not verified: the GitHub workflows (not executed), Windows itself (only Wine), macOS (not supported).

### Earlier verification (before the Rust removal)

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
