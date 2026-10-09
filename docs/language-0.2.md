# Ryn 0.2 language work

This document records what the 0.2 work added, how it is tested, and what is still missing. It
complements `docs/audit-0.0.2.md` (the feature audit) and `docs/self-hosting-plan.md`.

## Baseline before the work

`cargo test --release` passed completely: 123 library tests plus the integration suites, the
103 analysable corpus programs of `ryn_sema_subset`, and the parity tests between the Rust
bootstrap parser and the Ryn frontend. Generics (inferred and explicit), `shape` contracts,
`#[drop]` destructors, `defer`, `Option`/`Result` with `?`, `?.` and `??`, `choose` over enums
with exhaustiveness checking, modules, `extern "C"`, `Vec`/`Map`/`Set`, and Ryn Guard's move and
borrow rules were already present.

## What was added or repaired

| Area | Change |
| --- | --- |
| Output | `print("fmt {}", value)` writes a line to stdout, `write(...)` writes without a newline, `eprint(...)` writes a line to stderr. Placeholders are `{}`, braces are written `{{` and `}}`. A user function with the same name takes precedence. `echo` is unchanged. |
| Comptime | Assigning to a constant is R0467 (a local or parameter of the same name is a different variable). Top-level `const NAME: T = expr`; constants may call ordinary functions made of literals, locals, `String`, `Vec`, and structures (`push`, `append`, field assignment, `concat`, `char_count`, `index`, ...), `when`/`while`/`for`, and calls. A constant may be a scalar, `str`, `String`, or a structure; a `Vec` can be used while evaluating but cannot be a constant (R0464). A top-level constant of an integer type can give the length of an array type: `[i32; N]` and `[0; N]` with `const N: u64 = 4`. `when` on a constant condition keeps only the taken branch at compile time (see Known limits). `comptime(expr)` forces evaluation. `static_assert(cond[, "message"])` stops compilation. Evaluation is bounded (2,000,000 steps, 200 nested calls) and integer overflow is an error. Codes R0460-R0466. |
| Metaprogramming | `field_count::<S>()`, `variant_count::<E>()`, `type_name::<T>()`, `has_field::<S>("f")`, `sizeof`/`alignof` of scalar, array, and plain struct types inside constants, and `#[derive(Default)]` which synthesizes `Type::default()`. |
| Patterns | `choose` accepts integer, character, boolean, and string literals, negative literals, tuple patterns (`(0, y)`), structure patterns (`Point { x: 0, y }`), and nested variant patterns (`Outer::Nested(Inner::Data(5))`) in any combination. They are compiled to plain variant arms and `when` chains; a pattern that can never match is rejected (R0235). A top-level name is a binding pattern that catches every remaining value (`other => other * 2`). Arms take the type expected by the `choose` context: a function's result type or a declared local type applies to every arm, so `-> u8` with `1 => 200` yields a `u8`. |
| Generics | Generic `extend<T> Name<T> { ... }` blocks for generic structures (methods and `const`s, several parameters, declared before or after use); the block is parsed again with the parameters bound for every instance. Generic functions take and return generic structures (`fun size<T>(s: Stack<T>) -> u64`), including structures with two parameters. Generic functions with a function-typed parameter (`fun app<T, U>(v: T, f: fun(T) -> U)`) infer `T` and `U` from a plain function passed as the argument, so `std::option::map` and `std::result::map` work. Generic functions over generic enums (`fun tag<T>(o: Outcome<T>)`), tuple arguments (`id((1, 2))`) and `Option::Some(x)` arguments infer their type arguments. Nested generic calls (`id(id(x))`), explicit type arguments with integer literals, and parameters with concrete types no longer fail inference. |
| Contracts | A generic function bounded by a shape may call only the methods of that shape on a parameter of type `T` (R0450), checked on the generic definition. Duplicate shapes, duplicate shape methods (R0453), and unknown bounds (R0452) are rejected. A shape method that takes `mut self` must be implemented with `mut self` (R0450). Generic `extend<T> Name<T> as Shape { ... }` works: each instance is checked against the shape. Shape default methods may end in an implicit tail expression. |
| RAII | A destructor-owning struct may hold a `String`; the destructor releases it once. Owned fields of such a value can be read, and moving them out is rejected (R0255). |
| Guard | Using a value while a mutable reference to it is alive is rejected (R0249); a constant array index out of range is a compile error (R0257). Moving a non-copy value out of a reference (`return *r` with `r: &String`) and borrowing a moved value (`eat(s); &s`) are R0240. A reference or raw pointer stored into a binding or field that outlives the local it points to is rejected (R0251, R0252), including a raw pointer held in a structure literal or a copied structure, and a returned structure that holds a pointer to a local (R0248), and reading a raw pointer whose owner has been moved is R0240. A destructor value that is discarded as a statement is dropped at the end of the statement. Returning the address of a local through a raw pointer (`return &raw mut x` with `x` local) is rejected (R0248): a raw pointer to a local cannot escape its stack frame. |
| Language | `null` is the zero address of the raw pointer type its context expects (`f(null)`, `p == null`, `q: *i64 = null`). `unsafe { ... }` is accepted as an ordinary scope; it is not required for raw pointers. `use a::b as c` imports a module under another name and `pub use` is the same as `use` (imports are visible project-wide). A stack overflow prints `Ryn runtime error: stack overflow` and exits with status 1 on Linux. `type_name::<Box<i32>>()` prints `Box<i32>` for generic structures. |
| Lexer, parser | A number directly followed by letters (`12abc`) or a second decimal point (`1.2.3`) is R0011. Trailing commas are accepted in function parameters and enum payload types. A keyword used as a name says it is reserved. `#[derive]` on a non-struct is an error. `when flag { n: i32 = 4 }` parses (a `name {` in a condition is not a struct literal when the block starts with a declaration). |
| Diagnostics | A tuple or structure pattern on a value of another type is R0235 ("pattern does not match the type of the value"); bare `None`/`Some`/`Ok`/`Err`, `id<i32>(1)`, and unknown enum variants carry hints; a missing item in a module is "unknown item" instead of "unknown enum"; private functions in nested modules give R0425. |
| Modules | Circular imports are rejected (R0427); private constants are rejected across modules (R0425); `pub const` is visible through `use`. |
| FFI | `extern "C"` accepts callback parameters (`extern "C" fun(*u8, *u8) -> i32`), verified against libc `qsort`, and `#[repr(C)]` records of 9 to 16 bytes on non-Windows targets, classified per System V eightbyte. Eightbytes made only of floats are passed in SSE registers (`{f32,f32}`, `{f64}`, `{f64,f64}`); this was wrong before and is verified against real C (`conjf`, `cabsf`, `ldiv`, `lldiv`). Records of 16 bytes are run against libc `ldiv`, `cabs`, and `conj`; records of 9 to 15 bytes are accepted by `ryn check`, but no test calls real C code with one yet. Larger records are rejected (R0247). |
| Stdlib | `std::arena`: `Arena::with_capacity`, `alloc_bytes(size, align)`, `alloc(&mut arena, value)`, `reset`, `used`, `remaining`, `count`. Requests larger than the remaining space (including sizes that would overflow a `u64`) return a null pointer. |

## Test suite

`tests/suite/<category>/<name>.ryn` (or a project directory with `src/main.ryn`) is one test. A
`//` header states the expectation; `tests/suite.rs` builds and runs every positive test and checks
stdout, stderr, and the exit status, and requires every negative test to fail `ryn check` with the
named diagnostic code and without a compiler crash. The suite grew from the original 250 to 571
tests: regression tests for every limit that was lifted, plus adversarial tests written by dedicated
testing passes (each reported bug has a minimal reproduction and, once fixed, a test); `EXPECTED_TOTAL` in `tests/suite.rs` pins the count.

| Category | Tests |
| --- | ---: |
| arena | 12 |
| collections | 61 |
| comptime | 84 |
| contracts | 20 |
| ffi | 28 |
| generics | 53 |
| guard | 72 |
| literals | 7 |
| metaprogramming | 24 |
| modules | 41 |
| output | 23 |
| patterns | 58 |
| raii | 46 |
| result_option | 38 |
| structs | 4 |
| **TOTAL** | **571** |

Counted by category directory (`tests/suite/<category>/`, one test per `.ryn` file, and one per
project directory for `modules`).

The task's target distribution was 35 generics and contracts, 25 RAII, 30 Guard, 20 Result and
Option, 30 comptime and metaprogramming, 20 modules, 15 FFI, 20 collections and arena, 15 patterns
and 40 output, diagnostics and regressions. Against that target generics and contracts (73), RAII
(46), Guard (72), Result and Option (38), comptime and metaprogramming (108), modules (41), FFI
(28), collections and arena (73), and patterns (58) are all at or above it; output (23) plus the
`literals` category (7) is still below the 40 asked for output and diagnostics, although
diagnostics are also covered by the `// code:` and `// msg:` checks of the negative tests in every
other category.

Library tests, integration suites, and corpus gates are not part of the 571-program census. The latest full debug run passed 381 Rust tests with `RUST_MIN_STACK=16777216`; this includes the corpus runner and native ownership tests. See [native ownership validation](native-ownership-validation.md) for the bootstrap and memory gates.

Regression tests for the fixed bugs live in the suite: `generics/generic_calls_generic`,
`generics/explicit_args_adapt_integer_literals`, `contracts/duplicate_shape_method_rejected`,
`contracts/unknown_shape_bound_rejected`, `raii/regression_*`, `guard/use_while_mutably_borrowed_rejected`,
`guard/constant_array_index_out_of_bounds_rejected`, `modules/circular_dependency_rejected`,
`modules/private_constant_rejected`, and `ffi/qsort_calls_back_into_ryn`.

Tests for the 0.2 features include `comptime/constant_as_array_length`,
`generics/function_takes_generic_structure`, `patterns/top_level_binding_catches_the_rest`,
`patterns/literal_arms_take_the_function_result_type`, `guard/returning_raw_pointer_to_local_rejected`,
`ffi/sixteen_byte_float_record_to_and_from_c`, `ffi/sixteen_byte_integer_record_from_c`, and
`arena/arena_stores_structures`. `comptime/comptime_conditional_expression` covers a constant
`when` value.

## How the pieces fit

`print`/`eprint`/`write` lower in `sema.rs`. Constants, `comptime`, `static_assert`, metadata
functions, and `derive(Default)` are `src/comptime.rs`; literal and nested patterns are
`src/patterns.rs`; both are syntax-tree passes (`src/visit.rs` is the shared traversal) that run
before semantic analysis in `Frontend::analyze` and in `sema::analyze`, so the Rust and the Ryn
lowering always see the rewritten program. The parsers in Rust (`src/parser.rs`) and Ryn
(`selfhost/src/frontend/parser.ryn`) accept the new syntax identically, and the generic
specializer exists in both languages (`src/generics.rs`, `selfhost/src/frontend/generics.ryn`);
`tests/self_hosted_parity.rs` compares them on every suite program.

A top-level constant is stored as a parameterless function with `external_symbol = "$const"`. A
literal pattern is a `choose` arm with the enum name `$lit`; a payload position that is not a plain
name holds canonical pattern text. Neither needs a new syntax-tree node, so the text encoding of
the tree is unchanged.

## Known limits

Open items, as of this suite:

- Ryn Guard does not fully track raw pointers. It rejects the cases named above (a raw pointer to a
  local returned from a function or stored where it outlives its block, directly or inside a
  structure) but not every lifetime. Raw pointer dereferences are unchecked, pointers kept in
  arrays or reached through calls are not followed, and the arena's pointers dangle after `reset`
  or after the arena is dropped.
- A `when` whose condition is a compile-time constant (or `comptime(...)`) keeps only the taken
  branch, and the dead branch is not type-checked: `when false { missing_function() } else { echo 2 }`
  compiles. A non-constant condition checks both branches as usual.
- The shape-bound check of a generic body covers calls on parameters declared with the type
  parameter itself (`a: T`); locals and `&T` parameters are not checked until specialization.
- `Option::None` or a `Result` constructor as a generic argument cannot infer its type (R0262).
- `300 as u8` is rejected at run time (R0206) but wraps to 44 in a `const`.
- `Result.unwrap()` on an `Err` prints a generic message without the payload.
- Only one attribute is accepted per item (`#[repr(C)]` and `#[derive(Default)]` cannot be combined).
- A fixed array cannot be passed to C by address (`&raw mut arr` is R0206); a `str` literal cannot
  be passed to a `*u8` parameter; top-level `static_assert` is not accepted (use it inside a
  function).
- Variadic C functions (`printf`) cannot be declared with `...`. Cranelift cannot set the System V
  vector-register count that a variadic call needs, so only a separate declaration with the exact
  argument list works, and only for integer and pointer arguments.
- Module system: a transitive path dependency can be imported by the root package; a missing path dependency gives R0425 under
  `ryn check` but R0410 under `ryn build` and `ryn run`; a private struct used from another module
  reports "unknown structure" (R0227) instead of a privacy diagnostic. A struct literal may set
  private fields of a struct from another module (reading them is R0426).
- An empty struct declaration is rejected (R0221).
- A destructor value nested inside another discarded value is not destructed; a raw pointer stored
  in a struct field is not checked after its owner moves.
- With `--frontend rust`, `metaprogramming/metadata_in_generic_function.ryn` fails (R0465) because
  the bootstrap path evaluates constants before specialization; the default frontend passes it.
- Pattern matching: tuple and structure patterns read fields by access, so they do not move owned
  fields out of the scrutinee.
- Compile-time evaluation covers scalars, strings, structures, and `Vec` locals; it cannot call
  functions with side effects, and a `Vec` cannot be a constant. There is no generation of arbitrary
  declarations; `#[derive(Default)]` is the declaration-generating form.
- Destructors still require a raw-pointer first field and scalar, `String`, or pointer fields;
  destructor values cannot be stored in enum payloads.
- Ryn Guard checks borrows lexically: a reference is alive until the end of its scope.
- C records larger than 16 bytes are rejected (R0247). Records of 9 to 15 bytes pass `ryn check` and
  are classified like the 16-byte ones, but only 16-byte records are run against real C code.
  Mixed integer and floating-point eightbytes follow the System V classification and were checked
  against real C for 13 shapes. When an aggregate does not fit the remaining argument registers C
  passes it on the stack; Ryn splits it across registers. C calling Ryn with a 9 to 16 byte record
  through a function pointer is rejected (R0247).
- Linux is the only platform on which the 0.2 tests were run. Windows is untested for this work.
- Not self-hosted yet: Ryn-side diagnostics (`sema.rs` and `guard.rs` still produce every error),
  the native backend (Cranelift is Rust), the linker and runtime, and the CLI are still Rust.
  The lexer, parser, generic specializer, and the lowering of valid programs are Ryn; see
  `docs/self-hosting-plan.md`. `selfhost/src/back/*.ryn` are untested drafts and are not part of the
  build.
