# Contributing to Ryn

Thanks for helping build Ryn. This document explains how to set up a workspace, how to change the
compiler, and what a change has to satisfy before it is merged.

## Ground rules

- The repository is the source of truth. Read the code you are about to change before changing it.
- Do not remove or weaken a passing test to make a change pass. Add a regression test instead.
- Do not change language semantics by accident: a change to the lexer, parser, analysis or code
  generation that alters what programs mean needs a deliberate decision and a test that pins it.
- Keep `ryn check`, `ryn build` and `ryn run` consistent — a program that builds must first pass the
  same analysis as `check`.
- Prefer complete features over parser-only placeholders.
- Update `CHANGELOG.md`, `README.md` and `docs/` when behavior, commands or limits change.

## Environment

You need a stable Rust toolchain (edition 2024).

On Linux x86-64, building Ryn programs needs nothing else: the default native backend emits the
executable itself, without `cc`, `ld` or libc. On Windows the Cranelift backend links through the
compiler's own PE linker (`src/pe_linker.rs`), again without `link.exe` or an SDK. On other targets
the Cranelift backend uses the system C compiler driver (`cc` or `CC`), and realtime keyboard input
uses evdev on Linux and virtual-key state on Windows.

```sh
git clone https://github.com/rynlanguage/Ryn.git
cd Ryn
cargo build --release
./target/release/ryn --help
```

`rustup component add clippy rustfmt` is required for the local checks below.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/` | The Rust compiler: CLI, lexer/parser bootstrap, semantic analysis, Ryn Guard, Cranelift backend, PE/ELF linking, runtime |
| `selfhost/src/frontend` | Ryn-written lexer, parser, generic specializer |
| `selfhost/src/middle` | Ryn-written lowering to typed IR, compile-time evaluation, patterns |
| `selfhost/src/back` | Ryn-written native code generator, runtime, ELF writer |
| `selfhost/src/driver` | Ryn-written manifest and diagnostic driver |
| `selfhost/src/rync.ryn` | The native compiler entry point |
| `stdlib/std/src` | The standard library |
| `tests/suite` | The language suite; each `.ryn` file (or project directory) is one test |
| `examples/` | Runnable programs; every entry is built in CI |
| `tools/` | Bootstrap, native suite and diagnostic-coverage scripts |
| `docs/` | Language reference, memory model, self-hosting plan, validation reports |

## Building and testing

```sh
cargo check --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps

cargo test --workspace                                   # everything
cargo test --release --workspace -- --test-threads=4     # suite against optimized code
cargo test --release --test suite                        # the 628-program language suite
RUST_MIN_STACK=16777216 cargo test                       # deep compiler tests need a 16 MiB thread stack
```

`RUST_MIN_STACK` matters: several compiler tests recurse deeply and overflow the default 8 MiB test
thread stack. CI sets it for every job.

### Native compiler

```sh
./target/release/ryn bootstrap --native    # rebuild rync from its own sources; stages 2 and 3 must match
tools/bootstrap-native.sh <seed> <out-dir>  # Cargo-free bootstrap from bootstrap/rync-seed
python3 tools/test-native-sources.py <rync> tests/suite/...  # compile suite programs with rync only
```

The native compiler must reproduce itself: stage 2 and stage 3 byte-identical. `bootstrap/rync-seed`
is the committed, Cargo-free seed; replace it only through the procedure in
`docs/native-compiler.md`.

### Diagnostics work

`tools/ryn-diagnostic-coverage.sh` compares the Ryn analysis with the Rust analysis on every invalid
program of the corpus and writes `tests/fixtures/ryn_sema_coverage.tsv`;
`tests/ryn_sema_ratchet.rs` fails when an outcome changes, so every agreement change is deliberate.

### Benchmarks

```sh
cargo bench --bench compiler_pipeline
```

Benchmarks report medians on the machine that ran them; they are not performance guarantees.

## Code style

- Rust: `cargo fmt` defaults, propagate errors with `?` instead of `unwrap()`/`expect()` on paths
  reachable from user input, and no `unsafe` without a comment explaining the invariant.
- Ryn (`selfhost/`, `stdlib/`): follow the surrounding files. Public items carry a `//` comment that
  explains what they guarantee, not what they do line by line.
- Diagnostics: keep the `Rxxxx` code stable, write the message in the compiler's voice, and add a
  `help` line when the fix is not obvious. A new rule needs tests for both the accepted and the
  rejected program.
- Comments explain why. Delete commented-out code.

## Adding language features

A new syntax or semantic feature needs, in order:

1. A decision recorded in the issue (what it means, what it rejects, what it costs).
2. Implementation in the Rust bootstrap path and, where applicable, the Ryn front end, keeping both
   consistent — `tests/self_hosted_parity.rs` compares their trees.
3. Tests: at least one positive and one negative program in `tests/suite/<category>/`, plus an
   example in `examples/` when the feature is user-visible.
4. Documentation: `docs/language-0.2.md` for semantics, `README.md` for a one-line mention,
   `CHANGELOG.md` for the entry.

Use the [Language Proposal](.github/ISSUE_TEMPLATE/language_proposal.md) issue template for step 1.

## Reporting bugs

Open an issue with the [Bug Report](.github/ISSUE_TEMPLATE/bug_report.md) template. A minimal
reproducing program, the exact command, the observed and expected output, and the compiler version
(`ryn --version`) are enough to start. Security-sensitive bugs follow [SECURITY.md](SECURITY.md)
instead of a public issue.

## Pull requests

- One logical change per PR. Keep the diff reviewable; say in the description what the change costs.
- Fill in the [pull request template](.github/PULL_REQUEST_TEMPLATE.md).
- The PR must pass CI: formatting, Clippy with `-D warnings`, `cargo doc`, both test jobs, and the
  native bootstrap job on Linux.
- Describe behavior changes in `CHANGELOG.md` under the unreleased section.
- PRs are merged with a merge commit; keep the branch history readable.

## Commit messages

Use an imperative subject that names the change, under about 72 characters, followed by a body that
explains the motivation, the approach and the verification. Reference issues (`Fixes #123`) in the
body. Example:

```
Clone nested enum payloads in the native backend

An enum with a multi-cell payload was copied shallowly, so the original and the
copy shared owners and both were freed. Describe the payload cells when cloning
and cover it with tests/native/nested_result_ownership.ryn.
```

## License

Contributions are accepted under the [Mozilla Public License 2.0](LICENSE). Files you add should carry
the license notice used by the files around them. By contributing you agree that your work is
distributed under that license.
