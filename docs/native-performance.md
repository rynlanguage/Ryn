# Native performance and systems features

Status of the systems features added after 0.1.8. Everything here is written in Ryn (standard library) or in the
native compiler (`selfhost/src`); there is no Rust, no Cranelift, no garbage collector. "Verified" means a test in
`tests/suite/` passes with the native compiler and the bootstrap fixpoint (stage2 == stage3) still holds.

| Area | Module / command | State |
|---|---|---|
| Bit intrinsics | `std::bits` | verified: popcount, clz, ctz, rotl, rotr, bswap at 8/16/32/64 bits |
| Memory intrinsics | `std::mem` | verified: `copy`, `move_bytes` (overlap safe), `set`, `prefetch` |
| Overflow control | `std::overflow` | verified: `checked_*` (Option), `wrapping_*`, `saturating_*` for add/sub/mul, i8..i64 and u8..u64 |
| Atomics | `std::atomic` | verified: `Atomic<T>` (1/2/4/8-byte integers), `AtomicCell`, orderings, fence; multi-thread test |
| Threads | `std::sync` | verified: `JoinHandle` (argument + result), `Mutex`, `RwLock`, `Condvar`, `Barrier`, `Once`, `Local` (TLS) on pthreads |
| Allocators | `std::alloc`, `std::arena` | verified: `Stack` (mark/rewind), `Pool` (double free detected), `Arena`, custom allocators |
| Memory layout | `#[repr(C, packed)]`, `#[repr(C, packed(N))]`, `#[repr(C, align(N))]`, `size_of`, `align_of`, `offset_of` | verified at compile time and in `sizeof` |
| SIMD | `std::simd` | verified: 10 vector types, SSE2 (128-bit) and AVX2 (256-bit) with runtime dispatch and scalar fallback |
| CPU detection | `std::cpu` | verified against `/proc/cpuinfo`: SSE2/4.1/4.2, POPCNT, BMI1/2, AVX, AVX2, FMA, AVX-512 (with OS state) |
| Assembly explorer | `ryn asm` | verified: listing, `--function`, `--summary`, `--compare`, `--arch`, `--target` |
| Benchmarks | `benches/`, `ryn bench` | present; results below |
| Update | `ryn update` | added; runs the repository installer (not exercised in tests: it downloads) |

## Syntax

```ryn
use std::bits
use std::atomic
use std::sync

echo bits::popcount_u32(255 as u32)          // 8
cell := atomic::AtomicCell::new()             // owns an aligned cell, frees it on drop
counter := Atomic::<u64> { address: cell.address }
counter.fetch_add(1 as u64, atomic::Ordering::SeqCst)

#[repr(C, packed)]
struct Header { tag: u8, count: u32 }
static_assert(size_of::<Header>() == 5)
static_assert(offset_of::<Header>("count") == 1)
```

`Atomic<T>` is a handle (the address of a cell), not the owner: a generic structure cannot have a destructor, so
`AtomicCell` owns and `Atomic<T>` accesses. Build the handle with a literal that names the type, after `use std::atomic`.

## Limits and honest notes

* **x86-64 only.** There is no ARM64 backend, so NEON is not implemented; `std::cpu::has_neon()` is always false and
  `ryn asm --arch aarch64` is an error. Windows: the new intrinsics are plain instructions, but `std::sync` needs
  pthreads (Linux) and none of the new features were run under Wine.
* **The SIMD types are correct, not fast; the fused loops are fast.** The backend has no register allocation: each vector
  operation is a call that moves 16/32 bytes through memory, and only leaf functions of one expression are inlined.
  `benches/simd_sum_*.ryn` shows it: the vector-type versions are *slower* than the scalar loop. The fused array loops
  `std::simd::sum_f32`, `sum_i32`, `dot_f32` and `add_f32` keep the registers across iterations (`benches/simd_fused_sum*.ryn`). `simd<f32, 4>` syntax is not
  provided; the types are `F32x4`, `F64x2`, `I32x4`, `I64x2`, `I16x8`, `I8x16`, `F32x8`, `F64x4`, `I32x8`, `I64x4`.
  Shuffles and reductions are scalar loops. `tools/gen-simd.py` generates `stdlib/std/src/simd.ryn`.
* Operations SSE2 lacks (32-bit integer multiply before SSE4.1, 64-bit compares before SSE4.1/4.2, 8/64-bit multiply)
  run a scalar loop with identical results. A 256-bit operation without AVX2 runs as two 128-bit operations.
* `popcount` is the portable parallel sum, not `popcnt`, so it runs on every x86-64 processor.
* Memory orderings: on x86-64 loads are acquire and stores release; `SeqCst` stores are `xchg`; read-modify-write
  operations are `lock`ed (always at least as strong as requested). `Relaxed` is therefore not weaker in practice.
* `std::sync` primitives hold no data (`Mutex<T>` would be a generic structure with a destructor). Share data by raw
  pointer and protect it by convention. Guards unlock on drop. A shape bound cannot name a module path, so there is no
  standard `Allocator` shape.
* Comptime already had step and depth limits (R0463) and readable errors (R0460-R0467); `size_of`, `align_of`,
  `offset_of` are new. There is no separate memory limit for the evaluator.
* `packed`/`align` apply to `#[repr(C)]` structures; such a record is not passed to C by value (declined).
* There is no general optimizer (no register allocation, no loop optimizations beyond what is listed below), so `ryn asm` has no `-O` option (it says so). `ryn asm` lists functions the program reaches.
* `read_file` returns "" for `/proc` files (it sizes the file first); use `try_read_file`.

## Ryn 0.1.9 "Performance Revolution": the ten optimizations

Honest status. "Done" means implemented, tested in `tests/suite/` or `tools/tests/test_perf_019.py`, and the bootstrap
fixpoint holds. Measurements: one machine (12 cores), medians of 5 runs, "before" is the shipped 0.1.8 `rync-seed`
unless noted.

| # | Optimization | Status | What exists, and what does not |
|---|---|---|---|
| 1 | Arena allocation in the compiler | not implemented | `rync` still uses the C library allocator. Measured after the fixes below: `malloc` + `free` are 16-21 % of a self-compile, and the runtime allocator is about 27 % faster on an allocation-heavy program, so a replacement would gain well under 10 % and mixes allocators across the C boundary. Left out on purpose. |
| 2 | Symbol interning in the compiler | partial | Names are still `String`s. Types, structures, functions, aliases (`lower.ryn`) and drop/glue routines (`gen.ryn`) are found through hash indexes instead of linear scans. This was the quadratic cost: self-compile 152 s to 15 s. No interned symbol type. |
| 3 | Function inlining | done, narrow | A function whose body is one `return` of a small expression over scalar/reference/raw-pointer parameters (at most 32 nodes, leaves may call leaves) is inlined. No inlining of bodies with statements or owned values. `inline_leaf_calls` 1.11 s to 0.15 s. |
| 4 | Profile-guided optimization | done, layout only; no measured gain | Real counts: `--profile-generate FILE` counts function entries, `--profile-use FILE` orders the code hot-first (two generation passes). The profile does not drive inlining or branch layout. Measured: `rync` compiling itself takes 15.0 s with and without a profile taken from the same job (the layout is not what limits it). A missing or damaged profile prints a warning and builds as without one. |
| 5 | Reuse of stack frame slots | not implemented | Every local keeps its own frame cell. Only the cost of zeroing a frame (counted qword loop up to 256 bytes) was reduced. |
| 6 | SROA | partial | A structure literal (or a local) that is returned is written straight into the result cells when every field is one cell. Locals that are structures are not split into scalars. |
| 7 | Constant folding | done | Integer, bool and char expressions of literals fold to one immediate with the exact wrapping/shift/division semantics of the emitted code; `x * 2^k`, `x / 2^k`, `x % 2^k` use shifts and masks. No float folding, no folding across statements. |
| 8 | SIMD auto-vectorization | partial | There is no loop auto-vectorizer. `std::simd::{sum_f32, sum_i32, dot_f32, add_f32}` are fused loops (AVX2 when `std::cpu` reports it, SSE2 otherwise; both give the same bits, and the float reduction order is fixed and documented). `simd_fused_sum` 0.010 s against 0.27 s for the scalar loop. |
| 9 | Parallel compilation | not shipped | A prototype ran the freestanding and the C-library builds in two forked processes (`--jobs N`, byte-identical output). It used `fork`, `pipe` and `waitpid` directly in `rync`, which the Windows build of `rync` cannot compile (`the C library function pipe is not available in a Windows program yet`), and it only helped programs that need the C library. It was removed. A portable version needs the process primitives behind `std::native` and `std::native_windows`; there is no per-function or per-module parallelism. |
| 10 | Link-time optimization | done, whole-program | `rync` always compiles the joined program as one unit, so the optimizations are whole-program: only reachable functions are lowered (native runtime module) and generated, only the runtime routines the code names are written, equal literals share one copy, the object has no padding. There is no separate link step to optimize. |

### Measurements

Compile time (seconds, peak RSS about 20 MB for small programs, 550 MB for the compiler itself, unchanged):

| Program | 0.1.8 seed | 0.1.9 | Binary size 0.1.8 to 0.1.9 |
|---|---|---|---|
| hello | 0.383 | 0.110 | 16 384 to 8 327 B |
| `benches/alloc_pool.ryn` | 0.474 | 0.146 | 20 480 to 12 475 B |
| `benches/inline_leaf_calls.ryn` | 0.434 | 0.118 | 53 248 to 45 546 B |
| Rynix (project) | 0.690 | 0.179 | n/a |
| `std::simd` programs | 2.85 | 0.52 | (the seed cannot build the post-0.1.8 features) |
| `rync` compiling itself | 152.5 | 6.7 after the third pass; 15.3 before it (18.2 with a compiler built by the 0.1.8 seed from the new source, so about 8.4x from the source changes and 1.2x from the new code generator) | 9.83 MB to 9.74 MB |
| bootstrap (stage2 + stage3) | about 305 (2 x 152) | 32 | stage2 == stage3 |

Run time (seconds): `inline_leaf_calls` 1.114 to 0.148; `alloc_pool` 0.579 to 0.218; `alloc_arena` 0.212 to 0.158;
`simd_sum_scalar` 0.286 to 0.270; `simd_fused_sum` 0.010 (AVX2 vector-type loop `simd_sum_avx2`: 0.364); `loop_sum`,
`vec_work`, `map_work`, `string_work`: unchanged within noise. Native against archived 0.1.6 Cranelift binaries of the same sources: `loop_sum` 1.91 against 1.94, `vec_work` 0.043 against 0.090, `map_work` 0.078 against 0.382,
`string_work` 0.012 against 0.006 (the one program where Cranelift is faster); the native executables are 8 KB, the
Cranelift ones 5.7 MB.

### P0 scalar optimizations (second pass of 0.1.9)

A second pass looked at the classic scalar optimizations. `rync` has no optimizing IR: the code generator is a one-pass
stack machine with rax as the accumulator and no register allocator, so an optimization pays only where it removes
instructions of that machine. `RYN_NO_OPT=1` switches the whole pass off (a kill switch to compare behaviour).

| Optimization | State |
| --- | --- |
| Constant folding | Done earlier (integers, bools, chars, power-of-two shifts and masks). |
| Constant propagation | **Done.** A local defined once by a `let` of a constant, never assigned, never addressed or moved, not a parameter, loop counter or match binding, is that constant wherever it is read (`scan_definitions`, `const_local`). |
| Dead code elimination | **Done, narrow.** Statements after `return`, `break`, `continue` in the same list; an `if` whose condition is a constant keeps one branch; an unused scalar `let` whose value cannot fail (no division, call or memory access) is dropped. A division by a zero local still fails at run time. |
| Function inlining | Done earlier, narrow (leaf functions of one expression). |
| Strength reduction | **Done.** Unsigned `/` and `%` by a constant use multiplication by the reciprocal (round-up method, exact for every width); signed `/` and `%` by a power of two use the bias shift; signed `*` by a power of two is a shift; other constant divisors skip the zero and -1 checks; a constant operand that fits 32 bits is an immediate (add, sub, and, or, xor, compare). Checked against the checked division at every width in `tests/suite/optimize/strength_reduction_matches_checked_division.ryn`. |
| Copy propagation | **Not implemented.** With no registers a copy is a load either way, so it removes no instruction. |
| Tail call optimization | **Done for self recursion.** `return f(...)` inside `f` stores the arguments over the parameters and jumps back (constant stack). Only for functions with scalar parameters and no owned values, addressed locals or `repr(C)` copies. A recursion that never ends in a tail call is now an infinite loop, not a stack overflow. Sibling tail calls are not done. |
| Operand handling (not on the list) | A constant or non-addressed local operand of a binary operator is read straight into rcx (or rax) instead of being pushed and popped around the other operand. |
| Loop invariant code motion | **Not implemented.** The only hoistable thing in the hot loops is `v.len()`: one load per iteration, under 5% in the measured loop. |
| Common subexpression elimination | **Not implemented.** Needs a place to keep the value: with a stack machine a repeated load is as cheap as a reload of the saved value. |
| Bounds check elimination | **Done only where it was** (a constant index into an array). A loop-based elimination would remove two instructions of about fifteen per element access, and needs the proof that the vector is not changed in the loop. |
| Escape analysis, stack allocation | **Not implemented.** A `Vec` or `String` buffer moves with the value, grows by `realloc` and is freed by its owner; putting a buffer on the frame needs a proof about all three. |

Measured with the self-built compiler against the 0.1.9 compiler of the first pass (medians of 5 runs; the outputs are
identical with and without `RYN_NO_OPT`):

| Program | First pass | Now |
| --- | --- | --- |
| `loop_sum` (u64 `%` and `*` by constants) | 1.97 s | 0.49 s |
| vector loop, `s += v[i]` | 0.81 s | 0.60 s |
| `inline_leaf_calls` | 0.154 s | 0.106 s |
| `alloc_pool` | 0.226 s | 0.186 s |
| `simd_sum_scalar` | 0.277 s | 0.173 s |
| compiling the compiler | 15.5 s | 15.9 s (6.7 s after the third pass) |

The compiler does not get faster by itself: it is dominated by memory traffic, calls and field access, which these
optimizations do not touch. Bootstrap stage2 == stage3 holds for the new source, peak memory is unchanged (555 MB).
The pass is in `selfhost/src/back/gen.ryn` (analysis, `constant_division`, `tail_call`, `is_stable_operand`).

### Faster compilation of the compiler (third pass of 0.1.9)

Compiling the compiler took 15.5 s after the second pass and takes 6.7 s now (peak memory unchanged, 552 MB). The
profile was taken with `perf` on the compiler compiling itself, with the functions named from the real layout
(`RYN_SYMBOLS=1` makes `rync build` write the offset and name of each function it generated, and each clone and drop
routine, to standard error) and a `malloc` shim that counted the allocation sites. What it showed:

- `vector[index].field` copied the whole element (every `String` and `Vec` it owns), took the field and dropped the
  rest. The compiler does this everywhere (`self.structs[id].field_names.len()`), so lowering spent 9.9 s of 15.5 s in
  copying. A field of a vector element is now read where the element lies, after the index check, and only the field is
  cloned. When the value is only borrowed by a builtin (`len`, `is_empty`, an index, a comparison, `contains_key`, an
  option test, `echo`, `unwrap`) the field is lent: not cloned at all. `vector[index]` of a `String` or a `Vec` that a
  builtin borrows is lent too (`v[i] == name`). Lowering went from 9.9 s to 1.7 s.
- Linear scans replaced by indexes: the drop routine of a handle type, the type of a structure, the duplicate
  declaration check of lowering (it cloned the list of names for every declaration).

Measured on one machine (medians of 5 runs): compiling the compiler 15.5 s to 6.7 s; the bootstrap (stage2 + stage3)
32 s to 23 s; `hello` 0.110 s to 0.103 s; Rynix 0.179 s to 0.146 s. Allocations during the compile: 526 million to 177
million. Compiled with the committed 0.1.8 seed (2 min 35 s), the result bootstraps to a stage2 == stage3.

What is left: 35% of the time is `malloc` and `free`, and a `String` is two allocations (header and bytes). The
lowering writes the typed IR as text and the code generator decodes it again (the token strings are 8% of the
allocations); changing that is a larger redesign. Reading a `String` field of an element for `.len()` directly
(`vector[i].name.len()`) is still rejected by the code generator, as before.

### Not integrated

Reading `vector[index].field` in place (a prototype in the `perf/0.1.8` worktree) made the code generator miscompile
the compiler (the self-built stage grew to 12 GB while lowering), so it is not part of 0.1.9.

### Limits of the verification

* The committed seed (`bootstrap/rync-seed`) is the 0.1.8 compiler and was not touched: `tools/bootstrap-native.sh` started from it
  gives stage2 != stage3 (stage2 is compiled by the old code generator). Started from a compiler built from this tree it gives
  stage2 == stage3 (sha256 identical). Updating the seed is the release step that remains.
* Windows: `rync` and `ryn` build for Windows (`tools/make-dist.py` produces the zip), but nothing was executed there (no Wine
  container was available); lazy lowering also applies to `std::native_windows`.
* `simd_fused_sum` and `simd_sum_scalar` sum the same 1 048 576 f32 values for 64 passes and print the same total.
