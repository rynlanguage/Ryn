# Native float validation — 2026-10-09

The 17 known freestanding float-text declines from `docs/native-ownership-validation.md` are
resolved. Every case below ran without a C library: the native backend links a freestanding
executable containing neither libc nor a startup file, and the object carries no `_start` from
libc. All measurements used an unchanged 1,073,741,824-byte `RLIMIT_AS`.

## What was implemented

- **Embedded float runtime.** `selfhost/src/back/float_runtime_ir.ryn` holds typed IR generated from
  `selfhost/src/float_runtime.ryn` and `middle/floatlib.ryn` by `tools/update-native-float-runtime.py`.
  The generator (`back/gen`) appends that IR to the program it is compiling with `ir::append_ir`, so
  the same Ryn big-integer algorithms that the frontend uses for decimal literals are compiled into
  every freestanding binary. `format_f64`/`format_f32` produce shortest round-tripping text;
  `parse_f64`/`parse_f32` round a decimal rational directly to the requested IEEE width.
- **`std::math` without libm.** `back/math.ryn` emits SSE2 for `sqrt` and x87 for rounding,
  logarithms, trigonometry and finite powers. `pow` classifies zero, infinity, NaN, negative bases
  with fractional exponents, negative integer parity, signed zero and out-of-range exponents before
  the x87 path, which only handles finite positive magnitudes. The x87 control word and register
  stack are restored on every call, including on the early classification exits.
- **Trigonometric reduction.** Arguments with magnitude below 2^63 and finite are reduced with
  `fprem1` in a loop that repeats while the x87 C2 flag stays set; larger or non-finite arguments go
  straight to the hardware instruction, which returns NaN for infinity. This avoids the unbounded
  argument reduction of a single `fprem`.

## Verified gates

- **Former 17 declines, IR pipeline.** The native sweep of `runlist2.txt` plus the nested Result
  regression reports **424 passed, 0 declined**:
  `tests/suite/literals/native_math_functions.ryn`, `native_float_display_shortest.ryn`,
  `tests/suite/modules/std_math_import/src/main.ryn`, `w9_std_modules_combined/src/main.ryn`,
  `tests/suite/result_option/native_float_parse_and_text.ryn`, `w7_try_parse_f64_forms.ryn`,
  `tests/suite/comptime/const_float.ryn`, `const_structure.ryn`,
  `w7_float_display_specials.ryn`, `w7_runtime_cast_sign_matrix.ryn`,
  `tests/suite/output/echo_literals.ryn`, `print_formats_values.ryn`,
  `w4_print_extreme_integers.ryn`, `tests/suite/generics/same_function_two_instantiations.ryn`,
  `tests/suite/metaprogramming/derive_default_scalars.ryn`,
  `w10_meta_derive_default_nested_fields.ryn`, `tests/suite/arena/arena_stores_structures.ryn`.
- **`cargo test --test native_backend`**: 6 tests pass, including
  `float_text_is_freestanding_and_rounds_without_libc`, which requires each float program's object to
  contain no libc `_start` and to link with `ld -static`.
- **Rounding correctness.** `tests/native/freestanding_float_rounding.ryn` checks direct
  decimal-to-`f32` rounding at the subnormal boundary: `1.0000000596046448` rounds to `1.0000001`
  and `1.0000000596046447` to `1`, `7.006492321624086e-46` prints the smallest subnormal,
  `7.006492321624085e-46` prints `0`, and `-1e-10000000` prints `-0` without double rounding
  through `f64`. `try_to_f64` reports `Option::None` for `1e+`, NaN stays NaN, `1e10000000` stays
  infinite, and 1000 parse/format iterations leave live allocation count and live bytes unchanged
  (`0`, `0`).
- **Allocation stability.** `tests/native/freestanding_float_rounding.ryn` continues to hold the
  zero-growth guarantee of `tests/native/ownership_paths.ryn`: 1000 iterations of parse, format,
  `Vec` get/pop and comparison temporaries net zero live allocations and zero live bytes.

## Native bootstrap with the float runtime included

The seed was rebuilt from freshly dumped IR after the generator changes, per the rule recorded in
`native-ownership-validation.md`. `tools/bootstrap-native.sh` compiles the compiler from source and
then recompiles it with that result under the unchanged 1 GiB limit.

| Run | Exit | Peak RSS (KiB) | Seconds |
| --- | ---: | ---: | ---: |
| Fresh seed → stage2 | 0 | 403528 | 58.875130428 |
| Stage2 → stage3 | 0 | 402320 | 58.003734780 |

Stage2 and stage3 are byte-identical, SHA-256 `1beb76cd1625021ad6c9cae5520d1c09677ddbecb499b8878f89a37430f2fcef`.

The float tests also compile and run straight from source with the self-hosted stage3 binary:

| Program | Exit |
| --- | ---: |
| `tests/native/freestanding_float_rounding.ryn` | 0 |
| `tests/native/freestanding_math.ryn` | 0 |
| `tests/suite/literals/native_float_display_shortest.ryn` | 0 |
| `tests/suite/result_option/native_float_parse_and_text.ryn` | 0 |
| `tests/suite/comptime/w7_float_display_specials.ryn` | 0 |

## Direct-source compilation of Ryn's own sources

`rync` compiles its own sources from `.ryn` files with no Rust frontend, no IR dump and no external
linker:

| Run | Exit | Peak RSS (KiB) | Seconds |
| --- | ---: | ---: | ---: |
| stage3 compiles `selfhost/src/rync.ryn` | 0 | 402320 | 58.003734780 |
| stage3 compiles `selfhost/src/main.ryn` | 0 | 332652 | 49.394851132 |
| stage3 compiles `examples/hello.ryn` | 0 | — | — |
| stage3 compiles `tests/native/ownership_30m.ryn` | 0 | — | — |

The compiled 30-million-iteration allocation stress program prints `480000000`, `0`, `0` and uses
676 KiB of address space at peak, matching the Rust-built binary's output byte-for-byte.

The `runlist2.txt` corpus still reports 303 of 424 programs when compiled directly from source. The
remaining cases are frontend features `rync` does not yet implement in its own pipeline: 46 use
compile-time constants, 12 call compile-time functions, 19 need C library imports that a
freestanding executable cannot satisfy, 10 use metaprogramming reflection, 4 use pattern forms the
source pipeline does not lower, 3 fail generic specialization, 3 need module path dependency lookup,
and the rest are single-case diagnostics. The typed-IR pipeline passes all 424, so these are source
pipeline gaps, not code generator gaps. See `native-compiler.md` for the compatibility statement.

## Reproduction

From the repository root with a built release `ryn`:

```sh
cargo build --release --locked
./target/release/ryn build selfhost/src/native.ryn -o /tmp/ryn-float/native
mkdir -p /tmp/ryn-float/dump
RYN_DUMP_SEMA=/tmp/ryn-float/dump ./target/release/ryn check selfhost/src/rync.ryn
/tmp/ryn-float/native /tmp/ryn-float/dump/ryn.ir exe > /tmp/ryn-float/seed
chmod +x /tmp/ryn-float/seed
tools/bootstrap-native.sh /tmp/ryn-float/seed /tmp/ryn-float/fixpoint
python3 tools/test-native-sources.py /tmp/ryn-float/fixpoint/rync-stage2 \
    tests/native/freestanding_float_rounding.ryn tests/native/freestanding_math.ryn \
    tests/suite/literals/native_float_display_shortest.ryn \
    tests/suite/result_option/native_float_parse_and_text.ryn
cargo test --test native_backend
RUST_MIN_STACK=16777216 cargo test
```

To regenerate the embedded runtime after editing `selfhost/src/float_runtime.ryn` or
`middle/floatlib.ryn`, run `python3 tools/update-native-float-runtime.py`; the script re-dumps the
typed IR and rewrites `selfhost/src/back/float_runtime_ir.ryn`, which is committed so end users and
the native bootstrap never need that step.

## Limits

The x87 elementary functions are hardware approximations and are not claimed to match libm
bit-for-bit over every input; large trigonometric arguments have limited reduction precision, and
the `pow` implementation only handles magnitudes whose x87 exponent range the `f2xm1` path accepts,
with everything else classified explicitly. `std::math` is the only standard-library module
verified freestanding; other standard modules that import C functions still require the Rust
backend. Freestanding float formatting follows the Rust-style output already used by the Rust
backend, and both backends were compared on the float corpus in IR mode.
