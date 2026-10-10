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

You need a Linux x86-64 machine, `python3` (for the test runners), `tar`, and `sha256sum`. The compiler is built by the
compiler: the committed seed `bootstrap/rync-seed` compiles the sources, and the result must compile itself to the same
bytes. No Rust toolchain, C compiler, linker or C library is used.

```sh
git clone https://github.com/rynlanguage/Ryn.git
cd Ryn
tools/bootstrap-native.sh "$PWD/bootstrap/rync-seed" /tmp/ryn-bootstrap      # stage 2 must equal stage 3
```

`docker`/`podman` with the `ryn-wine` image (Wine on Debian) is only needed to run Windows programs; the Windows target
is cross-compiled on Linux.

## Repository layout

| Path | Contents |
| --- | --- |
| `selfhost/src/ryn.ryn` | The `ryn` command (new, check, build, run, clean, lock, login, publish) |
| `selfhost/src/rync.ryn` | The native compiler entry point |
| `selfhost/src/frontend` | Lexer, parser, generic specializer |
| `selfhost/src/middle` | Lowering to typed IR, analysis and diagnostics, compile-time evaluation, patterns |
| `selfhost/src/back` | Code generator, runtime, ELF and PE writers, the Windows runtime |
| `selfhost/src/driver` | Manifest, packages and registry, SHA-256, diagnostic rendering |
| `stdlib/std/src` | The standard library (`native.ryn` is the Linux runtime, `native_windows.ryn` the Windows differences) |
| `bootstrap/` | The committed seed compiler and its checksum |
| `tests/suite` | The language suite; each `.ryn` file (or project directory) is one test |
| `tests/programs`, `tests/fixtures`, `tests/native` | Programs and recorded diagnostics used by the tests |
| `examples/` | Runnable programs |
| `tools/` | Bootstrap, distribution, suite listing and the Python test runners (`tools/tests`) |
| `docs/` | Language reference, memory model, self-hosting plan, validation reports |

## Building and testing

```sh
tools/bootstrap-native.sh "$PWD/bootstrap/rync-seed" /tmp/ryn-bootstrap          # the compiler: rync-stage2
python3 tools/test-native-sources.py /tmp/ryn-bootstrap/rync-stage2 $(python3 tools/list-suite.py)   # the language suite
python3 tools/make-dist.py /tmp/ryn-bootstrap/rync-stage2 "$(cat VERSION)" /tmp/ryn-dist             # the archives
python3 tools/run-tests.py /tmp/ryn-dist/.../ryn                                  # ryn command and runtime tests
```

`tools/test-native-sources.py` reads each program's header (`// test: run` with `// out:`, `// err:`, `// exit:`, or
`// test: fail` with `// code:`, `// msg:`). Set `RYN_TEST_TARGET=windows` to cross-compile the programs and run them under
Wine (`RYN_TEST_WINE=local` uses a local `wine`; the default is the `ryn-wine` container).

### Changing the compiler

The seed can only compile sources it supports. When a change needs a feature the seed lacks (or changes the code the
compiler generates for itself), the new compiler is built in two steps: the old seed compiles the new sources without
using the feature, then that compiler compiles the full sources. The result of `tools/bootstrap-native.sh` must have
stage 2 equal stage 3; copy that stage 2 to `bootstrap/rync-seed` and update `bootstrap/rync-seed.sha256` in the same
change. The Rust implementation that used to bootstrap the first seed is in the git history before its removal.

### Diagnostics work

`tests/fixtures/ryn_sema_coverage.tsv` records, for each invalid program of the corpus, the code and message that the
original (Rust) analyzer produced; `tools/tests/test_diagnostic_corpus.py` requires `rync check` to give the same.

## Code style

- Ryn (`selfhost/`, `stdlib/`): follow the surrounding files. Public items carry a `//` comment that
  explains what they guarantee, not what they do line by line.
- Python (`tools/`): standard library only.

## Reporting bugs

Open an issue with the [Bug Report](.github/ISSUE_TEMPLATE/bug_report.md) template. A minimal
reproducing program, the exact command, the observed and expected output, and the compiler version
(`ryn --version`) are enough to start. Security-sensitive bugs follow [SECURITY.md](SECURITY.md)
instead of a public issue.

## Pull requests

- One logical change per PR. Keep the diff reviewable; say in the description what the change costs.
- Fill in the [pull request template](.github/PULL_REQUEST_TEMPLATE.md).
- The PR must pass CI: the bootstrap fixpoint, the language suite, the command-line tests and the Windows jobs.
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
