# Native Ryn compiler (`rync`)

`rync` is the Ryn-written compiler: its lexer, parser, generic specialization,
lowering, ownership, machine-code generation, ELF linker and runtime run without
Rust, a system linker or libc. Its current executable target is Linux x86_64.
The standard `ryn` command remains the Rust/Cranelift compiler and CLI.

The `rync-0.1.5-linux-x86_64.tar.gz` release archive contains `rync`, its Ryn
sources, the standard-library sources and the native bootstrap script.
From the extracted directory:

```sh
./rync --stdlib "$PWD/stdlib/std/src" /absolute/path/to/main.ryn > program
chmod +x program
./program
```

The compiler writes executable bytes to stdout and diagnostics to stderr.
Status 0 means successful compilation, 1 means a frontend error, 2 means invalid
command arguments, and 3 means a feature the native pipeline cannot handle.
Module paths resolve relative to the entry file's directory; standard modules
resolve under `--stdlib`.

## Rebuild the compiler using itself

```sh
./tools/bootstrap-native.sh "$PWD/rync" /tmp/ryn-native-bootstrap
```

This compiles `selfhost/src/rync.ryn` and all the compiler modules it imports,
then recompiles them with that result. Both processes run under a 1 GiB
address-space limit, and the script requires byte-identical stage2/stage3.
No Rust compiler, Cargo, C library or external linker is used in this bootstrap.
The output directory must not already contain bootstrap outputs.

## Float text and runtime ownership

The native backend embeds typed IR generated from the same Ryn big-integer
algorithms the frontend uses for decimal literals. Both binary32 and binary64
have shortest, round-tripping formatting, with Rust-style fixed decimal output,
`NaN`, `inf`, `-inf` and signed zero. Parsing rounds the decimal rational directly
to the requested IEEE width, including ties to even and subnormal boundaries;
`f32` parsing does not round through `f64` first.

Discarded `Vec::pop/get` results release their Option box and owned payload.
Case conversion and reversal adopt their allocated buffers into the returned
String. Regression tests check that repeated float parsing/formatting leaves
live allocation and byte counts unchanged.

`std::math` can execute without libc: sqrt uses SSE2, and rounding, logarithms,
trigonometry and finite power calculations use x87. These elementary functions
are hardware approximations, not a claim of bit-for-bit libm parity over every
input; very large trigonometric arguments have limited reduction precision.
Power handling includes zero, infinity, NaN, negative integer powers and signed
zero. Finite arguments are reduced for trigonometry with `fprem1` before the
hardware instruction; magnitudes at or above 2^63 and non-finite arguments are
passed directly, since a single reduction cannot bound their range. The
floating-point control word and x87 stack are preserved.

Measured results for these routines, including the subnormal-boundary rounding
cases and the byte-identical stage2/stage3 bootstrap with the runtime embedded,
are recorded in [native float validation](native-float-validation.md).

To regenerate the embedded runtime after editing its Ryn sources, run
`python3 tools/update-native-float-runtime.py` with a built bootstrap `ryn`.
That development step is unnecessary for end users or native compiler bootstrap.

## Compatibility

Successful native compiler bootstrap does not imply complete language-corpus
parity. A direct-source sweep currently passes 303 of 424 programs; outstanding
cases include compile-time evaluation (46), compile-time function calls (12),
C library imports a freestanding executable cannot satisfy (19), metaprogramming
reflection (10), pattern forms (4), generic specialization (3) and module path
dependency lookup (3). The native code generator passes all 424 cases when fed
the existing typed-IR pipeline, including all 17 formerly declined float cases.
`tools/test-native-sources.py` tests executable output and status directly through
`rync`, without using Rust-generated IR. Tests are retained for unsupported
features.
