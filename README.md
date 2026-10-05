# Ryn

**Reliable. Fast. Native.**

Ryn is an early native programming language and compiler implemented in Rust. Version `0.0.1` is a small bootstrap milestone: the compiler checks a compact statically typed language subset and emits native machine code with Cranelift.

## Why Ryn exists

Ryn explores a language with readable source, strong static types, predictable costs, and native performance. The long-term aim is to make ordinary code simple to write while the compiler handles the hard analysis. Ryn is not intended to be Rust with different syntax, and its current bootstrap does not define the eventual memory model or complete language design.

## Status

Ryn is an experimental compiler at version `0.0.1`. The current subset is useful for small programs and compiler experiments; it is not yet a general-purpose production language.

Implemented syntax and behavior include:

- `//` line comments and nestable `/* ... */` block comments.
- Functions, typed parameters and results, function calls and recursion, with early `return` statements or tail-expression returns. `main` may be `fn main()` or `fn main() -> i32`; the latter supplies the native process exit code.
- Immutable `let` bindings, mutable `let mut` bindings, type inference, and signed and unsigned fixed-width integers, `f32`, `f64`, `str`, and `bool`.
- Numeric arithmetic and comparisons, explicit numeric casts with `as`, integer `%`, integer bitwise `~`, `&`, `^`, `|`, `<<`, and `>>`, equality for matching numeric, boolean, string, and structure values (structures compare fields recursively), `!`, and short-circuiting `&&` and `||`.
- `if`/`else if`/`else` statements and value-producing `if` expressions with one value per branch, `while` and exclusive integer-range `for` loops with `break` and `continue`, `print`, simple string interpolation, numeric compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`), and integer bitwise compound assignment (`&=`, `|=`, `^=`, `<<=`, `>>=`) on mutable locals and structure fields.
- String literals support `\0`, `\n`, `\r`, `\t`, `\"`, `\\`, and Unicode scalar escapes in the form `\u{1F980}`. Invalid, surrogate, and out-of-range Unicode values are rejected. Strings carry an explicit length, so an escaped NUL is preserved when printed.
- Structures with named fields, including nested by-value structures, named-field construction, field reads and mutation through `let mut`, and pass/return by value. `print` displays structures in declaration order as `Type { field: value }`, including nested values; strings inside structures are printed without quotes, like standalone strings. Cyclic by-value layouts are not supported yet.
- Process arguments through `arg_count() -> u32` and `arg(index: u32) -> str`. The count excludes the executable path, indexes start at zero, and an out-of-range index returns an empty string; compare with `arg_count()` to distinguish it from a deliberately empty argument. Non-Unicode operating-system arguments are converted lossily to valid UTF-8.
- `ryn new` creates a project with `src/main.ryn`; `ryn check`, native `ryn build`, and `ryn run` accept either a source file or a project directory. Project commands use `src/main.ryn`, and their default executable is written under `build/`. `ryn clean <project-dir>` removes only that project's direct `build/` directory and refuses a symbolic link at that path. Rust library APIs are available through `ryn::check`, `ryn::check_recovering`, `ryn::check_source`, `ryn::check_source_recovering`, `ryn::compile`, `ryn::compile_source`, and the public compiler modules.
- `ryn check` gathers recoverable lexical errors and, when lexing succeeds, recovers between structure fields, function parameters, call arguments, structure literal fields, and statements, inside nested blocks, and across top-level declarations to report independent parser errors together. It also checks control-flow bodies after malformed conditions or range headers. Semantic analysis reports duplicate structure and function declarations together in source order. With unique top-level names, it reports all invalid field declarations, function parameter/result types, and invalid `main` signatures. With valid signatures, it reports all duplicate parameter names and skips affected bodies. Other bodies collect errors across independent statements and nested blocks, including each missing name in a print interpolation, errors in arguments to known or unknown calls (including known calls with the wrong arity), independent field initializer errors in structure literals, and errors from both operands of a binary expression when both can be checked independently. It also checks an assignment's right-hand expression even when its target is invalid, a local initializer when its declaration is invalid, and return expressions when the function has no declared result type. A failed new local declaration stops later statements in that block to avoid cascading unknown-name errors. An invalid `if` or `while` condition does not prevent checking its branches or loop body. Tail-return validation also reports independent errors unless recovery stopped at a failed function-level local declaration. Recovering semantic analysis reports independent recursive by-value structure layout cycles together; other global layout errors stop at the first issue.

Integer literals may be decimal, binary (`0b1010`), or hexadecimal (`0x2a`). Binary and hexadecimal prefixes may use uppercase letters, and `_` may group digits in any integer base. Decimal integers and floating-point literals may also use `_` between digits, including in the fractional part and exponent. Separators at the start or end of a digit sequence, next to a decimal point or exponent marker, or repeated together are rejected. Numeric conversions are explicit: mixed numeric operands are rejected unless converted with an explicit cast. Integer literals use an expected integer type when context provides one, or default to `i64`; floating-point literals use an expected `f32` or `f64`, or default to `f64`. Signed integer unary `-` and integer `+`, `-`, and `*` wrap at the operand width. Signed integer division truncates toward zero; `%` returns the corresponding signed remainder. Unsigned `/` and `%` use unsigned division and remainder. The signed minimum divided by `-1` wraps to that minimum, and its remainder is `0`. Integer `/` or `%` by zero prints a runtime diagnostic to stderr and exits with code `1`. Floating-point arithmetic follows IEEE-754: division by zero can produce infinity or NaN, NaN is unequal to itself, and ordered comparisons with NaN are false. Non-finite floating-point literals are rejected; non-finite runtime results are allowed.
Numeric casts use `value as target`. Integer widening extends the source sign for signed values and fills with zero for unsigned values; integer narrowing keeps the low bits. A cast to a same-width integer type with different signedness preserves the bits and changes their interpretation. Integer-to-float conversions round to nearest, ties to even. `f32` to `f64` is exact; `f64` to `f32` rounds to nearest, ties to even. Float-to-integer conversions truncate toward zero and saturate to the target type's range; NaN converts to zero.

Bitwise `~`, `&`, `^`, and `|` require integer values; binary operands must have the same integer type. Complement and binary operations keep that type's width and signedness. Shifts take an integer value on the left and a `u32` count on the right, and keep the left operand's type. A shift count at least as large as the left type's width produces zero for `<<` and unsigned `>>`; signed `>>` fills with the sign bit. Arithmetic binds more tightly than shifts, shifts bind more tightly than bitwise `&`, then `^`, then `|`; bitwise operators bind more tightly than comparisons, which bind more tightly than logical `&&` and `||`.

During semantic recovery, an invalid `for` start bound does not hide errors in the end bound. If the end bound has an integer type, the loop body is checked using that type as well. A duplicate loop-variable name also does not hide independent range-bound or loop-body errors.

Function arguments, binary operands, and structure field initializers evaluate from left to right in source order. Structure values are then stored, passed, and printed in field declaration order.

## Ryn example

The runnable [quick start](examples/quick_start.ryn) demonstrates a function, inferred and mutable locals, compound assignment, a value-producing conditional, and string interpolation:

```ryn
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let name = "Ryn"
    let mut health = 100

    health -= 20

    let status = if health > 0 { "alive" } else { "down" }
    print(status)

    if health > 0 {
        print("Hello, {name}!")
        print(add(20, 22))
        print(health)
    }
}
```

The [if-expression example](examples/if_expression.ryn) covers typed branch values, nested `else if`, and selected-branch evaluation through the native backend.

The [integer semantics example](examples/integer_semantics.ryn) demonstrates fixed-width wrapping, signed division toward zero, minimum-value division edge cases, and unsigned division and remainder at each integer width.

The [numeric casts example](examples/integer_casts.ryn) demonstrates integer sign extension, zero extension, narrowing, signedness reinterpretation, float conversion and saturation, and converting a computed shift count.

The [bitwise example](examples/bitwise.ryn) demonstrates integer masks, width-preserving complement, shifts, compound flag updates, and operator precedence.

The [range-loop example](examples/for_ranges.ryn) demonstrates exclusive integer ranges, one-time ordered bound evaluation, `break`, `continue`, and an unsigned range near its type maximum.

The [string-escapes example](examples/string_escapes.ryn) demonstrates escaped NUL, quote, backslash, and Unicode characters in native string output.

Write a range loop as `for index in start..end { ... }`. The bounds must have the same integer type. The start is included and the end is excluded; both expressions are evaluated once from left to right before iteration. The immutable `index` binding is available only inside the loop body, and the loop advances it by one.

The [structures example](examples/structs.ryn) shows nested field initialization, reads and updates, pass/return by value, and whole-structure output both directly and inside string interpolation.

The [exit-code example](examples/exit_code.ryn) shows a program returning a nonzero native process status.

The [process-arguments example](examples/arguments.ryn) reads the number of command-line arguments and the first argument.

## Build and run

From the repository root:

```powershell
cargo build
cargo run -- --help
cargo run -- --version
cargo run -- check examples/quick_start.ryn
cargo run -- build examples/quick_start.ryn
cargo run -- run examples/quick_start.ryn
cargo run -- run examples/arguments.ryn -- hello "two words"
cargo run -- build examples/quick_start.ryn --output target/quick-start.exe
```

Create and run a project from its directory:

```powershell
cargo run -- new hello-ryn
Set-Location hello-ryn
cargo run --manifest-path ..\Cargo.toml -- check .
cargo run --manifest-path ..\Cargo.toml -- build .
cargo run --manifest-path ..\Cargo.toml -- run .
```

When using an installed `ryn` executable, the commands from inside the project are simply `ryn check .`, `ryn build .`, and `ryn run .`. The project entry point is `src/main.ryn`; default native output goes to `build/<project-name>` (with `.exe` on Windows). `ryn new` refuses to overwrite an existing path. The generated project intentionally has no dependency manifest yet.

## Install the latest release

The installers download the latest stable GitHub release, verify its SHA-256 checksum, and install the CLI for the current user. Windows uses `install.ps1` and installs to `%LOCALAPPDATA%\Programs\Ryn`; Linux x86_64 uses `install.sh` and installs to `~/.local/bin`. The Bash installer adds that default directory to Bash startup files when needed.

Run the script from a checkout of this repository:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

```bash
bash ./install.sh
```

The repository is currently private. Sign in with `gh auth login` before running an installer, or provide a short-lived `GH_TOKEN`/`GITHUB_TOKEN` with read access to the repository. The scripts also work without a token if the repository is made public. Linux currently publishes x86_64 binaries; other Linux architectures need a matching release asset before they can be installed.

Remove the project's default build artifacts with `ryn clean .`. This command leaves source files and executables written outside `build/` untouched.

`build` and `run` accept `-o` or `--output` to choose the native executable path. `run` keeps the generated executable at that path after it finishes.

When `main` returns `i32`, `ryn run` exits with that status after flushing the program's standard output. A `main` without a result returns process status `0`.

Pass program arguments to `ryn run` after `--`, for example `ryn run app.ryn -- input.txt "two words"`. The separator is required so the CLI can distinguish its own options from arguments for the Ryn program. `arg_count()` excludes the executable name, and `arg(0)` reads the first supplied value.

If linking fails, an existing executable at the output path is left intact. A new executable replaces it only after linking succeeds.

Ryn function code is generated by Cranelift; the compiler does not transpile Ryn into another high-level language. The current native linker driver is `rustc`, with a small host shim for output and string equality.

Native code generation follows Cranelift's host backend support. The build-and-run path is verified on x86_64 Windows; supported Linux hosts include x86_64, aarch64, s390x, and riscv64. `ryn check` works on other Rust targets too, but `ryn build` and `ryn run` report that Cranelift's native backend is unavailable when it has no backend for that host; 32-bit x86 is one such target.

## Compiler architecture

The `ryn` Rust library exposes source diagnostics, lexer, AST, parser, semantic analysis, typed IR, and native code generation. `ryn::check` and `ryn::compile` accept source text; `ryn::check_source` and `ryn::compile_source` accept a `SourceFile` so callers can retain its path, render diagnostics against the original source, and prevent output from overwriting that file. `CompileError::render` produces source excerpts for source errors and plain messages for build errors. `SourceFile::load` reads a file while preserving its path. Both the CLI and library use the same compiler path.

The native backend emits a host object with Cranelift, then links it with the small runtime shim. The compiler and CLI remain in this repository until another ecosystem component has a concrete reason to be independently versioned.

## Tests

Run `cargo test --workspace` for lexer, parser, semantic, diagnostics, malformed-source no-panic, compile-pass, compile-fail, library API, and native end-to-end coverage. Run `cargo test --release --workspace -- --test-threads=4` as well to exercise the full suite against optimized compiler code. Every file in `examples/` and `tests/programs/pass/` is checked with `ryn check`; files in `tests/programs/fail/` must fail with their expected diagnostic codes.

## Benchmarks

Run `cargo bench --bench compiler_pipeline` to measure lexer and parser throughput on existing examples, ordinary and recovering checks on valid source, recovering checks on malformed source, rendering a batch of independent diagnostics, Cranelift object emission, linking with the host runtime, and full native-build time. Each result reports the median and range from five samples. These local measurements are useful for comparing changes on the same machine; they are not performance guarantees.

## Roadmap

- Continue improving diagnostics, source handling, and the reusable compiler API.
- Expand the language deliberately, with tests for each addition and design review before decisions that shape memory safety, errors, generics, or concurrency.
- Consider a standard library, language specification, editor tooling, and Ryn Pods after the compiler foundations justify those separate efforts.

Standard library and Pods work are planned; they are not part of the current compiler.

## Contributing

Contributions should preserve the native Cranelift pipeline and avoid introducing language semantics by accident. Include tests and update examples or documentation when behavior changes. Before submitting changes, run:

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release --workspace -- --test-threads=4
```

## License

The project license has not been selected yet.
