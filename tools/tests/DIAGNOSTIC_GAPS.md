# Diagnostic gaps against the recorded Rust oracle

Checked by `test_diagnostic_corpus.py` (the 195 rows of `tests/fixtures/ryn_sema_coverage.tsv`, each run as
`rync check --stdlib stdlib/std/src <fixture>`), and by `test_recovery.py` and `test_build_native.py`, which port the
Rust tests that the Rust analysis used to answer.

Result at the time of writing: 194 of 195 rows report the recorded code and message (paths compared up to the path).
The old tsv classes (ACCEPT, DECLINE, WRONG_CODE, WRONG_TEXT) came from the old frontend that skipped passes; the real
compiler matches them, so those classes no longer describe the compiler.

Tests marked `GAP` fail on purpose. The assertions are not weakened.

## 1. Stale oracle row (not a compiler gap)

| Row | Recorded (Rust) | `rync check` |
| --- | --- | --- |
| `tests/suite/modules/w9_missing_path_dependency/src/main.ryn` | `R0420` `module `ghost::x` not found (looked for ... )` | `R0425` `cannot resolve path dependency `ghost` of package `w9-missing-path-dependency` at ...: No such file or directory` |

The fixture's own header says `// code: R0425` and `// msg: cannot resolve path dependency `ghost``, and the manifest
`ryn.yaml` declares `ghost: path: ./vendor/ghost`, which does not exist. The compiler is right; the recorded Rust
row came from an analyzer that never read the manifest. This row must be regenerated in the tsv (its message needs the
path cut, as the other path rows do) before the corpus test can pass. Until then the test fails on this row.

## 2. Compiler gaps: `for` range errors hide later errors

`ryn check` stops after the first error of a `for` range whose start is invalid. The Rust recovering API reports the
body and the end bound too (`tests/compiler_api.rs`, `recovering_for_range_checks_*`). Duplicate-name cases work.

| Program | Expected (Rust) | `ryn check` reports |
| --- | --- | --- |
| `fun main() { for item in missing_start..2 { echo(missing_body) } }` | `R0203` missing_start, `R0203` missing_body | only `R0203` missing_start |
| `fun main() { for item in true..2 { echo(missing_body) } }` | `R0206` found `bool`, `R0203` missing_body | only `R0206` |
| `fun main() { for item in missing_start..missing_end { echo(missing_body) } }` | `R0203` missing_start, `R0203` missing_end | only `R0203` missing_start |

Tests: `test_recovery.py`, `test_GAP_*` (three tests).

## 3. Compiler gap, data loss: output may overwrite the source through a link

`ryn build <src> -o <link>` refuses to write over the source only when the output path is the source path (`R0303`).
When the output is a symbolic link or a hard link to the source, the build succeeds and the executable overwrites the
source file. The Rust `ryn::compile_source` returned `CompileError::OutputWouldOverwriteSource` for both
(`tests/compiler_api.rs`, `source_file_compile_api_rejects_overwriting_its_source`).

Reproduction:

    printf 'fun main() {\n    echo 42\n}\n' > a.ryn
    ln -s a.ryn b.ryn          # or: ln a.ryn b.ryn
    ryn build a.ryn -o b.ryn   # exit 0; a.ryn is now an ELF executable

Tests: `test_build_native.py`, `test_GAP_build_refuses_a_symbolic_link_to_its_source` and
`test_GAP_build_refuses_a_hard_link_to_its_source`. This one should be fixed first: it destroys user source.
