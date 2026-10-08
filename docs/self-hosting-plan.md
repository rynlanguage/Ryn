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
- Stage 2, step 2a (done): `selfhost/ir` is a Ryn program that checks textual IR
  against the grammar the Rust decoder accepts. It walks the same tokens in the
  same order and reports the byte offset of the first mismatch.
  `tests/ryn_ir_check.rs` builds it with the compiler under test, checks every
  pass fixture and analysable example, and requires truncated or corrupted text to
  be rejected. The Ryn checker validates the format only; it does not build an IR
  data model yet, which is the next step.
- Language gaps found while writing it: closures are not available; `choose` arms
  take expressions only; `Result::Ok` needs an expected type inside `choose`.
  `Vec` of enums was rejected (`R0234`) and is now supported, including
  `Vec<Option<T>>`, with clone/drop callbacks (`tests/vec_of_enum.rs`), so the IR
  data model can use enum node lists instead of an index arena of structs.
- Stage 2, step 2 (done): `selfhost/ir_roundtrip` is a Ryn implementation of the
  IR data model and its text decoder and encoder (`src/ir.ryn`). `decode_ir`
  consumes the text in the order `src/ir_codec.rs` writes it, and `encode_ir`
  writes it back. `tests/ryn_ir_roundtrip.rs` builds the tool with the compiler
  under test and requires every analysable pass fixture and example to round-trip
  byte for byte, and truncated, short, corrupted, and UTF-8-splitting texts to be
  rejected. Recursive nodes live in flat arenas (`Program.exprs`, `stmts`, `types`,
  `parts`, `arms`) and refer to each other by index.
  Known simplifications, to be closed before the Ryn IR feeds semantic analysis:
  operation names are kept as their text tokens, not typed enums; integer
  literals are kept as their decimal text because the IR holds `i128`; the
  decoder does not validate operation names (the Rust decoder still does).
- Language gaps found while porting the decoder: a variant cannot hold a `Vec`
  of its own enum (`R0234`), so recursive IR needs arenas; `choose` arms take
  expressions only and `choose` cannot match `String` literals; a `choose` must
  be the last expression of its function; assignment through a nested field
  path (`self.a.b = x`) is rejected on a borrowed receiver, so records are built
  as whole values instead.
- Stage 2, step 2b (done): `selfhost/ast_roundtrip` is the same kind of Ryn model for
  the syntax tree that `ryn::ast_codec` writes (`src/ast.ryn`). Declarations, types,
  statements, and expressions round-trip byte for byte, including the newlines the Rust
  encoder places between items. `|>`, `??`, and `?.` stay as their own nodes, since the
  bootstrap decoder rewrites them but the frontend's text names them; the rewrite belongs
  to the lowering stage. `tests/ryn_ast_roundtrip.rs` checks the corpus and frontend output
  with those forms. Operators and type words are kept as validated text tokens, and float
  literals as the text the encoder wrote.
  Porting note: `Result<Vec<T>>` is rejected for struct element types, so lists of
  structures live in arenas and lists are `Vec<u64>` of indices.
