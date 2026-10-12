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

### Calls, registers, inlining and frames (fourth pass of 0.1.9)

A fourth pass went through the code that `rync` generates for a call, a local, a small function and a frame, with the
assembly of `fib`, `collatz` and a sieve next to the output of `rustc -O` and `g++ -O2`. `RYN_NO_OPT=1` still switches all
of it off. What the listings (`ryn asm`) showed, and what was done:

| Item | State | What exists |
|---|---|---|
| Function call overhead | **Done** | A function with at most six one-cell parameters (integers, bools, chars, floats, references, pointers, handles) and no string or structure result gets its arguments in `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9` (the last argument stays in `rax`). Nothing is pushed or popped around the call and the callee does not copy parameters from the stack. Thunks (C to Ryn) move the C arguments into the same registers. Other functions keep the stack convention. |
| Frame zeroing | **Done** | A function with no owner (no `String`, `Vec`, destructor or `repr(C)` copy) no longer zeroes its frame: the zeroing code is written after the function, and five `nop` bytes in the prologue jump to it only when it is needed. Before, every call ran 14 instructions to zero a frame that could be 16 bytes. |
| Register allocation | **Done, simple** | The four most used scalar locals of a function (weighted by loop depth; `r12` to `r15`, which nothing else in the generated code or the runtime uses) live in callee-saved registers. A function saves the registers it uses in its frame on entry and restores them on every return. The frame cell is not written: nothing reads it (`RYN_REGPOISON=1` fills the cells of such locals with a marker, and the suite, the bootstrap and the command-line tests pass with it). Only locals that are written by `let`, assignment, `for` and parameters, and never addressed, are candidates. `x = x + c`, comparisons and `lea` use the registers directly. |
| Inlining | **Extended** | Besides functions of one expression, a function of at most four scalar parameters, eight locals and 40 nodes whose body is `let`, assignment, `if` and `return`, and whose every path returns, is generated at its call (`max`, `clamp`, `sign`, `median`). Its locals get scratch cells of the caller; `return` jumps to the end. Functions with loops or owned values are not inlined. |
| Stack frame | **Done** | A function ending in `return` writes no second epilogue, `leave` replaces `mov rsp, rbp; pop rbp`, a `then` that ends in `return`/`break`/`continue` or an empty `else` has no jump over the `else`. |
| Conditions | **Done** | A comparison is one `cmp` and one conditional jump (`&&`, `||` and `!` become jumps), not `setcc`, `test` and `je`; `x & mask == 0` and `x % 2^k == 0` are one `test`; a loop tests at its bottom (one jump per iteration). |
| Branch-free selection | **Done** | `when c { x = a } else { x = b }` (or no `else`) of an integer local, with a comparison for `c` and small operands that cannot fail, computes both values and picks one with `cmov`: no mispredicted jump. `collatz` mispredicted 61 million branches before and 3 million in C++. |
| Strength reduction | **Extended** | Signed division and remainder by any constant use the multiply-high form of Hacker's Delight (the multiplier and shift are found at compile time); only unsigned and powers of two had it. `x * 2^k` and an element index are a shift. |
| Standard output | **Done (behaviour change)** | On Linux, standard output that is not a terminal is buffered (64 KiB): `echo`, `print` and `io::print` fill the buffer, which is written when it is full, at exit (also after a panic, a runtime error and `exit`), before an error message, before the program reads standard input or starts a process, and on `io::stdout_flush()`. A terminal is not buffered, so prompts and progress show at once, and a program that starts a thread writes at once from then on. 1 000 000 lines took 1.50 s (two `write` calls each) and take 0.037 s. `RYN_NO_OPT=1` and the Windows target keep every write unbuffered. |
| Vectors | **Partly** | `v[i]` and `v.set(i, x)` with a constant or local index and a scalar element no longer push the vector, the index and the value on the stack. Elements are still 8-byte cells: `Vec<u8>` takes 8 times the memory of C++ (the sieve below). |

Instructions executed (`perf stat`, user mode), `fib(40)`, `collatz` below 3 000 000 and the sieve of 20 000 000, in
billions:

| Program | Ryn 0.1.9 first pass | Ryn now | `rustc -O` | `g++ -O2` |
|---|---|---|---|---|
| `fib` | 12.7 | 5.6 | 3.1 | 2.0 |
| `collatz` | 9.2 | 6.9 | 3.9 | 2.3 |
| sieve | 2.6 | 1.6 | 0.42 | 0.41 |

`fib` in assembly: 49 instructions and 219 bytes before, 25 instructions and 104 bytes now (`ryn asm --function fib`).
`g++ -O2` unrolls the recursion, so its `fib` is 207 instructions; `rustc` inlines it into `main`. The inner loop of
`collatz` runs 22 instructions per step on average before (428 million steps, 9.2 billion instructions), 16 now, 9 in
`rustc` (`lea`, `test`, `cmov`) and 8 per iteration in `g++`, which folds `3x+1` into the halving (5.4 per step).

Run time and build time, same program in three languages (one machine, 12 cores, medians of 5 runs; `rync` of this tree,
`rustc 1.99 -O`, `g++ 16.2 -O2`; sources in `benches/versus/`):

| Program | Ryn before this pass | Ryn | Rust | C++ |
|---|---|---|---|---|
| `fib(40)`, run | 1.07 s | 0.67 s | 0.33 s | 0.16 s |
| `collatz`, run | 1.05 s | 0.56 s | 0.45 s | 0.33 s |
| sieve of 20 000 000, run | 0.45 s | 0.39 s | 0.12 s | 0.10 s |
| hash map of 2 000 000 keys, run | 0.75 s | 0.78 s | 0.25 s | 0.30 s |
| sum of 1 048 576 `f32`, 64 passes (`std::simd::sum_f32`), run | 0.009 s | 8.5 ms | 53.1 ms | 14.9 ms |
| 5 000 000 list nodes from `std::alloc::Pool`, run | 0.15 s | 0.14 s | 0.21 s (`Box`) | 0.21 s (`new`) |
| 1 000 000 lines of `echo` / `println!` / `printf` to a pipe, run | 1.50 s | 0.037 s | 0.46 s | 0.068 s |
| `hello`, start and exit | 0.18 ms | 0.19 ms | 0.62 ms | 0.43 ms |
| `hello`, binary size | 8.3 KB | 8.3 KB | 4.3 MB | 16 KB |
| 3000 functions (42 000 lines), build | 2.5 s | 2.5 s | 15.2 s | 13.3 s |
| `hello`, build | 0.10 s | 0.10 s | 0.06 s | 0.04 s |

Ryn is faster at starting, in size, at printing lines, when it builds a large program against an optimizing compiler, when a program uses
the allocators and the SIMD loops of its standard library, and slower on compute-bound code: 1.3x to 4.2x slower than
`g++ -O2` and `rustc -O` on `fib`, `collatz` and the sieve, and 3x on the hash map. The reasons are the ones in the first
sections: one pass, no register allocator for temporaries (only four locals), no loop optimizer, a chained hash map with a
node per entry (every lookup is two cache misses where a flat table has one), and 8-byte vector cells. `rustc` and `g++`
also build with optimizations a Ryn build does not have (`-O0` builds are faster than `rync`: `rustc` debug 1.1 s, `g++ -O0`
2.0 s for the 3000 functions). The `std::simd` result compares an explicit AVX2 loop with a plain `f32` loop: `rustc` cannot
reorder the additions (it prints the same total only because the data is exact in binary), `g++ -O2` vectorizes it, and
`g++ -O3 -march=native -ffast-math` matches Ryn (0.009 s).

Checked: 692 of 692 tests of `tests/suite/` with the compiler built by the committed seed, `tools/bootstrap-native.sh`
gives stage2 == stage3 (sha256), the 260 command-line tests of `tools/run-tests.py` pass with the `ryn` of its distribution,
and `RYN_NO_OPT=1` gives the same output as the optimized build on every new test. The tests of this pass are
`call_arguments_travel_in_registers`, `small_functions_with_statements_are_inlined`,
`locals_in_registers_survive_calls_and_returns` and `conditions_and_loops_keep_their_meaning` in `tests/suite/optimize/`;
the signed division is checked by `strength_reduction_matches_checked_division`. A function that had only a local was
inlined and no longer counts its entries, so the profile test (`tools/tests/test_perf_019.py`) puts a loop in each function.

Not done: a register allocator for temporaries, loop-invariant code motion, a flat hash table, vector elements narrower than 8 bytes,
and recursion unrolling.

### Hash map, loops and structures (fifth pass, 0.1.11)

The fifth pass took the three items that were left from the audit of the fourth (a better register allocator, bounds checks
and loop-invariant work, scalar replacement) and the biggest gap in the table above, the hash map, which was profiled on its own.
`RYN_NO_OPT=1` switches the code-generation items off; the map is a library change and stays.

| Item | State | What exists |
|---|---|---|
| Hash map | **Done** | The map was a table of chains with one node per entry: `malloc` and a cache miss per insert, two cache misses per lookup. It is now a compact dictionary, like the one of CPython: an array of entries (hash, key, value, contiguous) and an open-addressed array of 16-byte slots (hash, entry offset) probed linearly. A lookup compares the hash stored in the slot before it touches the key. Removal moves the last entry into the hole (swap-remove) and shifts the slots back, so there are no tombstones. The scalar hash is a multiply by the golden-ratio constant and an xor-shift. 2 000 000 inserts and lookups: 0.78 s before, 0.35 s now (Rust 0.30 s, C++ 0.36 s). Iteration (`keys`, `values`) still sorts by hash, so the order of keys changed with the hash function; the order was never specified, and the one test that pinned it (`test_map_keys_and_values_iterate_with_clone_and_drop_glue`) has the new order. |
| Global register allocation | **Done** | The four registers `r12` to `r15` were given to the four most used locals of a function for all of it. They are now given by live range: the scan records where each local is first and last used (a local used inside a loop lives to the end of the loop, a parameter lives for the whole function), and two locals whose ranges do not meet share a register, so a function can keep more than four locals in registers. A local that a deferred statement uses lives for the whole function. A function with more than four live locals at one point still keeps the least used in memory. The change is correct (suite, bootstrap, poison) but gave no large measured change on its own: the programs measured here use at most four locals at a time. |
| Bounds-check elimination | **Done for counted loops** | In `while i < v.len() { ... }` and `for i in 0..v.len() { ... }`, `v[i]` and `v.set(i, x)` read the vector without the check when `v` and `i` are plain locals (their address is never taken), the body does not change the length of `v` (any use of `v` but reading its length and elements and setting an element ends the proof), and `i` is not written in the body except by one step `i = i + c` at the top level, after which the proof stops. A signed counter needs a constant start of at least 0. Anything else keeps its check, and `tests/suite/optimize/bounds_check_stays_*` checks that it does. The sieve is not helped: its loops test `j <= n`, which says nothing about the length of the vector. |
| Loop-invariant work | **Partly** | With the check gone, `v[i]` of a scalar vector is `mov rdx, [header]; mov rax, [rdx + i*8]` (the data pointer and the index meet in an addressing mode), `v.set(i, x)` and `v.push(x)` of a scalar do the same and keep the value in a register, and the failure of `set` jumps to one routine at the end of the program instead of being written inside the loop. The header and the data pointer are still loaded in each iteration: hoisting them out of the loop needs to know that nothing in the body can reallocate the vector, which the proof above does not give for a `push`. |
| Register operations | **Done** | `x = x op y` of a 64-bit local in a register is one instruction on the register for `+ - * & \| ^`, `x / 2^k` (unsigned), `x * 2^k` and shifts by a constant, whatever `y` is (the old form needed a constant or a local). `3*x + c`, `5*x + c` and `9*x + c` are one `lea`. A call argument that is a constant, a local or `register + constant` goes straight into its argument register (`lea rdi, [r12-1]`) instead of through `rax`, and `x = cond ? a : b` writes the register from the `cmov` result. |
| Scalar replacement | **Done for structures of scalars** | A local structure whose fields are all integers, bools or chars, that is built by a struct literal and only ever read and written field by field, is replaced by one scalar local per field, and the fields get registers like any local. If the structure is read as a whole (passed, copied, returned, printed), assigned as a whole, or its address or a field's address is taken, all its cells stay in memory. `RYN_REGPOISON=1` proves, for the suite and the bootstrap, that nothing reads a cell whose field lives in a register. A loop that updates three fields of a local structure 400 million times: 0.51 s before, 0.21 s now. |
| Build of a small program | **Done** | Every program was compiled together with `std::native`, the runtime written in Ryn (2700 lines): the loader parsed and decoded it to look for imports, and then every phase read it again, although the lowering is lazy and `hello` reaches none of it. Without it a `hello` builds in 11 ms, with it 105 ms. The loader now takes the module as text (it has no imports and no public alias), and a program under 120 KB of source gets the module with every `__rt_` function that it does not seem to use cut down to its header and a body the lowering rejects (a function is kept when a word of the program matches its name, and the functions that those call are kept). If the lowering reaches a cut function the whole front end runs again with the module as it is, so the result is the same; `RYN_PRUNE_STRESS=1` cuts everything it can, to test that, and the suite passes with it. `hello` now builds in 24 ms (Rust 60, C++ 42). |
| Base case before the frame | **Done** | `when p <op> constant { return p-or-constant }` as the first statement of a function whose parameters arrive in registers is tested and returned before `push rbp`: 4 instructions instead of 12 for the 62 % of the calls of `fib` that are base cases, and the body does not test it again unless the function has a self tail call. `fib(40)` 0.66 s to 0.50 s. |
| Escape analysis | **Not implemented** | Stack allocation of a `Vec`, `String` or `Box` that does not leave the function needs the drops, the moves and the hundred places that read a frame cell directly to know about a header that is not on the heap; there is no intermediate representation to do it on, so it was not started. A scalar structure that does not escape is covered by the scalar replacement above. |

Run time, same machine and method as the table above (medians of 5 runs; `rustc 1.99 -O`, `g++ 16.2 -O2`):

| Program | Ryn 0.1.10 | Ryn 0.1.11 | Rust | C++ |
|---|---|---|---|---|
| `fib(40)`, run | 0.67 s | 0.50 s | 0.33 s | 0.17 s |
| `collatz`, run | 0.56 s | 0.36 s | 0.46 s | 0.34 s |
| sieve of 20 000 000, run | 0.39 s | 0.39 s | 0.14 s | 0.12 s |
| hash map of 2 000 000 keys, run | 0.78 s | 0.35 s | 0.30 s | 0.36 s |
| sum of a 4 000 000 element `Vec<i64>`, 20 passes (`while k < v.len()`), run | 0.129 s | 0.103 s | 0.063 s | 0.086 s |
| 1 000 000 lines of `echo` to a pipe, run | 0.037 s | 0.038 s | 0.48 s | 0.072 s |
| 3000 functions (42 000 lines), build | 2.5 s | 2.6 s | 16.4 s | 14.1 s |
| `hello`, build | 0.105 s | 0.024 s | 0.060 s | 0.042 s |

`collatz` is now faster than `rustc -O` (the halving is one `shr` of the register, `3*x+1` one `lea`, and the choice between
them one `cmov` that writes the register), and the hash map is on the level of `g++` and 15 % behind `rustc`. `fib` and the sieve did
not move: `fib` is bound by the call and the frame (`push rbp`, the save of `r12`, the `leave`), which a frame-pointer-free
convention would change, and the sieve is bound by memory (20 000 000 cells of 8 bytes, 160 MB, against 20 MB in C++), which only
a narrower vector element changes; the instruction count of its loops fell, the time did not.

Checked: 702 of 702 tests of `tests/suite/` with the compiler built by the committed seed and with its own stage2, also with
`RYN_REGPOISON=1`; `tools/bootstrap-native.sh` gives stage2 == stage3 (sha256), also with `RYN_REGPOISON=1`; every new test gives
the same output with `RYN_NO_OPT=1`. New tests in `tests/suite/optimize/`: `loops_over_a_vector_skip_the_bounds_check`,
`bounds_check_stays_when_the_loop_does_not_prove_it`, `bounds_check_stays_when_the_vector_is_cleared`,
`register_updates_shifts_and_scaled_adds_keep_their_meaning`, `structure_fields_live_in_registers_when_the_structure_stays_local`;
in `tests/suite/collections/`: `map_matches_a_vector_model`, which runs 30 000 random inserts, replacements, removals and lookups of integer
and `String` keys against a model made of two vectors and compares them after every step.

Not done: stack allocation of non-escaping heap values, a register allocator for temporaries, hoisting the header and data
pointer of a vector out of a loop, vector elements narrower than 8 bytes (`Vec<u8>` is 8 times larger than in C++), a
frame without `rbp`, shrink-wrapping (saving `r12` only on the path that needs it), and recursion unrolling.

### Not integrated

Reading `vector[index].field` in place (a prototype in the `perf/0.1.8` worktree) made the code generator miscompile
the compiler (the self-built stage grew to 12 GB while lowering), so it is not part of 0.1.9.

### Limits of the verification

* The committed seed (`bootstrap/rync-seed`) is a stage2 of the 0.1.9 release commit. A change of the compiler source
  (the fourth pass above) needs the seed replaced by a stage2 of the new tree, with its checksum, in the same commit:
  the release workflow requires the seed to be the fixpoint of the source. The new tree builds with the committed seed (one
  step) and bootstraps to stage2 == stage3.
* Windows: `rync` and `ryn` build for Windows (`tools/make-dist.py` produces the zip), but nothing was executed there (no Wine
  container was available); lazy lowering also applies to `std::native_windows`.
* `simd_fused_sum` and `simd_sum_scalar` sum the same 1 048 576 f32 values for 64 passes and print the same total.
