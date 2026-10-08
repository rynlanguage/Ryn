# Full self-hosting plan

Goal: the Ryn compiler is a Ryn program end to end, and `ryn bootstrap`
rebuilds it from its own source without a Rust toolchain.

Today the lexer, parser, and generic specializer are written in Ryn
(`selfhost/src/frontend`). Semantic analysis (`src/sema.rs`), Ryn Guard
(`src/guard.rs`), code generation through Cranelift (`src/codegen.rs`), the
runtime (`src/runtime_shim.rs`, `src/runtime/`), the Windows linker
(`src/pe_linker.rs`), and the CLI are Rust.

Each stage below keeps the compiler working and is verified the same way the
frontend was: the Ryn implementation must produce byte-identical results to
the Rust one over the whole test suite before it becomes the default.

## Stage 1 — no Rust needed to build programs (done)

- The runtime is compiled once into a static library embedded in `ryn`.
- Windows executables are linked by `src/pe_linker.rs`; Linux uses `cc`.
- Programs and the self-hosted frontend build on machines without `rustc`.

## Stage 2 — semantic analysis in Ryn

- Port `sema.rs` to `selfhost/src/middle/sema.ryn`, producing the same typed
  IR. Define a stable textual IR encoding (like `ast_codec` for the AST) so
  the Rust backend can consume IR produced by Ryn.
- Gate: for every example and test program, IR from Ryn equals IR from Rust.
- Port Ryn Guard (`guard.rs`) the same way; diagnostics must match exactly.

## Stage 3 — a native backend in Ryn

Cranelift is a Rust library, so a self-hosted compiler needs its own code
generator:

- x86-64 instruction encoder and a simple SSA-based lowering of the typed IR
  (linear-scan register allocation is enough to start).
- COFF and ELF object writers.
- Gate: every native test passes with the Ryn backend; then performance work
  (constant folding, inlining of small functions) until it is close to
  Cranelift's `speed` level.

## Stage 4 — linker and runtime in Ryn

- Port `pe_linker.rs` to Ryn and add an ELF static linker for Linux so `cc`
  is optional.
- Rewrite the runtime in Ryn on top of Win32 and Linux system calls, removing
  the dependency on Rust's standard library.

## Stage 5 — Rust-free bootstrap

- `ryn bootstrap` builds stage 1 with the previous release, stage 2 with
  stage 1, and checks stage 2 and stage 3 are identical.
- Releases ship a compiler built by the previous release; the Rust sources
  remain only as a reference until they are removed.

## Progress log

- Stage 2, step 1 (done): `src/ir_codec.rs` writes the typed IR produced by
  `sema::analyze` as text and reads it back. Types are written by content (an
  array is its element and length, a function pointer its signature), so the
  registry indices do not leak into the text. `tests/ir_codec.rs` checks that
  every `tests/programs/pass` fixture round-trips and that at least half of the
  examples do (`encode(decode(encode(ir))) == encode(ir)`). Operation enums are
  named through tables generated from their definitions, and the encoder's
  `match` arms are exhaustive, so a new IR variant cannot be dropped silently.
- Stage 2, step 2 (not started): a Ryn implementation of the IR data types and
  the decoder, gated by equal re-encoding of the same corpus.
