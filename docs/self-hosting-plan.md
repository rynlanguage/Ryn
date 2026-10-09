# Full self-hosting plan

Native ownership validation (2026-10-09): deterministic cleanup without GC now passes the 1 GiB native bootstrap and the stage2/stage3 binary fixpoint. See [native ownership validation](native-ownership-validation.md) for exact gates and memory measurements.

Goal: the Ryn compiler is a Ryn program end to end, and `ryn bootstrap`
rebuilds it from its own source without a Rust toolchain.

Today the lexer, parser, and generic specializer (`selfhost/src/frontend`) and
the lowering of valid programs to typed IR (`selfhost/src/middle`, which does
the work of `src/sema.rs` and the ownership part of `src/guard.rs`) are written
in Ryn. What is still Rust: the diagnostics for invalid programs, Ryn Guard's
checks (`src/guard.rs`), code generation through Cranelift (`src/codegen.rs`), the
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
- Stage 2, step 3e (verified by hand, no corpus program yet): `for name in a..b` and
  `a..=b` over `i32` and `i64`. The loop variable and a hidden end slot are allocated
  after the bounds are lowered, the variable is in scope only for the body, and
  `break`/`continue` follow the `while` rules. A scratch program with inclusive and
  exclusive ranges, `continue`, and negative bounds lowered identically to `sema`; the
  scratch file was removed. The corpus programs that use `for` also use `u8`, which the
  subset does not model yet, so the gate count is unchanged.
- Stage 2, step 3f (done): the subset gained the sized integer types `i8`, `i16`,
  `u8`, `u16`, `u32`, and `u64` next to `i32` and `i64`. Each takes the local type
  `sema` gives it (`U8` is an `I8` slot, `U32` an `I32` slot, and so on). Negation is
  defined for signed types only, and a literal folds only when its magnitude is out of
  range for a signed type. Nineteen corpus programs now lower to identical IR.
- Stage 2, step 3g (done): integer casts `value as T` between integer types (a bare
  literal takes the target type as its source, as the corpus shows), and the two
  command-line builtins `arg_count()` and `arg(index)`, which are used only when no
  user function has the same name. Twenty corpus programs now lower to identical IR.
  Bitwise operators (`&`, `|`, `^`, `~`, shifts and their compound forms) are next,
  but only one corpus program uses them, and it also needs `~` and mixed-type shifts.
- Stage 2, step 4 (verified by hand, no corpus program yet): user structures with
  scalar, `str`, and nested structure fields. The IR structure section is written
  (field offsets and slot counts follow the field order, and a structure's slots are
  the sum of its fields'), literal values list fields in the order written, field
  reads and writes address the field's slots, and a compound field assignment's
  target span covers `object.field...`. Structures are also parameters and results.
  A scratch program with all of these lowered identically to `sema`; the scratch file
  was removed. The corpus structure programs still use `when` as a value, enums,
  `extend`, attributes, or collections, so the gate count is unchanged.
- Stage 2, step 4b (done): `when` as a value (`If` expressions, including `else when`
  chains), typed by the expected type, else by its branches, else `i64`. Twenty-two
  corpus programs now lower to identical IR; the two new ones are the structure
  program and one with a `when` value.
- Stage 2, step 4c (verified by hand, no corpus program yet): non-generic type
  aliases, resolved through one table of names and type words that also holds the
  structures. A scratch program using an alias for a structure and for a scalar
  lowered identically to `sema`. Corpus programs with aliases also use pointers,
  function pointers, or FFI types, so the gate count is unchanged.
- Stage 2, step 5 (verified by hand, no corpus program yet): owned `String` locals,
  parameters, and temporaries. `String("...")` lowers to `Call String New` and yields an
  owned value; each block, function, `break`, and `continue` drops its owned strings,
  last declared first; a named owned string used by value (call argument, `let`,
  assignment, `return`, structure field) is a `Move` out of its slot, while printing
  borrows it. Two scratch programs (a loop with `break`, a block, a function that takes
  an owned argument, and a moved local) lowered identically to `sema`. The corpus
  programs with owned strings also use `defer` or `String` methods, so the gate count is
  unchanged.
- Stage 2, step 6 (done): `defer`. A `defer` registers its body with the locals it
  saw and leaves the placeholder `Block 1 Drop 0` that the guard completes. A block runs
  its own defers after its statements, and a `return` computes its value into a fresh
  slot before the pending defers (last registered first) run, then returns it. `break`
  and `continue` run the defers of the loop's frames inside a block with their drop. A
  function's own defers run after its body, or after the result expression through a
  `let`. Defers that follow a body that does not fall through are dropped, as the guard
  drops unreachable statements. Two corpus programs now lower to identical IR (24 in
  total), and a scratch program with `break`, `continue`, defers in a branch, and a tail
  result matched sema.
- Stage 2, step 7 (verified by hand, no corpus program yet): user enums without
  generics. Enum definitions come first in the IR's enum section, before the builtin
  registry, with their variants and field types. A value of an enum is an owned slot
  (`OwnedPtr` owned by `Enum <id>`), and `Enum::Variant(args)` lowers to `EnumNew` with
  the variant's tag, its arguments (moved when they are owned), and the enum's type;
  a bare `Enum::Variant` is the same with no arguments. Enum locals are dropped and
  moved like owned strings. Pattern matching (`choose`) comes next. Two scratch programs
  with unit and tuple variants, string payloads, printing, and a move matched sema.
- Stage 2, step 8 (done): `choose` over user enums. Each arm's bindings get fresh slots
  in arm order and are visible only in that arm's body; `_` is the default arm; the
  first arm gives the result type when the context does not. Calls by name that Rust
  treats as constructors and builtins also get their result types from `infer`. The
  example `enum_formatting` (unit, tuple, and string-payload variants, printing,
  templates, `choose` as a value) matches sema byte for byte, and one more corpus
  program is accepted (25 in total).
- Stage 2, step 9 (verified by hand, no corpus program yet): `#[repr(C)]` structures
  (the record's flag), and structures with owned fields (strings, enums, nested
  structures with them). A local of such a structure is one owned part per owned field,
  and its drop lists the parts last first; a field used by value is moved out of its
  slot, and a whole structure that owns storage is moved as a unit. Structures with
  `derive` are declined for now, because the IR also writes the functions that derive
  generates; that needs its own step.

- Stage 2, steps 3h-12 (done): the Ryn lowering now covers the whole language for valid
  programs. It handles floats, chars, all operators, builtin functions (from a table generated
  from `sema.rs`), `String`/`Vec`/`Map`/`Set` methods, generic `Option`/`Result` (construct,
  `choose`, `?`, `??`, `?.`, `|>`), `extend` blocks and operator methods, references and raw
  pointers, function pointers, `extern "C"` declarations, arrays, slices, tuples, ranges,
  `for` over collections, `#[derive(Clone)]`, destructors (`#[drop]`), `shape` default methods,
  namespaces and `use` module aliases. `selfhost/src/middle/lower.ryn` is the lowering and
  `selfhost/src/middle/ast.ryn` the syntax tree model; both are part of the frontend executable,
  which gained the modes `--sema` and `--ast-roundtrip`.
  Gates: `tests/ryn_sema_subset.rs` requires identical IR for every one of the 103 analysable
  corpus programs; `tests/ryn_sema_projects.rs` requires identical IR for the compiler's own
  projects (the frontend with the lowering itself, 790 KB of IR). Both pass.
- IR format fix: call targets for `Vec`, `Map`, `Set`, `VecSlice`, `VecGetOption`,
  `ArrayAsSlice`, and `IndirectFunctionPointer` used to write an index into a process-wide type
  registry, so the text depended on the order in which a process met types and no other
  implementation could read it. They now write the types themselves.
- Integration: `Frontend::analyze` runs the Ryn lowering and the bootstrap analyzer on the same
  program. The bootstrap supplies diagnostics for invalid programs; if both produce IR the texts
  must be equal, otherwise compilation stops with R0904. A program the Ryn lowering declines
  (status 3) is analyzed by the bootstrap alone. `RYN_SEMA=rust` skips the Ryn lowering. `ryn
  bootstrap` reaches its fixpoint with the lowering inside the frontend.
- Stage 2, what is left: the Ryn analysis assumes a valid program. Reproducing every diagnostic
  of `sema.rs` (about 300 sites) and of `guard.rs`, with identical codes, messages, spans, and
  help, is what stands between this and dropping the bootstrap analyzer. The `tests/programs/fail`
  corpus and `tests/diagnostics.rs` are the gate for it.

- 0.2 work (see `docs/language-0.2.md`): the Rust passes `comptime.rs` and `patterns.rs` run before the
  Ryn lowering, so the lowering sees only the rewritten program and needs no change for constants,
  literal patterns, or `print`. The parsers and the generic specializer were extended in both
  languages and compared by `tests/self_hosted_parity.rs` on every program of the 518-test
  suite. `ryn bootstrap` still reaches its fixpoint. Still not self-hosted: Ryn diagnostics,
  the native backend, the linker and runtime, and the CLI. The bootstrap Rust compiler remains
  required; the project does not yet compile itself without Rust.
  `Set<T>::clone()` typing was fixed in `src/sema.rs` (it was typed as a `Map`); regression test
  `collections/set_clone_keeps_set_type`.

- Stage 2, diagnostics, step 1 (done): the Ryn lowering now reports `R0203` (unknown variable,
  including the typo suggestion, the bare `None`/`Some`/`Ok`/`Err` hint, and the `id<i32>(1)` hint)
  itself. `ryn-frontend --sema` writes `diagnostic <code> <start> <end> <message> <help>` and
  `src/frontend.rs` checks that the bootstrap analyzer reports the same code, message, span, and
  help. With `RYN_SEMA_STRICT=1` a difference is an `R0904` error; without it the bootstrap's
  diagnostics stand. `tests/ryn_sema_diagnostics.rs` runs the whole invalid-program corpus in
  strict mode; it already found one difference (the `id<i32>(1)` hint) while this step was built.
  About 300 diagnostic sites of `sema.rs` and the `guard.rs` diagnostics remain; each one is added
  the same way, with the corpus as the gate. The bootstrap analyzer still decides every result.
- Stage 2, diagnostics, step 2 (done): the Ryn analysis reports `R0204` (`` `x` is immutable ``) for
  plain, compound, element, and field assignments to a binding declared without `mut`, with the
  bootstrap's span, message, and help. A field assignment through a raw pointer is left to the
  bootstrap (`R0224` or the reference path). Statement and function wrappers used to append
  ` @ <function> <offset>` to every refusal, including a diagnostic's own text; a `diagnostic `
  message now passes through unchanged. `tests/ryn_sema_diagnostics.rs` runs the Ryn analysis
  alone and requires its output to equal the bootstrap's diagnostic text byte for byte. The
  strict gate also covers `tests/programs/pass` and `examples`, so a diagnostic on a valid program
  is caught as well as a disagreement on an invalid one.
  Next family: `R0211` (wrong argument count for a call). Rust checks `R0425` (private function
  of another module) first, and the Ryn signature table has no visibility yet, so that needs
  the function's `pub` flag and module in `Signature` before the arity error can be reproduced.
- Stage 2, diagnostics, step 3 (done): `R0211` (wrong argument count for a call to a function of
  this program) and `R0212` (argument of the wrong type, for scalar and string types only). `Signature`
  records the function's `public` flag and module, so a private function of another module is
  declined (the bootstrap reports `R0425` first). `R0212` is reported at the argument's span; an
  argument that the expression lowering refuses with `... where another type is expected` is lowered
  once more without an expected type, and its own type decides. Structs, collections, and pointers
  are declined, because their spelling is not written in Ryn yet. A `bool` literal passed where an
  integer is expected is still declined by the expression lowering (`boolean where another type is
  expected`), so the bootstrap reports it.
  Next family: the rest of `R0212` (structure and collection spellings), which needs the same
  type-name formatter over IR type words.
- Stage 2, diagnostics, step 4 (done): `R0205` for an annotated `let` whose initializer has another
  type (`declared type ... does not match initializer type ...`), at the initializer's span. The
  redeclared-name check now runs before the initializer, as in the bootstrap (`R0202` comes first).
  The shared probe (`other_type_of`) serves both `R0212` and `R0205`. Scalar and string types only.
- Stage 2, diagnostics, step 5 (done): `R0206` for operators applied to scalar operands of the wrong
  types: `%` on non-integers, arithmetic, bitwise operators, shifts (the count must be `u32`), `==`/`!=`,
  and ordering. The spans and help are the bootstrap's (`operator_mismatch`). A right operand that
  the expression lowering refuses against the operator's context is probed for its own type. A
  compound assignment to a name uses the bootstrap's span (name to end of value); a compound
  assignment to a field is declined, because its span is not written here. Logical operators
  (`&&`, `||`) and unary operators are still declined. The bootstrap's `R0206` for structures and
  collections is not reproduced.
- Stage 2, diagnostics, step 6 (done): `R0202` for a `let` whose name is already declared in the scope,
  including a name declared again in a nested block, at the statement's span. The `for` loop variants
  of `R0202` have their own spans and help and are not yet ported.
- Stage 2, diagnostics, step 7 (done): `R0206` for the unary operators: `-` on an unsigned value (or a
  literal in an unsigned context), `!` on a non-`bool` value, and `~` on a non-integer value, each at
  the operation's span. Operands the expression lowering refuses against the operator's context are
  probed for their own type, as for the binary operators. An integer literal in a non-integer context
  is now typed `I64` (range-checked against `i64`), as the bootstrap types it, so the caller reports
  the mismatch instead of the literal being declined.
- Stage 2, diagnostics, step 8 (done): `R0206` for `&&` and `||` with an operand that is not `bool`, at
  the first such operand's span. Operands that the expression lowering refuses are probed as for the
  other operators. An operation with a bare numeric literal (or a negation or `~` of one) as an operand
  is declined, because the bootstrap derives that literal's type from the other operand's hint, which
  the logical path does not reproduce.
- Stage 2, diagnostics, step 9 (done): range loops (`for name in a..b`). `R0202` when the loop variable
  is already declared, `R0206` when a bound is not an integer (`found `f64``) and when the two bounds
  have different integer types, all at the loop variable's span, as the bootstrap reports them. The
  literal start's retyping to the end's type is unchanged. The `for element in ...` form (`R0202` for its
  variable and `R0206` for a collection that is not a `Vec`, `String`, `str`, or range) is not yet
  ported.
- Stage 2, diagnostics, step 10 (done): `for element in collection`. `R0202` when the loop variable is
  already declared (collection wording), and `R0206` for a scalar collection that is not iterable, at the
  collection's span. Non-scalar, non-iterable collections are still declined.
- Stage 2, diagnostics, step 11 (done): `R0233` for an enum variant built with the wrong number of field
  values (`Figure::Circle(1.0, 2.0)` for a one-field variant), at the call's span. The check is reported
  only when every field of the variant is a scalar or string, which cannot hold a destructor, so the
  bootstrap's earlier `R0255` check cannot apply; other variants are declined. The span reaches the enum
  constructor through `call`, and the pipe forms (`x |> f`) have no call span and are declined.
- Stage 2, diagnostics, step 12 (done): the `defer` rules and `break`/`continue` outside a loop.
  `R0267` for `return` in a `defer` body, `R0268` for `break`/`continue` in a `defer` body inside a loop,
  `R0269` for `defer` in a function that returns a reference, and `R0016`/`R0017` for `break`/`continue`
  outside any loop. A counter (`defer_depth`) is raised while a deferred body is lowered. Known
  difference: the bootstrap lowers a `defer` body where it is written, so its own diagnostics appear at
  the `defer`; Ryn lowers it where the defers run, so a diagnostic inside a `defer` body can come after
  a later statement's. That ordering is not yet reproduced.
- Stage 2, diagnostics, step 13 (done): `R0225` for a field that a local structure value does not declare,
  reads and `let` initializers alike, at the field's name span, with the bootstrap's did-you-mean or
  "use a field declared by this structure" help. Destructuring is desugared by the parser into such
  reads, so `Point { x, z } := point` is covered. Tuple destructuring (`(a, b, c) := pair`) reports the
  bootstrap's `R0225` through a `$RynTuple#...` name, which is declined here. Values reached through a
  reference or a field chain are declined.
- Stage 2, diagnostics, step 14 (done): `R0233` for an enum that has no variant of the written name, both
  in a construction (`Mode::Execute`, at the expression) and in a `choose` arm (at the arm), with the bootstrap's
  help that lists the variants in declaration order. The arm form applies only when the arm's written enum
  is the scrutinee's enum. The built-in `Option` and `Result` are declined. (The bootstrap's `R0233` code
  is shared by the field-count and unknown-variant cases; the corpus names them under `R0233` too.)
- Stage 2, diagnostics, step 15 (done): structure literals. Each written field is checked in order, as the
  bootstrap does: `R0225` for an unknown field, `R0228` for a field written twice, and `R0205` for a value
  of another scalar or string type. Then the first left-out field is reported as `R0229` at the literal's
  span. The literal's field count is no longer a precondition, so a literal with missing fields reaches the
  check. Tuple literals and non-scalar mismatches are declined.
- Stage 2, diagnostics, step 16 (done): `R0201` for a function whose signature key (its name, with `#method`
  for a method) an earlier function already has, at the later function's span. The check runs before the
  function's parameters are read, as the bootstrap's signature loop does.
- Stage 2, diagnostics, step 17 (done): `R0213` for a function with a scalar or string result type whose body
  can end without returning a value, at the function's span. The fall-through analysis that already decided
  this (declining with `function must return a value`) now reports the bootstrap's diagnostic; the bootstrap's
  rule (a top-level `return`, or an `if` whose both branches return) agrees with it for these bodies.
- Stage 2, diagnostics, step 18 (done): `R0214` for a function without a `->` result type that has a result
  expression (`fun f(x: i32) => x * 2`), at the function's span, with the bootstrap's help to add a result
  type or remove the expression.
- Stage 2, diagnostics, step 19 (done): array repeats `[value; n]`. `R0240` when the length differs from the
  expected array's, `R0205` when the element is of another scalar or string type than the array's, when the
  expected type is a scalar, and `R0256` for a `String` or enum element, which cannot be copied. The checks
  follow the bootstrap's order: length, then element type, then copyability. Repeats of collections are
  declined; repeats of structs keep their current lowering.
- Stage 2, diagnostics, step 20 (done): `R0235` for a `choose` over an enum that leaves variants out and has
  no `_` arm, at the `choose`'s span, listing the missing variants in declaration order. Before this, the Ryn
  lowering accepted a non-exhaustive `choose`; now it reports it. The built-in `Option` and `Result` are
  declined.
- Stage 2, diagnostics, step 21 (done): `R0234` for a `choose`, or a `??` (which the bootstrap desugars into a
  `choose`), over a scalar or string value, at the expression's span. A tuple or structure pattern over a
  scalar is a shape probe, which the bootstrap reports first as `R0235`; such `choose` expressions are declined.
  The corpus gate found this ordering before it landed.
- Stage 2, diagnostics, step 22 (done): `R0230` for a path written as an expression (`Mode::Read`, `util::Missing`)
  whose first part names no enum. A lowercase first part, or functions or structures declared under it, is a
  module (`unknown item ... in module ...`); otherwise `unknown enum`. A call to such a path (`util::f(1)`) is a
  function call to the bootstrap (`R0210`), so it stays declined.
- Stage 2, diagnostics, step 23 (done): `R0208` for a `main` that takes parameters or returns a type other than
  `i32`, in the signature pass, and `R0013` for `return VALUE` in a function without a `->` result type, at the
  `return`'s span.
- Stage 2, diagnostics, step 24 (done): `R0222` for a structure field whose name an earlier field of the same
  structure already has, at the repeated field's span, checked before the field's type is resolved, as the
  bootstrap does.
- Stage 2, diagnostics, step 25 (done): `R0232` for a structure that contains itself by value. The struct loop
  records each by-value structure field (its target and span), and an iterative depth-first search over those
  edges reports the first field that reaches a structure still in progress, in the bootstrap's order (structures
  in declaration order, fields in order). Array and collection fields are not edges here.
- Stage 2, diagnostics, step 26 (done): `R0248` for a reference result (a `return` of a reference type) that is
  not derived from a reference parameter (directly, or as an element of a slice parameter), at the returned
  expression's span. The check is the bootstrap's `validate_reference_return`; `if` results are declined, since
  the bootstrap checks each branch.
- Stage 2, diagnostics, step 27 (done): `R0248` for a raw pointer result that is the address of a local
  (`return &raw mut x` in a function returning `*T`), at the returned expression's span. `if` results are
  declined. The `guard.rs` family is still open.
- Stage 2, diagnostics, step 28 (done): `R0247` for an `extern "C"` function whose parameter the C ABI never
  passes (a `String`, enum, collection, reference, array, or slice; scalars, `Char`, and raw pointers are fine),
  or whose result is not a scalar or raw pointer (`Char` included), and for an `extern "C"` `main`. Struct
  records and callbacks depend on their layout, so a signature with one is declined; the bootstrap's record
  layout rules (one-field scalars; 1, 2, 4, or 8-byte packed records) are not reproduced yet.
- Stage 2, ownership, step 29 (done, first `guard.rs` diagnostic): `R0240` (guard's "use of a moved value"),
  which is a different rule from `sema.rs`'s `R0240` (array length). An owned local moved by a top-level
  statement of a function body (a call argument, a `let` value, an assignment value) is moved from the next
  statement on; a later use of that name, anywhere in the body, is the error, at the use's span. A `let` or an
  assignment of the name makes it hold a new value again, including in the same statement (`name = f(name)`),
  and a choose arm's bindings shadow it. Moves inside a loop, a choose arm, or a choose scrutinee are not
  counted. The selfhost project (`ryn_sema_projects`) caught the `name = f(name)` case; it is pinned by
  `tests/programs/pass/owned_reassign_after_move.ryn`.
  Design note for the rest of the guard: the IR that Ryn lowering produces is already post-guard, and a
  consuming use is written `Move slot ty` with no span, while the guard reports a use at the span of the
  `Local` it replaces. So a guard port cannot run over the IR alone for `R0240`/`R0241`; the flow state has to
  be kept during lowering, where the spans are known. That means merging the moved set at `when`/`choose`
  (the guard's `intersect`) and checking the loop backedge (`R0241`, the guard's `backedge`), with the
  `Drop` statements clearing the slots of a scope (the guard's `drops`).
  Done in this step: each statement commits its moves at its end (`commit_moves`), so a move is visible in the
  rest of its block and in what follows it. A `when` lowers its branches from the same state, and the state
  after it is the union of the branches that fall through (`merge_branch_moves`). A loop body that moves an
  owned value declared outside the loop, when it falls through, is `R0241` at that value's last move
  (`loop_move_error`); afterwards the state is restored. The moves of a loop header (a condition or a
  collection) happen before the loop.
  Also done: a moved local used as a method receiver is `R0240` at the receiver's name (`receiver_name`), so
  `for x in v` followed by `v.len()` is reported as the bootstrap does. `choose` arms start from the moves before
  the `choose`, and the state after it is the union of the arms (`union_moves`), as for `when`. A use is moved
  when the move is earlier in the same statement too (`is_moved` checks the pending moves), which is how
  `consume(s) + consume(s)` in one arm is reported.
  `break` and `continue` record the moved state at the jump. A `continue` state takes part in the loop's
  backedge check (`R0241`, at the last move in the body), and a `break` state is the state after the loop (the
  moves that reach the loop's exit). Every move record carries the local's slot, so a shadowed name that went out
  of scope is not mistaken for the outer local (`is_moved`, `visible_moves`). Still open: the drops of a block's
  scope, and the R0240 forms that need borrows. This is a subset of the guard's flow analysis; the rest (`R0242`, `R0249`–`R0252`, `R0255`)
  is still open.
- Stage 2, diagnostics, step 35 (done): `R0249` for a use, a move or an assignment of a local while a reference to
  it is in scope (`&mut` for uses, any reference for moves and assignments), with the reference in scope until the
  block that declares it ends. `R0425` for a call to a function that its module does not make public (a
  `use`-aliased private function is not resolved, and is declined).
- Stage 2, diagnostics, step 36 (done): `R0206` for `&mut` of a binding that is not `mut` (at the name), for an address
  of anything other than a local or a borrowed slice element (at the `&`), and for an assignment through a shared
  reference (with the `&mut value` hint). `R0203` for a name that is no local, with the bootstrap's hint; `null` is
  declined. A bare variant used as a value (`None`) is `R0203`, and one called as a function (`Some(3)`) is `R0210`.
  `R0204` for a call of a `mut self` method on an immutable local, and for `append`, `push` or `clear` on an owned
  `String` that is not `mut`. A by-value parameter is a mutable receiver, as in the bootstrap, so it is not reported
  (`tests/programs/pass/mut_self_by_value_parameter.ryn`). Each shape has an exact-text case in
  `tests/ryn_sema_diagnostics.rs`.
- Stage 2, diagnostics, step 37 (done): `R0240` for a value read through a reference and used by value (`return *r`,
  `eat(*r)`), at the reference; the scrutinee of a `choose` through a reference is matched, not moved, so only an
  owned payload bound by an arm is reported (`cannot move an owned payload out of a reference`, at the reference,
  after the arms' own checks). `R0240` for a borrow (`&s`) of a value an earlier statement moved, at the whole `&`
  expression, and for a raw pointer used after the value it points at was moved out (reassigning the owner clears
  it). `R0249` for two reference arguments of one call that name the same local when either one is mutable, at the
  later `&` expression. Still open: the same conflict through a reference local, and the `R0249` that compares a
  new borrow with the references still in scope.
- Stage 2, diagnostics, step 38 (done): the built-in output calls `print`, `eprint` and `write` are checked before
  they are declined (their lowering is not written here): the first argument must be a string literal (`R0210`, with
  the bootstrap's example in the help), a brace that is neither `{}`, `{{` nor `}}` is `R0014`, and the `{}`
  placeholders must match the values (`R0208`). A user function with one of these names takes their place, as in the
  bootstrap. `R0257` for a literal index that an array has no element for, at the indexing expression. `R0221` for a
  structure that declares no field, at the structure.
- Stage 2, diagnostics, step 39 (done): `R0255` for a call of a structure's destructor (`#[drop(name)]`), as a
  statement or an expression, at the call; the destructor runs by itself. `R0203` for `null` where the context does
  not expect a raw pointer type, at the `null` (with the bootstrap's help); a `null` where a raw pointer is expected is
  still declined, since its zero-address lowering is not written here.
- Stage 2, diagnostics, step 40 (done): `R0234` for `?` on a value that is not an `Option` or `Result` (a scalar is
  reported; other values are still declined), for `?` in a function that does not return an enum, and for `?` in a
  function that returns the other family (`Option` in `Result`, or the reverse), each at the `?` expression. `R0207`
  for a `when` statement or expression whose condition is a scalar, at the condition (the expression form has its own
  message, as in the bootstrap).
- Stage 2, diagnostics, step 41 (done): `R0426` for a field read or moved by value from another module when the field
  is not `pub`, at the field's name. `R0425` for a method that its `extend` block does not mark `pub`, called from
  another module, at the call. Cross-module method calls had never resolved in the Ryn lowering, because a method of a
  structure in a module is named with the module (`util::P::hidden#method`); the lookup now uses that name, and falls
  back to the bare name for a generated method such as the `clone` of `#[derive(Clone)]`. So valid cross-module calls
  are lowered too: the projects gate now compares the whole `selfhost` project's IR with the bootstrap's, and the
  valid program runs. Field assignments and static `Type::method` paths are still open.
- Stage 2, diagnostics, step 42 (done): reference and raw-pointer provenance through fields and structures. `R0251`
  for a reference stored into a field of a structure that lives in a shallower block, at the stored value; `R0252` for
  a raw pointer stored into such a field (also for a direct `&raw`, as the bootstrap does). A structure local now
  carries the locals its raw pointer fields point at, so `R0252` is reported when it is assigned to a shallower
  structure, and `R0248` when a structure result holds the address of a local (at the first field's value). An `if`
  joins the origins of its branches: a raw pointer keeps the deeper source, and a reference keeps a source only when
  both branches agree. `R0250` for a reference bound from an element of a slice whose base is not a name.
- Stage 2, diagnostics, step 43 (done): `R0240` for a borrowed slice bound to a name. `R0255` for a custom destructor
  whose structure is `repr(C)`, whose first field is not a raw pointer, or which has a field that is not a scalar, a
  raw pointer or `String` (at the structure).
- Stage 2, diagnostics, step 44 (done): `R0426` for a field assigned through a `&mut` reference (`p.secret = 3` where
  `p: &mut util::P`), at the field's name, when the structure is from another module and the field is not `pub`.
  Finding, not changed: a plain assignment to a field of a local (`p.secret = 3` after `mut p := util::make()`) is
  accepted by the bootstrap across modules, as is a struct literal that sets a private field. Only reads and writes
  through a reference are checked, so Ryn accepts the same programs. `docs/language-0.2.md` documents the read rule
  only. Deciding whether plain writes should be `R0426` is a language decision, left open.
- Stage 2, diagnostics, step 45 (done): `R0210` for a call of `module::item` where `module` is a module of the program
  and no function of that name exists in any module (with the closest function name as the hint); `R0227` for a struct
  literal whose qualified name is unknown, or whose structure of another module is not `pub` (`StructInfo.public`).
  `R0425` for a private function reached through a module alias (`use lib::util` then `util::hidden()`), which
  is registered under the alias as the public ones are, and reported only when the caller is in another module.
  Enum variants written with a module path (`lexer::Kind::Number(...)`) are not functions: the last segment of the
  module must be lowercase.
- Stage 2, diagnostics, step 46 (done): the pattern checks of the bootstrap's pattern pass (`src/patterns.rs`) for the
  `choose` arms that the syntax tree keeps as they are written: wildcards, literals, bare names and variants. A new
  Ryn mode, `--patterns` (`selfhost/src/middle/patterns.ryn`), reads the tree before `patterns::desugar` rewrites it,
  and `Frontend::analyze` runs it first; a diagnostic it gives must be the bootstrap's pattern diagnostic (code, span,
  message and help), or it is `R0904` under `RYN_SEMA_STRICT`. The checks follow the branch the pattern pass takes on
  the first arm that is not a wildcard: a leading wildcard makes every later arm unreachable; a leading literal
  requires a `_` arm (unless the literals are `true` and `false`), and a variant arm after it is a mixing error;
  a leading variant makes a later literal a mixing error. `R0235` for a repeated literal, and for an arm after a
  wildcard or a binding (`this pattern can never match`). A `choose` that names a variant twice is checked in the
  lowering (`R0235`, `choose` handles variant `E::A` more than once). Not covered: nested and tuple patterns (the
  `$pat` text is not parsed), and the shape probes that the analysis reports.
- Stage 2, diagnostics, step 47 (done): the Ryn specializer's diagnostics (`--monomorphize`, `selfhost/src/frontend/
  generics.ryn`) are compared with the bootstrap's `generics::monomorphize`: an error must be the same, or the bootstrap's
  result stands (`R0904` under `RYN_SEMA_STRICT`). `Program` derives `Clone` for the comparison. This covers `R0263`
  and `R0264`, and a `contracts` fixture that the specializer already rejects.
- Stage 2, diagnostics, step 48 (done): the Ryn parser's syntax errors (`parse` and `parse_recovering`, which is the
  path `ryn check` takes) are compared with the bootstrap parser's, error for error. A tool failure (`R0901`, `R0902`) is
  not compared. Nothing changed in what Ryn reports; the errors are now verified and counted, which makes the
  `R0243` extend errors, the lexer errors and other parser codes measurable.
- Correction: `R0450`–`R0467` are the compile-time evaluation diagnostics of `src/comptime.rs` (constants, `comptime`
  functions, `static_assert`, metadata), not the module loader. The module-loader codes are `R0420`–`R0427`, in
  `src/modules.rs`. Porting `comptime.rs` is a separate, larger task: it is an evaluator (1,905 lines).
- Stage 2, diagnostics, step 49 (done): the compile-time checks of the bootstrap pass (`src/comptime.rs`) for the subset
  that Ryn evaluates exactly: top-level `const` items of integer and `bool` type, `static_assert` statements, and
  assignments to constants. A new Ryn mode, `--comptime` (`selfhost/src/middle/constants.ryn`), reads the tree before
  `comptime::evaluate` folds it, and `Frontend::analyze` runs it first; its diagnostic must be the bootstrap's, or it is
  `R0904` under `RYN_SEMA_STRICT`. The diagnostics: `R0460` for a failing assertion, `R0461` for a name that is not a
  constant (a local in an assertion), `R0462` for a constant that depends on itself, `R0464` for overflow, division
  by zero, a shift out of range, mixed integer types, and a value of the wrong type, and `R0467` for an assignment to a
  constant. Constants are evaluated in program order, then each function is checked: its assignments, then its
  assertions. A `when` on a constant keeps only the branch it takes. The check declines (and the bootstrap decides)
  for modules and imports, calls (including `comptime`), `Vec` values and other types, loops, and choose arms that bind
  a constant's name. Integers are kept in `i64`; an arithmetic result that would not fit in `i64` is declined.
- Stage 2, diagnostics, step 50 (done): `R0207` for a `when` on a constant. The bootstrap folds the constant into a cast
  before analysis, and a cast condition is now lowered without an expected type, so a non-`bool` one reaches the
  `R0207` check. The exact-text helper now runs the same order as the frontend: the compile-time check, the bootstrap
  evaluation, the pattern check, then the analysis on the evaluated tree.
- Stage 2, diagnostics, step 51 (done): `R0251` for a reference returned from a call. A function that returns a
  reference records the parameter it returns, when every `return` of it names one parameter (`return_param` in the
  signature). A call's reference result then points where the argument for that parameter points, so `let` and
  assignment see its origin (`pointed_local` handles calls).
- Stage 2, diagnostics, step 52 (done): the metadata builtins inside a constant or assertion (`field_count::<T>()`,
  `variant_count::<E>()`, `has_field::<T>("name")`), with `R0465` for a missing or non-structure type argument, a
  non-literal field name, or a type that the program does not declare; and `R0466` for a `#[derive(Default)]` structure
  whose field has no default (a structure without `Default`, at the field). The Ryn check now runs before
  `derive_defaults`, because the bootstrap derives first and reports a derive error before any constant error; the
  comparison is against derive then evaluation. `type_name` and metadata calls outside an assertion are not covered.
- Stage 2, diagnostics, step 53 (done): a walk over the expressions and statements of each function body (the
  children of every expression, in the order the bootstrap visits them, with a folded `when` visiting only the branch it
  keeps), so that a metadata call outside an assertion is evaluated: `R0465` for `field_count()` without its type
  argument. A `static_assert` statement is skipped, as the assertion check removes it first. A `comptime` call declines.
- Stage 2, diagnostics, step 54 (done): the pattern pass's matrix compile is ported as checks (`selfhost/src/middle/
  patterns.ryn`): wildcards, bindings, literals, tuples and variant mixing, with the pass's branch choice (the first column
  that is not a wildcard decides it; a leading wildcard reaches nothing after it), its errors in order (tuple arity, a
  literal or tuple mixed with another kind, a missing `_` arm for literals, with `true`/`false` complete), and its
  unreachable arms. A nested variant with a payload, a structure pattern, and a variant column are declined. The check
  parses the pattern texts the parser keeps (`$pat`).
- Stage 2, diagnostics, step 55 (done): the shape probe that the pattern pass makes for a tuple or structure pattern
  (`$shape`, `tuple:N` or `struct`) is checked against its value, as the bootstrap's `pattern_shape` checks it: a scalar
  value (`R0235`, "pattern does not match the type of the value"), and a tuple of another arity (`R0235`, "tuple pattern
  has N element(s) but the value has M"). A probe that matches is declined. The exact-text helper now desugars before the
  analysis, as the frontend does.
- Stage 2, diagnostics, step 56 (done): the compile-time checks now work on programs with modules. A constant is resolved
  as the bootstrap's `resolve_constant` does: the module's own constant, then the program's constant of that name, then
  a constant of a module that a `use` names, through its alias. A private constant of another module is `R0425` at the
  name (`constant `util::SECRET` is private to its module`). The rewrite pass resolves every name in a body that is not a
  local, and every zero-argument path `module::NAME` whatever the locals, so both are walked, after the assertions. The
  checks used to decline any program with a `use`, and no longer do; the projects gate covers the module projects.
- Measurement: `RYN_DUMP_SEMA=<dir>` writes the output of the Ryn analysis to `<dir>/ryn.ir` on every run, so the
  diagnostics Ryn reports itself can be counted per fixture. The strict gate also checks every project directory
  (`tests/suite` and `examples`, each as a whole program), which it did not before. Coverage over the compile-fail
  fixtures: 27 of 140 fixtures with a `// code:` header are reported by Ryn with that code (parser codes are not
  counted, since they come from the parser's own output). Largest gaps: `R0450`–`R0467` (compile-time evaluation, in Rust's
  `comptime.rs`), `R0252`/`R0251`/`R0250` (reference provenance), `R0206` (collections), `R0210` (unknown function).
- Coverage after step 56: 122 of 142 (step 55 was 121; every R0235 fixture is now matched; step 53 was 113 of 141, the new fixture adds one; step 52 was 112; step 51 was 109; step 49 was 107; step 48 was 94; step 46 was 82; step 45 was 75; the fail-directory module fixture added one; step 43 was 71 of 140; the step-35 figure was 27; 41 after step 36; 50 after step 37; 55 after step 38; 57 after step 39; 62 after step 41; 67 after step 42, 71 after step 43). Still open among the reachable codes: `R0251` through a function's returned reference (`w3_ref_via_fn_escapes_block`, which needs the callee's returned parameter, known only after its body is lowered), the `R0207`/`R0425` cases that need constants, and the module-loader codes. The largest remaining group is
  `R0235` (14 fixtures): the literal, tuple and structure pattern checks run in the Rust pattern pass
  (`src/patterns.rs`), before Ryn sees the program, so porting them means porting that pass. Next groups are the module
  loading codes `R0420`–`R0427` and the compile-time codes `R0450`–`R0467`, the generic-argument codes `R0243`/`R0262`–`R0264`, the built-in `print` checks
  (`R0208`, `R0210`), and the reference provenance codes `R0250`–`R0252`.
- Stage 2, diagnostics, step 57 (done): the shape conformance check of an `extend T as Shape` block (`R0450`) runs in
  Ryn. It is the bootstrap's `struct_conforms_to_shape`, read from the AST: a method is a function of the structure whose
  first parameter is `self`; the missing-method, `must take `self``, `must take `mut self` to satisfy``, parameter count,
  parameter type and return type messages are the bootstrap's, reported at the extend block. The check runs before the
  shape defaults are added, as the bootstrap does. Declined (`unsupported`, so the frontend compares nothing): a
  structure with generics, a missing method of a structure in a module (the bootstrap also looks up module-qualified
  method keys, so an absence there is not certain), a shape with other than one method, a parameter or return type
  that is not a builtin word (the bootstrap compares resolved types, and an alias would make a word comparison wrong),
  a receiver whose pointee is another named type (it may be an alias), and a program without `main`, which the
  bootstrap reports first. A receiver whose pointee is a builtin word gives `must take a `B` receiver`; the
  parser does not accept a `self` with an annotation, so no source reaches that message yet, and the branch only mirrors
  the bootstrap.
- Coverage after step 57: 125 of 142 (step 56 was 122). The three new matches are the two `i2_generic_extend_as_shape`
  fixtures and `w3_wrong_return_type_rejected`. The remaining `R0450` fixtures reject a call to a generic function whose
  bound the argument does not meet (`use_it<T: Scale>` called with `B`); that is the specializer's bound check at the call,
  not an `extend ... as` block, and it is still open.
- Exact-text cases: eight shape cases in `tests/ryn_sema_diagnostics.rs` (return type, missing method, receiver
  not `self`, `mut self` mismatch, parameter count both ways, parameter type). `R0450` is in the allow-list there.
- Stage 2, diagnostics, step 58 (done): the bounds of a function are checked in Ryn before its body, as the bootstrap's
  `prepare_function` does (`Lowering::bound_conformance`, called from the body loop of `lower_program`). The specializer
  gives each specialized copy of a generic function bounds named by the concrete type (`type_key`), so a bound whose name
  is a structure is checked against each of its shapes with the same conformance rules as step 57: `R0450` at the
  function's span, with the help `structural conformance checks `B` against `Scale` at the call site`. A bound that names
  an undeclared shape is `R0452` (`unknown shape `Missing` in the bound on `B``, help `declare the shape with `shape Name
  { ... }` before using it as a bound``). A bound whose name is not a structure is skipped, as the bootstrap skips it
  (a builtin word or an enum, as in the std arena functions). The bootstrap also looks the name up as `namespace::name`,
  so a bound is declined only when some structure has that suffix; otherwise the skip is exact.
- Probe: a program with a generic structure (`struct Box<T>`) gets no Ryn analysis at all, because the struct loop declines
  any structure with type parameters (`structure with generics`), so such programs are checked by the bootstrap alone.
  This is why `generics/w2_shape_bound_violated_through_generic` is still uncovered. Lifting that decline is a separate step.
- Tests: `tests/programs/fail/shape_bound_unknown_shape.ryn` (`R0452`) in `tests/compile_cases.rs`. The exact-text harness
  runs the bootstrap on the unspecialized program, where a generic function is not analysed, so bound cases are covered
  by the fixtures and the corpus and projects gates instead.
- Coverage after step 58: 130 of 142 (step 57 was 125).
- Stage 2, diagnostics, step 59 (done): a shape with several methods is checked method by method, as
  `struct_conforms_to_shape` does, instead of declining. The bootstrap collects the requirements in a `HashMap`, so two
  failing methods could be reported in either order; Ryn declines that case and reports a single failing method exactly.
  Five corpus programs that declined for the one-method rule (shapes with default methods, `mut self` methods and
  default tails) now get Ryn IR, and the strict corpus gate agrees with the bootstrap on them. Exact-text case: a two-method
  shape with one non-conforming method.
- Corpus census (588 files under `tests/suite`, `tests/programs/pass` and `examples`, single-file programs): 394 get Ryn IR,
  125 a Ryn diagnostic, 64 decline (before step 59). The largest decline is generic structure instances: 22 files decline
  with `tuple literal`, because a specialized instance is named `$RynStruct#…` and the Ryn lowering treats every `$`-named
  structure as a tuple. Lifting that is the next Stage 2 item; it also unlocks `R0262`
  (`uninstantiated_generic_structure_reported`) and `w2_shape_bound_violated_through_generic`.
- Coverage after step 59: 130 of 142 (unchanged; the step's files are valid programs, not compile-fail fixtures).
- Stage 2, diagnostics, step 60 (done): generic structure instances reach Ryn's lowering. A specialized instance is named
  `$RynStruct#…`; the struct-literal path declined every `$`-named structure as a tuple, which was the largest decline
  in the corpus (22 files). Only `$RynTuple#` names still decline there, and a tuple literal has its own path. The
  corpus census (588 files) goes from 394 IR, 125 diagnostics and 64 declines
  (before step 59) to 419 IR, 128 diagnostics and 41 files with no Ryn output (declines, and the five comptime fixtures
  whose frontend step fails); the gate agrees with the bootstrap on every program Ryn now analyses. `R0262` (`a generic structure has no instance for the inferred type arguments`) is reported by
  Ryn for a `$RynStructParam#` placeholder in a named type, at the name's span, with the bootstrap's help text.
  Regression: `tests/programs/pass/generic_struct_instance_literal.ryn` (an instance literal and a field read; runs and
  prints 7), plus the corpus gate over the suite's generic programs.
- Coverage after step 60: 132 of 142 (step 59 was 130; `w2_shape_bound_violated_through_generic` and
  `uninstantiated_generic_structure_reported` now match).
- Stage 2, diagnostics, step 61 (done): the built-in output calls are lowered in Ryn. `print` becomes one `PrintTemplate`
  statement (text pieces, and values through the echo value rule); `eprint` and `write` become a block of calls to
  `__io_stderr_write` or `__io_stdout_write`, with a string piece passed as it is, a `bool` as `"true"`/`"false"` through
  an `if`, and a number through `String(...)`. `eprint` ends with a newline piece. The block ends with the drop that the
  guard appends to every nested block (`owned_drop`). Other types are `R0234` (`cannot format this type yet`). The
  `echo` and `print` values now get the bootstrap's `R0234` for a type that holds a `Vec`, through an array's element or a
  structure's fields (`holds_vec`); Ryn had no such check before. Exact-text cases cover an `echo` of a vector, a `print`
  of a vector and a structure with a vector field.
- Corpus census after step 61 (588 files): 430 IR, 128 diagnostics, 30 with no Ryn output (up from 419 and 41).
- Coverage after step 61: 132 of 142 (run tests, so the count is unchanged).
- Stage 2, diagnostics, step 62 (done): a shape probe whose check passes keeps only its body, as the bootstrap's
  `pattern_shape` does; Ryn lowers the body at the expected type instead of declining. Nine corpus programs with tuple and
  structure patterns (`patterns/`, `generics/w2_generic_enum_tuple_payload`) now get Ryn IR, and the strict gate agrees.
- Coverage after step 62: 132 of 142. Corpus census: the remaining declines are the FFI signatures with a record or a
  callback (11 files), a method receiver that is not a local (2), `null` in a raw-pointer argument (1), and single cases
  (an unknown function in a destructor call, a boolean where another type is expected).
- Stage 2, diagnostics, step 63 (done): the C ABI check of an `extern "C"` signature is exact in Ryn. A record is passed
  when it is `#[repr(C)]` and its scalar fields pack to 1, 2, 4 or 8 bytes, or 9 to 16 bytes (the non-Windows rule; the
  bootstrap is built for the same host). A callback is an `extern "C"` function pointer whose parameters are scalars,
  `char` or raw pointers and whose result is one of those, a record, or none. Before, Ryn declined any record or callback
  parameter and result. The struct table is now set before the signatures, because the record check needs it; the derive
  loop reads it from the lowering. Eleven FFI corpus programs (the record and `qsort`/`bsearch` callback examples) now get
  Ryn IR, and four exact-text cases cover `R0247`: a structure that is not `#[repr(C)]`, a three-byte record, a callback
  that is not `extern "C"`, and a callback that takes a structure.
- Coverage after step 63: 132 of 142 (run tests, so the count is unchanged).
- Stage 2, diagnostics, step 64 (done): four small gaps, each with the bootstrap's message. (a) `null` in a raw-pointer
  context lowers to the cast of `0` as `U64` to that pointer type, as the bootstrap's `expression` does; a comparison
  with a raw pointer on the left types a `null` on the right (`left_context`), and `==`/`!=` accept raw and function
  pointers. (b) A destructor (`#[drop(name)]`) that names no function of the program is `R0255`, `destructor function
  `name` was not found`, at the structure; in a module a missing name is declined. (c) A method argument of another
  scalar or string type is `R0212` with the method wording and no help (`method argument has type `bool` but `i32` is
  required`), for both the generic-method path and the structure-method path (`struct_method`), which had its own loop.
  Exact-text cases cover (b) and (c); the `null` fixture is a run test in the corpus gate.
- Stage 2, diagnostics, step 65 (done): a method receiver that is not a local. A shared `self` on a copyable temporary
  (a call result, or a field of a local) borrows a new local that holds the value: a `Let` of the new slot is the setup of
  the statement, as the bootstrap's `materialize_copy_receiver` makes it (`pending_statements`). The statement that uses
  it is wrapped in a block with its setup (`with_setup`); the block ends with the scope drop, unless the statement returns,
  since the guard appends the drop only to a block that falls through. Setup belongs to one statement: an enclosing
  statement's setup waits while a nested statement is lowered. A `mut self` on a temporary is `R0235` (`mut self` cannot
  borrow a temporary), and so is a shared receiver whose type is not copyable (`receiver_copyable`: strings, collections,
  enums, slices, references, function pointers, and structures that own data or have a destructor). Two corpus programs
  (`w2_shape_bound_through_generic_struct`, `w6_generic_shape_stack`) now get Ryn IR; exact-text cases cover both
  refusals; `tests/programs/pass/temporary_receiver_borrows_copy.ryn` runs and prints 6 and 18.
- Coverage after step 65: 132 of 142.
- Stage 2, diagnostics, step 66 (done): a `while` condition's receiver temporaries belong to the loop. The bootstrap
  keeps them in `IrStatement::While { setup, .. }`, which runs before every test; the step-65 statement wrapper had put
  them in front of the loop, once. Ryn now takes the condition's setup right after the test is lowered and writes it into
  the `While` setup slot. Two native tests (`receiver_temporaries_in_conditions_run_before_the_branch`,
  `while_condition_reruns_copy_receivers_without_dropping_owners`) had reported `R0904` after step 65 and pass again.
- Stage 2, diagnostics, step 67 (done): the compile-time evaluator in Ryn follows a call of a root function. The arguments
  are evaluated and coerced to the parameters' types, as the bootstrap's `call` does. A body whose first statement is an
  `echo` is `R0461` (`this statement cannot be evaluated at compile time`) at that statement, which is what the bootstrap
  reports for a statement its evaluator does not run; any other body is declined, since the subset does not run statements
  yet. Calls are spanned by `span_of`, so a constant whose value is a call is evaluated. Covered:
  `comptime/runtime_function_is_not_constant_rejected`.
- Not covered and still open in the comptime group: `R0464` for a `Vec` constant or a `Vec` index out of range, which need
  the `Vec` values of the evaluator, and `R0463` for runaway recursion or loops, which need the step and depth counts of
  the bootstrap evaluator.
- Coverage after step 67: 133 of 142.
- Stage 3, step 1 (done, verified outside the repo): the Ryn x86-64 encoder (`selfhost/src/back/x64.ryn`) and the Ryn ELF
  object writer (`selfhost/src/back/elf.ryn`) work when the Rust compiler builds them. A driver encodes integer moves
  and loads/stores of each width, `lea`, the ALU operations, `imul`, `neg`, shifts by `cl`, `cqo`, `test`, `setcc`
  (with its zero-extension), `cmov`, sign and zero extensions, `push`/`pop`, backward `jcc`/`jmp` to a bound label,
  `syscall` and `ret`; `objdump` shows the intended instruction for each. A second driver writes a one-symbol object
  (`_start`, exit status 7 through `syscall`) with `write_object`; `ld` links it and it exits with 7.
  Still open: `gen.ryn` is a skeleton (its last helper uses `fn`, which is not a keyword), so no code generator exists.
- Stage 3, step 2 (done): the Ryn code generator for the scalar subset (`selfhost/src/back/gen.ryn`) builds an object
  file from typed IR (`selfhost/src/native.ryn` decodes the IR text and writes the object bytes, as decimal numbers
  for now, since `fs::write` takes text). Integers of every width, `bool` and `char` values, arithmetic, bitwise and
  comparison operators, `and`/`or` with short-circuit, `when`, `while`, `for` (`..` and `..=`), `break`, `continue`,
  `return`, calls of up to six arguments, `echo` of integers, booleans, string literals and templates, and `main`'s
  exit status. Layout: a cell per local below the frame pointer; each expression leaves its value in `rax`, normalized
  to its width; System V argument registers; `_start` calls `main` and exits. Outside the subset the generator declines
  with the reason: structures, enums, vectors, strings as values, extern functions, pointers, shifts and division.
- Verification: `tests/native_backend.rs` compiles ten programs of the subset, links them with `ld`, and compares their
  output and exit status with the `// out:` and `// exit:` lines; a structure value is declined with its reason. Over
  the 342 `// test: run` programs of `tests/suite`, 84 pass natively and none mismatch except the stack overflow test
  (`raii/r2_lead_stack_overflow_reports_error`: the Ryn object segfaults where the Rust backend reports the overflow;
  there is no stack guard yet); 250 are declined (structures 70, extern functions 33, enums 46, vectors 19, strings 34,
  pointers and builtins 30), and 7 have no expected output.
- Stage 3, step 3 (done): structures and references in the native generator. A structure local is its cells, each field
  at its slot offset; a structure value is stored field by field (`store_into`), and a structure local is copied cell
  by cell. Field reads and writes on locals are already locals in the IR. Through a reference (`&mut` of a local, a
  method's `mut self`), a field is read or written at `pointer - 8 * slot_offset`, since cells grow downwards. A
  dereference reads or writes one scalar. Every internal call now passes its arguments on the stack as cells (a scalar
  one cell, a structure local its cells): the callee copies them into its parameter slots and the caller removes them
  after the call. Programs with a destructor (`#[drop]`) or an owned slot are declined, because the code generator adds
  the destructor calls for them from `owned_slot_types`, which this generator does not yet read; ignoring them gave a
  wrong result on the recursion test, which is now declined. The native sweep over the 342 run tests: 105 pass, one
  known mismatch (the stack overflow test), and 229 declined. `tests/native_backend.rs` checks fourteen programs, and
  declines an extern function with its reason.
- Stage 3, step 4 (done): `str` values are two cells, the address and the length of the text. A string literal is
  stored as its address (a `lea` of its rodata symbol) and length, a `str` local is copied cell by cell, a `str` argument
  is pushed as its two cells (a literal directly), and `echo` of a `str` local writes its cells. The native sweep is now
  106 pass, the same stack-overflow mismatch, and 228 declined; the largest declines are owned values (String and Vec
  locals, which need the heap and drops: 78) and structures with destructors (47), which need the drop calls.
- Stage 3, step 5 (done): C functions. The object's entry point is a global `main` (it returns the result of the Ryn
  `main`, or 0), which the C runtime calls, so the objects link with `cc` and use libc. A call of an `extern "C"` function
  with scalar, pointer or bool arguments and result: the arguments go to the System V registers, the stack is aligned for
  the call (through rbx, which the generated code does not otherwise use), the call goes through the PLT to an undefined
  symbol, and the result is brought to its width. Memory access is at each scalar's own width (a `*u8` reads one byte), so
  C memory and frame cells agree. Extern declarations generate no body. The native sweep is now 109 pass, the stack
  overflow mismatch, and 225 declined. `tests/native_backend.rs` links with `cc`, checks fifteen programs, and declines an
  `f64` foreign argument with its reason (`an echo of this type` for an unprinted value).
- Stage 3, step 6 (done): the stack guard. The object's `main` allocates a 64 KiB alternate stack (malloc), installs it
  with `sigaltstack`, and installs a SIGSEGV handler with `sigaction` (`SA_SIGINFO | SA_ONSTACK`). The handler compares the
  fault address with the faulting stack pointer (both from its context): within 64 KiB is a stack overflow, and it prints
  `Ryn runtime error: stack overflow (recursion is too deep)`, otherwise the invalid-access message; either way it exits
  with status 1, as the Rust runtime does. The encoder gained `lea_label` for the handler's address. The native sweep is
  now 110 pass with no mismatches, and the stack overflow test passes with its stdout and exit status.
- Stage 3, step 7 (done): the native output is binary. `selfhost/src/native.ryn` writes the object's bytes to standard
  output with libc's `putchar` (an `extern "C"` call, which the Rust backend supports), and writes a decline to standard
  error with status 3. The harness and `tests/native_backend.rs` read the bytes from standard output. The native sweep is
  unchanged at 110 pass, no mismatches. No Rust change was needed.
- Stage 3, step 8 (done): integer division and shifts, pointer casts and the standard streams. Division and remainder
  check for a zero divisor (the runtime message on standard error, exit 1, as `ryn_integer_division_by_zero`); a signed
  least value divided by -1 is divided by 1, so it wraps. A shift takes a count below the width as is; a count at or
  above it gives zero, or all ones for a signed right shift of a negative value. A cast to a raw pointer keeps the bits.
  `eprint`/`write` to standard output or error are `write` system calls (a string literal or a string local); the flushes
  do nothing, since output is not buffered. The address of a `repr(C)` structure local is declined, because a frame's
  cells are not C's layout. The native sweep is now 122 pass, no mismatches, and 213 declined.
- Stage 3, step 9 (done): every argument goes through one recursive pusher (`push_value`): a scalar, a `str` (a literal's
  address and length, or a local's two cells), a structure local, or a structure value whose fields are written in
  declaration order (a structure value written out of field order is declined, because the fields are evaluated in the
  order they are written, and a test checks that order). The native sweep is now 135 pass, no mismatches, and 200
  declined. The remaining declines are owned values (79), structures with destructors (47), enums (21), floats in `echo`
  (float formatting), and generic instances.
- Stage 3, owned values (measured, no code change): with the owned-value and destructor gates temporarily disabled, the
  native sweep's declines behind those gates are: a value that is not a cell (`String` and `Vec` locals and results) 64,
  a structure value not in the subset 34, a parameter of an owned type 23, a system call 16 (`String` writes), an enum
  value 15, an echo of a float or owned value 15, an enum match 6, and smaller groups. The two `Vec` operations and one
  `String` operation that reach the call gate are `Vec` `Index` and `Len` and `String` `CharCount`. The gates are kept
  until owned values have a heap representation and their drops, which is a design decision (the runtime is written in
  Ryn and linked into each program, or emitted as machine code; malloc from libc is available through the foreign calls).
- Stage 3, step 10 (done): owned `String` values, the first owned values of the native backend. A `String` is one cell:
  the address of a 24-byte header {data, len, capacity} from libc's `malloc`; the data is a `malloc`ed copy of the text,
  made with `memcpy`. `String(text)` for a str makes the pair; printing a `String` writes its data; `len()` is the header's
  length; `char_count()` counts the bytes that do not continue a character; a moved local is read as a local. A `String`
  is not freed yet: its drop is left to the heap (the process ends with it), and owned values of other types are still
  declined, and so are destructors. The native sweep is now 147 pass, no mismatches, and 188 declined. The aligned calls
  to libc (`aligned_call`) keep the stack 16-byte aligned, through rbx, as the foreign calls do.
- Stage 3, step 11 (done): `Vec` and enum values. A `Vec` is one cell, the address of a header {data, len, capacity}; its
  elements are the canonical 8-byte cells of their values (scalars, Strings, and other one-cell values). `Vec()` is a
  fresh header; `push` grows the data by `realloc` (doubling, at least four) and stores at the end; `len()` reads the
  header; `vec[i]` checks the index against the length and, past it, writes the runtime's message and exits with 1.
  An enum value is one cell, the address of a box {tag, one cell per field} from `malloc`; a construction stores the tag
  and the fields; a `choose` compares the tag with each arm in order (an arm with no variant always matches), copies the
  arm's fields into their slots, and yields its body. Vectors and enums are leaked like Strings, and an enum with a field
  that is not one cell is declined. The native sweep is now 176 pass, no mismatches, and 159 declined.
- Stage 3, step 12 (done): enum payloads of any type that takes cells: a scalar or owned value is one cell, a `str` two,
  and a structure (a tuple included) its slot count, stored field by field in the order its values are written (so the
  evaluation order is the one of the source), and a structure local copied cell by cell. A match copies a bound payload's
  cells into the binding's slots. The payload offset of a field is the sum of the cells of the fields before it. The
  native sweep is now 183 pass, no mismatches, and 152 declined.
- Stage 3, step 13 (done): a call returns a `str` or a structure. The caller reserves the result's cells before it pushes
  the arguments, so they lie above the arguments; the callee writes them there, cell k at `rbp + 16 + 8 * (argument cells
  + count - 1 - k)`, in the order of a pushed value. A call that yields such a value leaves its cells on the stack: a
  `let` or an assignment copies them into its slots, a field read takes one cell, an argument or a `return` pushes them
  on, and a call statement drops them. A call in a one-cell expression declines a string or structure result. The native
  sweep is now 190 pass, no mismatches, and 145 declined. `tests/suite/structs/` gains two programs (the suite is 520),
  and `tests/native_backend.rs` runs them with a generic function that returns a pair.
- Stage 3, step 14 (done): a moved structure or `str` local (`MoveRef`) is read like a plain local wherever a structure
  or string is pushed, stored, or has a field read: as an argument, a `return` value, a `let` value, or a field of a
  moved tuple or structure. The native sweep is now 193 pass, no mismatches, and 142 declined. Two tuple programs still
  decline: a `choose` over a tuple literal reads the fields of a value that is not a local, which is a separate piece.
  `tests/native_backend.rs` runs the two generic structure programs that now pass. The full `cargo test --release`
  passed (378 tests) before this step, and the targeted run after it (native, suite, selfhost project, and the
  diagnostics and parity tests) passes.
- Stage 3, step 15 (done): a field of a structure value that is not a local (a tuple literal, or a `choose` over one).
  The value is pushed and the field read from its cells, as a call result is. Because each field read evaluates the
  value again, the value must have no effects: literals, locals, casts, and structures of those. An impure value, such as
  a call in a tuple, is still declined. The Rust compiler evaluates such a scrutinee once (checked with a call that
  prints), so the native backend declines rather than repeat it. The native sweep is now 196 pass, no mismatches, and
  139 declined. `tests/native_backend.rs` runs the two tuple programs.
- Stage 3, step 16 (done): the `Option` and `Result` methods on one-cell payloads: `is_some`, `is_none`, `is_ok`, `is_err`
  compare the box's tag with the variant they name; `unwrap_or` takes the payload when the tag is the success tag, and
  otherwise the default, which is evaluated after the receiver as in the Rust code; `unwrap`, `expect` and `unwrap_err`
  take the payload, and on the wrong variant write the runtime's message to standard error and exit with status 1 (an
  `expect` message must be a string literal, so the message is known when the code is generated). A `str` or structure
  payload is still declined. `tests/suite/result_option/` gains three programs that exit with status 1 (the suite is
  523), and `tests/native_backend.rs` runs them with the passing `Option` and `Result` programs. The native sweep is now
  203 pass, no mismatches, and 132 declined.
- Stage 3, step 17 (done): the vector operations `capacity`, `set`, `take` and `insert` for one-cell elements. `set` and
  `take` check the index against the length and `insert` checks it against the length plus one, each failing with the
  runtime's message and status 1; `take` moves the later elements down one, and `insert` grows a full vector and moves
  the later elements up one. A failing check writes its message through a shared `fail_exit`. Growth now doubles the
  capacity from 4, as the Rust runtime does; the old growth (twice the capacity plus four) was visible through
  `capacity()`, which the native backend had not yet exposed. `tests/suite/collections/vec_capacity_and_positional_edits.ryn`
  checks the capacities and the edits against the Rust output (the suite is 524). The native sweep is now 210 pass,
  no mismatches, and 125 declined. `clone` and `sort` of vectors are still declined.
- Stage 3, step 18 (done): the `?` operator on a one-cell payload (`Option` and `Result`). A success gives the payload;
  otherwise the function returns a new box of its own enum with the failure tag, carrying the error of a `Result` (the
  error is one cell). The `defer` bodies of the scopes it leaves run first, with the new box kept on the stack, since
  they clobber the registers. A `str` or structure payload or error is still declined. The native sweep is now 217 pass,
  no mismatches, and 118 declined; the seven `?` programs run in `tests/native_backend.rs`.
- Stage 3, step 19 (done): `echo` and `print` of a `char`. The codepoint is encoded as UTF-8 into an 8-byte buffer on the
  stack (one to four bytes, by the ranges of the Rust encoder) and written to standard output with one write call.
  `tests/suite/output/echo_char_utf8.ryn` checks one-, two-, three- and four-byte characters against the Rust output
  (the suite is 525). The native sweep is now 218 pass, no mismatches, and 117 declined. Float formatting is still
  declined (`echo` of `f32`/`f64` and `String(float)` in `write`).
- Stage 3, step 20 (done): destructors (`#[drop(f)]` structs). A destructor value is dropped where the IR says (`DropStmt`)
  when its handle, its first cell, is not null; the destructor receives every cell of the value. A move nulls the handles
  it moves out, and the function's own destructor owners are dropped on `return` and on the `?` failure path, last slot
  first, as the Rust codegen does. The owner handles start null, and a drop clears its handle, so a later drop of the
  same slot does nothing. An assignment to a value that holds a destructor builds the new value, drops the old one, and
  then replaces the cells; a field assignment of a destructor is still declined. A discarded destructor result is dropped
  at its statement, and a field read of a temporary destructor is declined. The Rust-checked program
  `tests/suite/raii/native_drop_order_and_moves.ryn` covers the order of drops, moves, a value passed to a function, and
  a discarded result (the suite is 526). The native sweep is now 259 pass, no mismatches, and 76 declined.
- Stage 3, step 21 (done): `String` operations: `clone` (a new header and a copy of the bytes, with the capacity of the
  length), `append` (`push_str`: the text is copied after the length, and a full buffer grows to twice its capacity, at
  least the new length and at least eight bytes, with `realloc`), and `char_at` on a `String` (the character is found by
  walking the UTF-8 lead bytes, and decoded; a character past the end fails with the runtime's message and status 1).
  `tests/suite/collections/native_string_append_grows.ryn` and `native_string_char_at_utf8.ryn` check them against the
  Rust output (the suite is 528). The native sweep is now 263 pass, no mismatches, and 72 declined. `slice_chars`,
  `ConcatString`, `TryParse`, and the byte and search operations are still declined.
- Stage 3, step 22 (done): the byte and slice operations of a `String`. `byte_at` reads the byte, and fails past the end
  with the runtime's message. `substring` (`slice_chars`) finds the byte offsets of its two character indices with the
  same walk as `char_at` (which now shares it), and copies the bytes between them into a new `String`; an out-of-order
  or out-of-range pair fails with the runtime's message, which is the only one reachable after the order check.
  `tests/suite/collections/native_string_slice_chars.ryn` checks the slices against the Rust output (the suite is 529).
  The native sweep is now 264 pass, no mismatches, and 71 declined.
- Stage 3, step 23 (done): `exit(status)` drops the function's destructor owners, last slot first, and then ends the
  process with the status, as the runtime does; a vector's `clear`, and its `clone` when the elements hold no memory of
  their own (a copy of the cells, with the capacity of the length, as the Rust clone). The native sweep is now 266 pass,
  no mismatches, and 69 declined; `tests/suite/collections/native_vec_clone_clear_capacity.ryn` checks the clone's
  capacity against the Rust output (the suite is 530).
- Stage 3, step 24 (done): `write` and `eprint` of numbers and bools. `String(n)` of an integer of any width (`FromI8`
  to `FromU64`) writes its digits backwards into a frame buffer, with a minus sign for a negative signed value, then
  copies them into a malloc'd text with a header {data, length, length}; `i64::MIN` and `u64::MAX` come out as the Rust
  runtime prints them. A `write` or `eprint` piece that is an owned `String` is read through its header, and a `bool`
  piece is a choice between two string literals (`true`, `false`), branched on at run time. The native sweep is now 270
  pass, no mismatches, and 65 declined; `tests/suite/output/native_integer_strings_and_eprint.ryn` checks the integer
  texts and the stderr lines against the Rust output (the suite is 531). Float pieces are still declined.
- Stage 3, step 25 (done): `Map` with scalar or `String` keys and scalar or `String` values, as one cell (a header of
  buckets, bucket count, length and a sequence number). A key is hashed with FNV-1a, over its eight bytes or its text,
  as the Rust runtime does; a node holds the hash, the key, the value and its sequence number, and a bucket chains its
  nodes. The buckets double when the length passes their count. The routines (hash, find, grow) are emitted once. The
  operations are `new`, `len`, `is_empty`, `clear`, `contains_key`, `insert` (1 for an existing key, else 0), `remove`
  (1 or 0), and `get` for a scalar value (an `Option` box). `get` of a `String` value is declined, since the copy would
  share the stored header. `keys`, `values`, `clone`, and `Set` are still declined; iteration must follow the Rust
  order, which is by hash. `tests/suite/collections/native_map_growth_remove_strings.ryn` checks growth, removal,
  re-insertion and string keys against the Rust output (the suite is 532). The native sweep is now 276 pass, no
  mismatches, and 59 declined.
- Stage 3, step 26 (done): sets (`Set<T>`) use the map code, as the Rust codegen does (a set is a map whose values are
  `bool`), and a `get` of a `String` value returns a copy of its header, cloned as `String.clone` does, so the copy can be
  changed without changing the map. `tests/suite/collections/native_set_and_get_copy.ryn` checks the set results and the
  copy against the Rust output (the suite is 533). The native sweep is now 278 pass, no mismatches, and 57 declined;
  `keys`, `values`, `clone` of a map or set, and maps with values that hold destructors are still declined.
- Stage 3, step 27 (done): `sort()` and `contains()` of a vector of scalars or Strings. The sort is a stable insertion
  sort, as the Rust sort is; a String is compared by its bytes (a routine), and a scalar by its word, signed or unsigned as
  its type is. `contains` is a scan. A Rust runtime bug was found by this comparison: the sort compared `u64` elements by
  their little-endian bytes, and `char` elements by their first byte only, so `Vec<u64>` and `Vec<char>` sorted wrongly
  (`src/runtime/vector.rs`). Both now compare the whole value. `tests/suite/collections/native_vec_sort_scalars_strings_chars.ryn`
  checks scalars, `u64`, `char` and Strings (the suite is 534). The native sweep is now 280 pass, no mismatches, and 55
  declined.
- Stage 3, step 28 (done): `==` and `!=` of Strings and `str`s (the lengths, then the bytes; a literal, a local, or a
  String through its `str` view, or a String expression through its header), which the string literal patterns of
  `choose` use; and `==` and `!=` of raw pointers (`null` tests). `tests/suite/patterns/native_string_equality_and_null.ryn`
  checks the cases against the Rust output (the suite is 535). The native sweep is now 283 pass, no mismatches, and 52
  declined.
- Stage 3, step 29 (done): `concat` of two Strings (a new String with the first text and then the second; the operands
  are consumed, as the Rust call does), and string queries (`len`, `char_count`) on a String that a call returns, read
  through its header. `tests/suite/collections/native_string_concat_edges.ryn` checks the edge cases against the Rust
  output (the suite is 536). The native sweep is now 285 pass, no mismatches, and 50 declined.
- Stage 3, step 30 (done): `try_to_<int>()` of a String or str for the eight integer types. The text is read as Rust's
  integer parse reads it: an optional `+` (or `-` for a signed type), then one or more ASCII digits and nothing else; a
  value past the range (the negative range of a signed type reaches one further) is None. The Option is found by the
  payload type, since the IR may already hold an `Option<u64>` under another name (`RynOption#FindU64`), which a name
  lookup missed. `tests/suite/result_option/native_try_parse_integer_bounds.ryn` checks the type boundaries against the
  Rust output (the suite is 537). The native sweep is now 286 pass, no mismatches, and 49 declined. Float parses are
  still declined.
- Stage 3, step 31 (done): `clone` of a map or a set. The copy has the same bucket count, length and sequence, and every
  node is copied into a new bucket (a String key or value through a copy of its text, as `String.clone` makes it), so
  changing or removing a key in the original leaves the copy as it was. `tests/suite/collections/native_map_clone_is_independent.ryn`
  checks the independence and the lookups against the Rust output (the suite is 538). The native sweep is now 287 pass,
  no mismatches, and 48 declined. `keys()` and `values()` (which need the hash order) are still declined.
- Stage 3, step 32 (done): `slice(start, end)` of a String by bytes. The range must be in order and within the text, and
  each end must be on a character boundary (the end of the text, offset 0, or a byte that is not a continuation byte);
  a bad range fails with the runtime's message and status 1. The bytes are copied into a new String.
  `tests/suite/collections/native_string_byte_slice_boundaries.ryn` checks the boundaries and the failure against the Rust
  output (the suite is 539). The native sweep is now 288 pass, no mismatches, and 47 declined.
- Stage 3, step 33 (done): the struct layout conflict is resolved. A pointer to a structure local (or a whole structure
  stored through a raw pointer) is the address of its lowest cell; a field at slot offset `k` of a structure of `N` cells
  lies `8 * (N - 1 - k)` above it, which is the frame layout itself, so references, raw-pointer reads (`(*p).field`) and
  stores through pointers agree. A structure stored through a pointer must fit its C size (`sizeof`, which the arena
  reserves) as `8 * N`, and a `repr(C)` structure is never read or written through a pointer by this layout (its C layout
  is different). `tests/suite/guard/native_struct_references_and_raw_pointers.ryn` and the arena round trip check it
  against the Rust output (the suite is 540). The native sweep is now 289 pass, no mismatches, and 46 declined.
- Stage 3, step 34 (done): `f32` and `f64` values. A float is a cell of its IEEE bits (4 or 8 bytes); its literal is the
  `f64` bits, narrowed for `f32`; `+ - * /` and the comparisons use the SSE instructions, and an ordered comparison of a
  NaN is false, `==` is false and `!=` true, as in Rust; negation flips the sign bit; conversions between the float
  widths and from a signed integer (or a 32-bit unsigned) use `cvtsi2*`/`cvtss2sd`. Float-to-integer conversion (which
  saturates in Rust) and `u64` to float are declined, and float arguments and results of C calls are declined (C passes
  them in SSE registers). Printing a float is still declined. `tests/suite/literals/native_float_*` check the arithmetic,
  conversions and NaN comparisons against the Rust output (the suite is 542). The native sweep is now 291 pass, no
  mismatches, and 44 declined.
- Stage 3, step 35 (done): vectors of structures. An element takes one 8-byte cell per cell of its structure, laid out as
  a pushed value (`push`, `index`, `set`, `take`, `insert` and `clone` copy the cells as a block; `take` and `insert` shift
  the elements with `memmove`). A structure with a destructor is still declined, since a vector drop does not run element
  destructors. `Vec::is_empty` is in. `tests/suite/collections/native_vec_of_structures.ryn` checks the operations against
  the Rust output, and `native_string_append_is_empty_starts_with.ryn` checks `append`, `is_empty` and `starts_with`.
- Stage 3, step 36 (done): structure places. A field of a field of a local is read in the frame at its offset. A structure
  read through a reference (a field of a `&Struct` or a dereference) is read in memory at its address plus the distance to
  the field's lowest cell (`8 * (N - o - n)`), and copied out as cells, pushed, or stored into a box. A `?` on a
  structure payload copies the success box's cells; a `Result` error of any cell count (without a destructor) is copied
  as all its cells. A structure local moved or returned by a call into an enum box is copied with its handles nulled.
  Tests: `native_nested_field_reads_and_copies.ryn`, `native_struct_places_through_references.ryn`,
  `native_question_on_structures.ryn` and `native_str_fields_of_locals.ryn`. A `str` field through a reference is declined,
  since the reference compiler does not accept it.
- Stage 3, step 37 (done): `unwrap`, `expect` and `unwrap_or` of a structure payload push its cells. The fallback of
  `unwrap_or` is evaluated only on the fallback path, so it must be pure. `tests/suite/result_option/native_unwrap_structure_payloads.ryn`.
- Stage 3, step 38 (done): `clone` of a vector whose elements own Strings, vectors, or enums with payloads that own nothing.
  Each vector element type gets a clone routine (called with the header in rdi) that copies the block and replaces each
  owned cell by its clone: a String by the String clone, a nested vector by its element type's routine. Maps and sets,
  and enums whose payloads own memory, are declined. `native_vec_clone_of_owned_strings.ryn` and
  `native_vec_clone_nested_owned.ryn` check that the copies are independent.
- Stage 3, step 39 (done): `String::push(char)` encodes the character as UTF-8 into a buffer on the stack (the encoder is
  shared with `echo` of a character); `concat` with a `str` suffix; `contains` and `ends_with` with `str` or `String`
  patterns; `split` with `str` or `String` separators, following Rust's rules (an empty separator matches between the
  characters, so the first and last pieces are empty). `native_string_push_utf8.ryn`, `native_string_search_and_concat.ryn`
  and `native_string_split_pieces.ryn` check them against the Rust output.
- Stage 3, step 40 (done): only the functions reached from `main` are generated (a call, a destructor drop, or `main`
  marks its target), so unused library code no longer blocks the driver. The program's arguments are kept in a zero-filled
  `.bss` cell that the entry fills from argc and argv: `arg_count()` is argc less one and `arg(i)` is argv[i + 1], with the
  empty text past the end. `str` call results are usable as text (in arguments, `String(...)`, comparisons and `echo`).
  `tests/suite/output/native_program_arguments_absent.ryn` checks the no-argument case; the case with arguments was checked
  by hand against the Rust build (`one two`).
- Stage 3, step 41 (done): `Vec::get`, `first`, `last` and `pop` (an `Option` of a copy; `get` clones the copy's owned
  cells, `pop` moves the element out and shortens the vector), and `read_file` and `write_file` through the C library. The
  messages for a file that cannot be read (`could not read `path`: strerror (os error n)`) and for invalid UTF-8 match the
  runtime's, and the UTF-8 check follows Rust's `str` rules (checked on overlong, surrogate, truncated and too-large
  sequences). `native_vec_get_option.ryn`, `native_vec_pop_moves_out.ryn` and `native_file_write_then_read.ryn` check the
  operations against the Rust output.
- Status after step 41: the native sweep over the run tests (before steps 40 and 41) had 349 pass, 43 declined, no
  mismatches. The driver's reachable functions that the Ryn backend declines went from 628 at the start of step 35 to a
  few dozen; the remaining ones are the split and search of string kinds not yet in the subset (`ToLower`, `ContainsString`
  with some forms), a few structure forms, and `try_parse` of non-integer types. The Rust compiler is still required, the
  driver is not yet built natively and run, and the bootstrap fixpoint without Rust is not verified (Stages 4 and 5 are
  not started).
- Stage 3, step 42 (done): fixed arrays of plain scalars (integers, `bool`, `char`, `f32` and `f64`) in locals, parameters,
  results and assignments. An array takes one cell per element, element `i` in cell `slot + i`, the layout the bootstrap's
  storage allocation gives it, so the IR slots need no change. `[a, b, ...]` and `[x; n]` push their items (the item of a
  repeat is evaluated once) and then store them, so `s = [s[1], s[0]]` reads the old values. `a[i]` reads and `a[i] = v`
  writes a local array; the index is checked as an unsigned value against the length, so a negative index of a signed type
  fails the same check as one past the end, and the write checks the index before it evaluates the value, as the
  bootstrap does. The bounds failure is the one `Vec` uses. Arrays of other elements (strings, structures, nested arrays),
  indexing of other places (fields, call results, references), `echo` of an array and slices are still declined.
  Tests: `native_fixed_array_values.ryn`, `native_fixed_array_calls_and_order.ryn`,
  `native_fixed_array_element_widths.ryn`, `native_fixed_array_index_checks_before_value.ryn` and
  `native_fixed_array_negative_index.ryn`, with expectations taken from the Rust compiler. The suite is 563
  (EXPECTED_TOTAL). `tests/native_backend.rs` also runs `fixed_array_indexing`, `dynamic_array_index_checked_at_runtime`
  and the two constant-length programs. The native sweep over the run tests is now 360 pass, 38 declined, no mismatches
  (before: 356 pass, 42 declined); the four newly passing programs are `fixed_array_indexing`, `w6_arena_bst` and the two
  constant-length programs, and no program that passed before fails now.
- Stage 3, steps 43-50 (done): the native sweep over the run tests has no declined program. All 421 run programs (and the
  project programs with path dependencies) build with the Ryn backend and give the Rust output, status and, where it is
  specified, the standard error text. What was added:
  - Floats: `echo`, `print`/`write` and `String(float)` give Rust's `Display` text (`back/floats.ryn`: the shortest digits that
    read back, found with `snprintf("%.*e")` and `strtod`/`strtof`, with the round-upward retry and the halfway rule that Rust's
    printer follows; checked on 11,373 values against the Rust runtime), `try_to_f32`/`try_to_f64` follow Rust's grammar,
    float to integer casts saturate (NaN is 0), `u64` converts to a float, and `std::math` (libm functions; `min`, `max`,
    `clamp`, `abs` inline). Programs now link with `-lm`.
  - Arrays of plain scalars are laid out so that element 0 is the lowest cell; slices (`&[T]`) of arrays and vectors, with the
    runtime's range checks; function pointers: a function whose address is taken gets a C-convention entry (`emit_thunks`),
    so `qsort` and `bsearch` call back into Ryn, and indirect calls follow the C ABI with integer and float registers.
  - C calls: float arguments and results in xmm registers, and `repr(C)` records of up to 16 bytes by value, classified per
    eightbyte (System V). A `repr(C)` local whose address is taken has a C-layout copy in scratch cells that foreign calls
    update before and read back after; records read and written through pointers are converted field by field.
  - Vectors and maps of destructor values: `clear`, scope end, replace and remove run the destructors (last element first),
    and a moved vector or map leaves a null slot behind. A map value of such a type is a one-cell handle.
  - Text: `trim`, `trim_start`, `trim_end` (Unicode White_Space), `to_lower`/`to_upper`, `find`, `replace`. Known limit: case
    mapping covers ASCII, Latin-1, part of Latin Extended-A, Greek and Cyrillic letters and `ß`; other scripts are unchanged.
  - Pattern matches that bind owned data out of a vector element copy that data (enum copy routines), because this backend
    shares the element's box; before this, a consuming loop over a payload vector emptied the vector in the program.
  - Generated objects carry local symbols `ryn_function_<index>` for the generated functions.
- Stage 3 gate (verified): the driver `selfhost/src/native.ryn` was compiled by the Ryn backend (no declined function), linked
  with `cc`, and the resulting executable generates the same object bytes as the driver run under the Rust compiler for every
  run program, and it compiles itself to an identical object (stage 2 equals stage 1; 73 s). The Rust compiler is still needed to
  produce the IR (`ryn check`), the executable is linked with `cc`, and the C library supplies memory, files and float text,
  so Stages 4 and 5 (Ryn runtime and linker, a bootstrap without Rust) are not done and the compiler is not fully self-hosted.

## Stages 4 and 5: status (2026-10-09)

- Stage 4 (no C library, own linker), done so far: `back/rt.ryn` is the freestanding runtime (malloc, free, realloc, memcpy,
  file and error routines, `putchar`, signal return), `back/elf.ryn` links static executables (`link_executable`), and
  `generate_executable` with `_start` builds programs without `cc` or libc. Freestanding, 407 of 424 run programs pass; the 17
  declined ones need float text (`echo` of a float), which still calls libc `snprintf`/`strtod`. Porting that to `middle/floatlib`
  is the remaining Stage 4 item.
  - Stage 4 float item done (2026-10-09): `back/float_runtime_ir.ryn` embeds the typed IR of `float_runtime.ryn`, so
    `middle/floatlib`'s shortest formatting and direct-width parsing compile into every freestanding binary; `back/math.ryn`
    emits SSE2 and x87 for `std::math`. All 424 run programs now pass in the typed-IR pipeline. See
    [native float validation](native-float-validation.md). The direct-source pipeline through `rync`'s own frontend still
    passes 303 of 424; the remaining cases are compile-time evaluation, compile-time function calls, metaprogramming, some
    pattern forms, module path lookup and C imports.
- `selfhost/src/rync.ryn` is the libc-free compiler driver. The native-built `rync` compiles hello correctly.
- Option 2 is implemented: deterministic owner drops and move invalidation in the native backend, with no GC.
  Nested enum payloads and unborrowed reference fields are cloned independently. Discarded temporaries (including String
  comparisons), propagated enum shells, moved-field remainders, replacement values, map removals and collection clears
  release their resources. Temporary owners remain visible to early-return cleanup; borrowed fields remain borrowed.
- The existing lowerer already emits iteration, `break` and `continue` cleanup. Allocation-counter regressions confirm it.
- Stage 5's native bootstrap now passes under the unchanged 1 GiB address limit. The native seed builds stage2;
  stage2 compiles working ownership regressions and rebuilds itself byte-identically as stage3. The lowerer driver also
  compiles natively and produces the same hello IR as the Rust-driven frontend.
- Validation and exact memory figures are recorded in [native ownership validation](native-ownership-validation.md).
  Stage 4 still has 17 known freestanding float-text declines; this does not claim that all libc functionality is ported.
