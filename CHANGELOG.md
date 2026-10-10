# Changelog

## 0.1.6 (unreleased)

This section lists what was built and checked in the working tree. Nothing here is a published release: there is no tag,
no release archive and no commit for 0.1.6 yet. Items marked "not verified" were not run in this work.

### Native compiler is the default

- `ryn build`, `ryn run` and `ryn check` use the Ryn-written native compiler (`rync`) by default. `--backend cranelift`
  selects the Rust/Cranelift backend, which remains available for comparison.
- `ryn build` and `ryn run` with the native backend now run the same analysis as `ryn check` before the native compiler.
  Before this, `build` accepted programs that `check` rejects: a mutating `String` method on an immutable `self` was
  compiled and mutated the value. Such programs are now rejected with the same diagnostic as `check`, and no executable is
  written.
- `ryn bootstrap --native` rebuilds the native compiler from its own sources and requires stage 2 and stage 3 to be
  byte-identical. The last run in this work gave a fixpoint of 8,953,856 bytes.
- `ryn run` of a single file without `-o` builds the program into a scratch directory that is removed afterwards. Before,
  every run left an executable beside the source.

### Diagnostics: the Ryn analysis takes over rules

- The Ryn analysis (`ryn-frontend --sema`) reports `R0204` for a mutating `String` method on a shared reference, and
  `R0234` for `Map.clone` of values with a custom destructor, with the same text as the Rust analysis.
- `RYN_SEMA=ryn-only` runs the Ryn analysis alone for the IR and the diagnostics, skipping the Rust bootstrap analysis
  (the Rust constant evaluation and pattern desugaring still run). It is opt-in; the default path is unchanged.
- `tools/ryn-diagnostic-coverage.sh` compares the Ryn analysis with the Rust one on every invalid program of the corpus.
  `tests/fixtures/ryn_sema_coverage.tsv` records the outcomes, and `tests/ryn_sema_ratchet.rs` fails on any change, so a
  rule that starts or stops matching has to be recorded. The Ryn checks run in the compiler's order: constant checks
  (`--comptime`), then the analysis (`--sema`). Current outcomes over 195 programs: 141 exact, 35 declined by the Ryn
  analysis, 14 with a different code, 1 with different text, 4 accepted. Four of the accepted programs are module-level
  checks (`R0420`, `R0421`), which a single-file measurement cannot see; the module tests pass under `ryn-only`.
- The Ryn analysis also reports the bootstrap's messages for: an unknown function (`R0210`) and an unknown variable
  (`R0203`) with a did-you-mean hint, an unknown field in an assignment (`R0225`), an integer literal outside its type's
  range (`R0206`), and a `Vec` of a set (`R0234`), whether written as a type or built with `Vec<Set<..>>()`.
- With `RYN_SEMA=ryn-only`, the full test suite has 6 failing tests. Five need several independent diagnostics reported
  together (the Rust analysis recovers after an error and the Ryn analysis stops at the first). One needs `R0248` for a
  reference returned from a call, a flow rule. These are the remaining rules to port before the Rust analysis can be
  removed from the default path. Collecting several diagnostics needs a design decision, which is still open.

### Language and runtime fixes

- `extend String` methods with `mut self` changed the caller's value unreliably. A statement-level `String` method on a
  `&mut String` receiver passed the reference instead of the String handle, so `push` and `append` corrupted memory. This
  affected both backends and predates 0.1.6; it is fixed in the Rust and Ryn frontends and in the native backend.
- Cranelift now drops a discarded value that holds a destructor in a field, and frees owned fields of a discarded
  structure. Before, a bare call or a bare `take` of such a value was never dropped.
- Native programs support, among others: file handles and `read_file_result`, `String::repeat`, Linux keyboard input,
  slices of structures, enum and array payloads, `echo` of structures and enums, structure equality, `repr(C)` updates
  through methods and raw pointers, structure keys in maps, vectors of destructor values with `set`, `take` and
  consuming iteration, maps whose values have a destructor (insert, replace, remove, clear, scope end), and vectors
  and maps of structures whose destructor sits in a field.
- Maps and vectors whose values have destructors still reject `get`, `values` and `clone` with `R0234` in both frontends.
- The Ryn analysis and the Rust bootstrap analysis agree on programs that use `std::process`. A vector passed to a
  borrowing system builtin (`__process_capture`) is lowered as a clone in both, because the native runtime function
  consumes the vector it is given. Before, the consistency check reported `R0904` for these programs under `ryn check`
  and under every Cranelift build; the native path did not run the check, so the difference was not visible there.
- Float records: the runtime shims for `f32` and `f64` records (`ryn_ffi_float_pair_sum`, `ryn_ffi_make_float_pair`,
  `ryn_ffi_double_value`) are now built for every x86_64 target, and the native backend implements the same three
  functions in assembly. Before, they existed only on Windows, so `examples/ffi_float_records.ryn` failed to link on
  Linux. Verified on Linux x86_64 (see Verification below).
- The live-allocation counter `ryn_native_live_allocations` is declined with `R0905` in a program that links the C
  library. Its counter covers only the freestanding allocator, so the symbol used to be left unresolved at run time.

### Build and tooling

- The crate version is 0.1.6 (`Cargo.toml`, `Cargo.lock`).
- `build.rs` lists the Ryn sources of the native compiler and the standard library in `native_sources.rs`, so the compiler
  can write them out and build `rync`, and it compiles the runtime shim object for the build target.
- `install.sh` reads the release archive listing in full before searching it, so a `pipefail` shell no longer reports a
  killed `tar` instead of the match. It also checks that extracting the executable succeeded.
- `tools/test-native-sources.py` compiles suite programs directly with `rync`, without the Rust frontend, with a 1 GiB
  address-space limit per process.
- `.github/workflows/ci.yml` has a new Linux job `native`: it builds the release compiler, runs `ryn bootstrap --native`,
  and runs the native suite, runtime leak tests and backend comparison tests. This job was not executed here.

### Known differences and limits in 0.1.6

- `realpath(path, NULL)` returns `NULL` in native binaries, with `errno` 22, while the same call succeeds under the
  Cranelift backend. The standard library passes a 4096-byte buffer instead. Ruled out in this work: the NULL argument
  itself (`strtol` with a NULL end pointer, `getcwd(NULL, 0)`, and `realpath` with the NULL passed through a function
  parameter all behave correctly natively), `malloc` and `sysconf` and `pathconf` values, and absolute versus relative
  paths. The cause is not found. What would settle it: tracing the glibc calls made inside `realpath` in a native
  binary, which needs a tracer that this machine does not have.
- Thread ids are pthread ids on Linux.
- Keyboard input state (`key_down`, `key_pressed`, `key_released`) is not thread-safe.
- TCP and UDP `read` do not replace invalid UTF-8 with replacement characters.
- `ryn_native_live_allocations` and `ryn_native_live_bytes` count only freestanding native programs; programs that use
  file, process, thread or socket operations are linked against libc and are declined for these counters.
- The default `ryn check`, `ryn build` and `ryn run` still run the Rust analysis next to the self-hosted analysis, for
  the `R0904` consistency check and for diagnostics the self-hosted analysis does not yet produce. The native compiler
  does not depend on Rust at run time, and the standard path works with an empty `PATH`, but the path still contains
  Rust code. Removing it needs the self-hosted analysis to produce the messages the Rust analysis produces. For example,
  the Rust analysis has 50 `R0234` diagnostic sites and the self-hosted analysis has 16; the Map diagnostics for
  destructor values exist only in Rust.
- Windows float records were not re-run in this work; the shims are now compiled for all x86_64 targets.
- Native declines: `Map.get`, `Map.values` and `Map.clone` of destructor values are rejected by the frontends, so programs
  that reach the native backend never use them.

### Verification in this work

Run from the working tree in this session; nothing was committed, tagged or published.

- `cargo test` (full suite, all test targets) exits 0.
- `ryn bootstrap --native`: stage 2 and stage 3 byte-identical.
- Examples: native and Cranelift outputs (stdout, stderr, exit status) compared for every `examples/*.ryn`; the result is
  in the release notes of this work, not repeated here.
- Example project directories (ten with `src/main.ryn`) compared the same way.
- Runtime leak tests: file handles, processes, threads and sockets keep the open descriptor count unchanged over 100 to
  200 rounds. The descriptor count is read from `/proc/self/fd`.
- `ryn check`, `ryn build` and `ryn run` work with an empty `PATH`, so the standard path does not call `rustc`, `cc` or
  `ld`.
- Not verified: the CI workflows (not executed), Windows (not re-run), macOS (not supported).
