# Rust test files: what they check and what replaces them

Each of the 16 Rust test files in `tests/` is classified for its removal with the Rust implementation (`src/*.rs`).
"Oracle" means the Rust code is the source of the expected results; "API" means the test calls Rust library code.

Ported by this change (Python, `tools/tests/`):

- `test_diagnostic_corpus.py`: the recorded Rust diagnostics of `tests/fixtures/ryn_sema_coverage.tsv`, checked against `rync check`
  (`ryn_sema_ratchet.rs`). See `DIAGNOSTIC_GAPS.md`.
- `test_recovery.py`: `ryn_sema_recovery.rs` and the recovering-check cases of `compiler_api.rs`.
- `test_build_native.py`: the native cases of `backend_selection.rs`, and the build-output cases of `compiler_api.rs`.

Removal order matters for one file: the expected spans, help texts and suggestions in `ryn_sema_diagnostics.rs` exist only in
that file and in the Rust analyzer. Snapshot them before `src/*.rs` is deleted (see that entry).

| File | What it checks | Needs Rust (oracle or API)? | Keep an equivalent? |
| --- | --- | --- | --- |
| `ast_text_dump.rs` | Writes the Rust-encoded syntax tree of `examples/` and `tests/programs/pass/` to `RYN_AST_OUTPUT_DIR`. No assertions. | Yes: producer (Rust parser and encoder). | No. The inputs for the Ryn AST tool come from `ryn-frontend <file> <out.ast>` (the Ryn parser). |
| `ir_text_dump.rs` | Writes the Rust-encoded IR of the pass programs to `RYN_IR_OUTPUT_DIR`. No assertions. | Yes: producer (Rust sema and encoder). | No. Use `ryn-frontend --sema` output as the IR inputs instead. |
| `ir_codec.rs` | Rust IR encode, decode and re-encode give the same text for every pass program and at least half the examples. | Yes: Rust codec and Rust sema. | Yes, reworked: IR from `ryn-frontend --sema` must survive `--ir-roundtrip` byte for byte. Needs a `ryn-frontend` build in the test run, which `run-tests.py` does not have yet. Not ported here. |
| `ryn_ast_roundtrip.rs` | The Ryn syntax tree model (`selfhost/src/middle/ast.ryn`, `--ast-roundtrip`) decodes every Rust-encoded tree and re-encodes it identically; rejects truncated and corrupted text. | Yes for the accept set (Rust encoder). The reject set (truncated, unknown tag) needs no oracle. | Yes. Use the trees from `ryn-frontend` as the accept set, and keep the reject checks. Needs a `ryn-frontend` build. Not ported here. |
| `ryn_ir_check.rs` | The Ryn IR checker (`selfhost/ir`) accepts every Rust-encoded IR and rejects truncated and corrupted text. | Yes for the accept set. The reject set needs no oracle. | Yes, same as above: accept set from `ryn-frontend --sema`; `selfhost/ir` exists and `ryn check selfhost/ir` passes. Not ported here. |
| `ryn_ir_roundtrip.rs` | The Ryn IR model (`--ir-roundtrip`) decodes and re-encodes Rust IR identically; rejects damaged text. | Same as `ryn_ast_roundtrip.rs`. | Yes, same as `ryn_ast_roundtrip.rs`. |
| `self_hosted_frontend.rs` | Runs `examples/self_hosted_project` with `ryn run`: valid samples print `ok`, invalid samples print an error, the type checker prints `ok (types)` or `type check failed`. | No. The expected outputs are literals. | Yes, and it can be ported as it is: pure CLI. Not ported here. |
| `self_hosted_parity.rs` | The self-hosted parser must produce the same syntax tree and first error as the Rust parser for every source, generated input, truncated input and specialization. Also a fixpoint: the frontend built by itself is byte-identical to the one built by the bootstrap. | Parity part: yes (Rust parser and specializer). Fixpoint part: no. | Fixpoint only, as a release gate, not a unit test: build `selfhost/src/rync.ryn` with the stage-1 binary and compare with the stage-2 binary. It takes about three minutes per build, using `bootstrap/rync-seed`. Parity is dropped. |
| `ryn_sema_diagnostics.rs` | (a) On the corpus and the projects, `RYN_SEMA_STRICT` reports no `R0904`, meaning the Ryn-only diagnostics agree with the bootstrap. (b) Unknown-name suggestions (`did you mean `b`?`, `Option::None`). (c) About 150 invalid programs: the Ryn `--sema` diagnostic must equal the Rust code, span, message and help exactly. | (a) and (c): yes, Rust analyzer is the oracle. (b): no. | (b): yes, verified through `ryn check` (`did you mean `b`?` appears). Port in a later change. (c): snapshot the expected code, span, message and help as a fixture before `src/*.rs` is removed. The tsv has codes and messages only, so spans and help texts would be lost otherwise. |
| `ryn_sema_projects.rs` | The Ryn analysis yields the same IR as the Rust sema for the compiler's own projects `selfhost/ir` and `selfhost`. | Yes: Rust sema IR is the oracle. | Partly: `ryn check selfhost` and `ryn check selfhost/ir` must pass (both do now). The IR equality is dropped unless golden IR is snapshotted. |
| `ryn_sema_subset.rs` | The Ryn subset of sema yields the same IR as the Rust sema for every program it accepts, and declines the rest with status 3. Requires at least 100 accepted programs. | Yes for the IR equality. The decline status is a property of the Ryn tool. | Only with a golden IR snapshot taken before removal. The 100-program floor is a useful guard if the snapshot is kept. |
| `ryn_sema_ratchet.rs` | Each invalid program's Ryn outcome (EXACT, WRONG_TEXT, WRONG_CODE, ACCEPT, DECLINE, PARSE_FAIL) must equal the recorded class in the tsv. | Yes for the classes: they are defined against the Rust analyzer. | Replaced by `test_diagnostic_corpus.py`, which asserts the recorded code and message directly. `tools/ryn-diagnostic-coverage.sh` still calls `ryn check --frontend rust` and must be rewritten before Rust is removed. |
| `ryn_sema_recovery.rs` | Each independent error in one file is reported, in order: five programs (two errors in one function, failed `let`, failed block, failed return, failed loop header). Both analyses must agree. | No: the expected lists are literals. The two-analysis comparison is gone. | Yes, ported: `test_recovery.py`. |
| `compiler_api.rs` | Library API: `compile`, `compile_source`, `check`, `check_recovering`, `SourceFile`, `CompileError` variants, native object emission, output-overwrite refusal, native setup failures. | Yes: the Rust library API. | Behaviours reachable through the CLI are ported. Recovering checks (lexer, parser, structure cycles, duplicate and range names) go to `test_recovery.py`; build and native setup, the overwrite refusal and the blocked-output case go to `test_build_native.py`. Not ported: typed error classes, the `OutputWouldOverwriteSource` variant, `emit_object` and `link_object`, and the rendered source context (`src/main.ryn:2:10` with a caret). The CLI renders snippets, covered by `test_diagnostics.py`. |
| `backend_selection.rs` | `--backend cranelift` and `--backend native` agree on collections and FFI programs; `run` leaves no executable; native build rejects what check rejects; unknown backends are rejected with `R0907`. | Cranelift half: yes, the Rust backend. The cranelift half is gone: `--backend cranelift` is now refused. | Native cases ported: `test_build_native.py` (native output and exit status per the program headers, `run` leaves no executable, native build rejects `R0204` with no output, `llvm` and `cranelift` refused with `R0907`). |
| `native_backend.rs` | The Ryn native backend (`selfhost/src/native.ryn`, driven from IR written by `RYN_DUMP_SEMA`) produces objects that print and exit as the headers say; a program outside the subset is declined with its reason; ownership paths leak nothing; 30 million temporary allocations fit in 1 GiB; a double free traps with SIGILL. | No: the expectations are headers and literals. It uses the Ryn driver, not Rust code, but reads the Ryn IR dump. | Yes, through the CLI, which is cheap: `ryn build --backend native tests/native/ownership_paths.ryn` runs and prints `0` and `0`, and `ownership_30m.ryn` builds and runs under `ulimit -v 1048576`. The double-free trap and the declined-subset reason need the native driver directly. Not ported here. |

## Summary

- Oracle-bound and to be dropped with the Rust code: `ast_text_dump.rs`, `ir_text_dump.rs`, the parity part of
  `self_hosted_parity.rs`, the cranelift half of `backend_selection.rs`, the classes of `ryn_sema_ratchet.rs`, and
  the accept sets of the IR and AST round trips.
- Keep as Ryn-only checks: the reject sets of the round trips, `self_hosted_frontend.rs`, `native_backend.rs`, the
  fixpoint of `self_hosted_parity.rs` (as a release gate), and the subset-of-sema decline status.
- Snapshot before deletion: the exact spans, help texts and suggestions of `ryn_sema_diagnostics.rs`, and the golden
  IR of `ryn_sema_subset.rs` if IR-level checks are wanted.
- Ported here: `test_diagnostic_corpus.py`, `test_recovery.py`, `test_build_native.py`.
