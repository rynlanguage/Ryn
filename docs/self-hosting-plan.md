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
- Stage 2 inventory for the `sema` port (not started). Every IR text begins with a
  fixed block of generated enums: built-in `$RynOption#...`, `$RynResult#<i>#<j>` for
  every pair of numeric types, and `$RynResult#Parse<i>String`, written by
  `src/sema.rs` around lines 950-1100. Programs that use other generic instances add
  more, through the parser's `$Ryn<Name>#<n>` names and `src/generics.rs`
  (`examples/maps.ryn` has 146 generated enums, most programs 144). Porting order:
  (1) the built-in registry, gated by the shared prefix of the corpus IR; (2) type
  checking and slot allocation for programs that use only `main`, scalar locals,
  `echo`, and arithmetic; (3) functions and calls; (4) the rest of the statement and
  expression forms, each behind the same IR-equality gate. The port is blocked on
  nothing in Ryn itself, but it is several thousand lines of Ryn and needs the
  checker's error paths reproduced too, so it spans multiple sessions.
- Stage 2, step 3 (subset, done): `selfhost/sema_subset` lowers the syntax tree to the
  textual IR for one function, `main`, with no parameters and an optional `-> i32`
  result; `i32`, `i64`, and `str` locals; integer arithmetic; string literals; `echo`
  and template prints; `let`; `=`; and compound assignment. It reproduces the
  builtin enum registry of `sema.rs` (144 entries, `TryParseU64` skipped), the slot
  layout (`str` takes two slots), the addressed and owned tables, the `Return` for a
  result expression, and the `Drop 0` that `guard::check` adds at the end of a body
  that falls through. `tests/ryn_sema_subset.rs` requires identical IR for every
  accepted program and `unsupported` for the rest. Four corpus programs are accepted:
  `hello`, `variables`, `exit_code`, `string_escapes`. Next: owned strings and drop
  placement, comparisons and `if`/`while`/`for`, calls and parameters, then the
  remaining expression forms, each behind the same gate.
- Stage 2, step 3b (done): the subset now covers several functions in source
  order, with `i32`/`i64`/`str` parameters, calls as expressions and as statements,
  and per-function locals and tables. Seven corpus programs are accepted and match
  sema byte for byte: `hello`, `variables`, `exit_code`, `string_escapes`,
  `evaluation_order`, `function_statements`, `string_abi`. The main reasons the rest
  are declined: user structs, enums, and `extend` blocks (52 programs), types the subset
  does not model (`bool`, 13), calls to standard-library functions (10), and statements
  and expressions not yet covered (8 and 7).
- Stage 2, step 3c (done): the subset gained `bool` (one `I8` local), boolean
  literals, `!`, `&&`, `||`, the six comparisons on integers, and `if`/`else`.
  Each branch becomes its own statement list, ending in `Drop 0` when it falls
  through, so an empty `else` is a single `Drop 0`, as `guard::check` produces.
  Declarations inside a branch are still declined, because they need scope handling.
  Ten corpus programs now lower to identical IR, including `conditions`.
- Stage 2, step 3d (done): the subset gained `while`, `break`, and `continue`
  (each `break`/`continue` is preceded by its `Drop 0`), integer negation, and
  block-scoped declarations (a block's names go out of scope at its end, and its
  slots are not reused). Statements after one that does not fall through are not
  lowered, as sema omits them. `-N` folds into one literal only when N alone is out of
  range for the type, which is how sema writes the smallest value of a type. Fifteen
  corpus programs now lower to identical IR.
