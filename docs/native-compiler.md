# Native Ryn compiler (`rync`)

`rync` is the Ryn-written compiler: its lexer, parser, generic specialization, lowering, analysis, ownership checks,
machine-code generation, ELF and PE linkers and runtime run without Rust, a system linker or libc. It targets Linux
x86-64 and (cross-compiled, see below) Windows x86-64. The `ryn` command (also written in Ryn, `selfhost/src/ryn.ryn`)
starts `rync` for `check`, `build` and `run`; the two programs and the standard library are installed in one directory,
and `ryn` finds `rync` and `stdlib/std/src` next to itself. `build` and `run` run the same analysis as `check`, so they
reject the same programs.

The `ryn-<version>-linux-x86_64.tar.gz` archive contains `ryn`, `rync`, the Ryn sources, the standard-library sources
and the bootstrap script. From the extracted directory:

```sh
./ryn run hello.ryn
./rync --stdlib "$PWD/stdlib/std/src" /absolute/path/to/main.ryn > program    # the compiler alone
```

`rync` writes executable bytes to stdout (or to `-o <path>`) and diagnostics to stderr. Status 0 means successful
compilation, 1 means a frontend error, 2 means invalid command arguments, and 3 means a feature the native pipeline
cannot handle. Module paths resolve relative to the entry file's directory; standard modules resolve under `--stdlib`.
An output that is a link to a source file replaces the link and never writes through it, because the executable is
written to a temporary file beside the output and renamed over it.

## Rebuild the compiler using itself

```sh
./tools/bootstrap-native.sh "$PWD/rync" /tmp/ryn-native-bootstrap
```

This compiles `selfhost/src/rync.ryn` and all the compiler modules it imports,
then recompiles them with that result. Both processes run under a 1 GiB
address-space limit, and the script requires byte-identical stage2/stage3.
No Rust compiler, Cargo, C library or external linker is used in this bootstrap.
The output directory must not already contain bootstrap outputs.

The repository commits its starting point as `bootstrap/rync-seed`, with its digest in
`bootstrap/rync-seed.sha256`. A checkout therefore builds the compiler without Cargo or
rustc (the release archive contains the same file as `rync`):

```sh
env -i PATH=/usr/bin:/bin HOME="$HOME" ./tools/bootstrap-native.sh "$PWD/bootstrap/rync-seed" /var/tmp/ryn-bootstrap
```

The seed is a fixed point of the sources it is committed with: stage2 and stage3 are byte-identical and equal to the
seed. To replace it, run the script with the current seed, check that stage2 and stage3 are byte-identical, then copy
stage2 to `bootstrap/rync-seed` and update the digest in the same change. A compiler change that the old seed cannot
compile (it uses a feature the seed lacks) is built in two steps: the old seed compiles sources that avoid the feature,
and the compiler it makes compiles the full sources.

## Windows x86-64 target

`rync --target windows` writes a PE32+ console executable (`ryn.exe` and `rync.exe` are built this way), and a `rync`
that runs on Windows targets Windows by default. The code generator is the same; what changes is how the program
reaches the system:

- The freestanding runtime's Linux system calls are calls of an emulation routine (`back/winrt.ryn`) over `kernel32`:
  `write`, `read`, `open`, `close`, `lseek`, `mmap`/`munmap`, `exit`, `mkdir`, `rmdir`, `unlink`, `rename`, `chdir`,
  `getcwd`, `stat`, the command line (UTF-8), and a vectored exception handler that prints the same messages as the
  Linux SIGSEGV handler for an access violation or a stack overflow.
- `back/pe.ryn` links the image at a fixed base with import stubs; there are no base relocations.
- Scalar C functions are imported from `ucrtbase.dll` with the Windows calling convention (up to the stack; no records
  by value, no function pointers passed to C).
- `stdlib/std/src/native_windows.ryn` replaces the runtime functions that differ: processes (`CreateProcessW`, pipes),
  directories, time, the executable and temporary paths, the current directory.
- Paths use `/` internally; `C:/...` counts as absolute.

What was verified, and how: nothing here was run on Windows itself. The programs were run under Wine 10 in a
container (`podman`, image `ryn-wine`, `RYN_TEST_TARGET=windows tools/test-native-sources.py`), including the Windows
`rync.exe` compiling programs under Wine (`RYN_TEST_WINE_COMPILER`). Not done yet: threads, sockets, dynamic libraries,
keyboard input, callbacks from C (`qsort`), records passed to or returned from C, directory listing through C
functions, and a run on a real Windows machine.

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

A successful bootstrap does not by itself show parity with the language corpus, so the
corpus is run through the compiler as well. `tools/test-native-sources.py` compiles each
entry of `tests/suite` directly with `rync`, without the Rust frontend or Rust-generated
IR, and checks the executable's stdout, stderr and status, or the rejection's status and
diagnostic. The suite has 631 entries: 474 that must run and 157 that must be rejected.
41 of the entries are project directories rather than single files. With the shipped `rync` and
`PATH=/usr/bin:/bin`, all 631 entries pass.

Programs that declare C functions are linked against the system's shared C library at
run time, so running them depends on that library being present.

## Benchmarks (0.1.6 working tree)

These numbers were measured before the Rust implementation and the Cranelift backend were removed, and are a record of that run, not something the current tree can reproduce. Four small programs, compiled and run with `ryn` 0.1.5 (Cranelift, its only backend) and with the 0.1.6 working tree
(native by default, and `--backend cranelift`). Each run time is the median of five runs after one warm-up run; the
compile times are single measurements. The three configurations printed identical output for every program. The
programs and the comparison script are kept outside the repository (in the local scratch area of this work), so the
numbers are a record of this run, not a reproducible repository benchmark. They are measurements on one machine, not a claim about other workloads.

| program | 0.1.5 compile / run | 0.1.6 native compile / run | 0.1.6 Cranelift compile / run |
|---|---|---|---|
| `loop_sum` (300 M integer iterations) | 0.45 s / 2.24 s | 0.85 s / 2.28 s | 0.44 s / 2.27 s |
| `vec_work` (5 M pushes, then a sum) | 0.41 s / 0.134 s | 0.70 s / 0.117 s | 0.46 s / 0.167 s |
| `map_work` (400 k inserts, 400 k lookups) | 0.52 s / 1.86 s | 0.77 s / 0.46 s | 0.43 s / 1.75 s |
| `string_work` (2 M `push` calls) | 0.44 s / 0.019 s | 0.76 s / 0.031 s | 0.45 s / 0.018 s |

The native backend is faster on the map workload and slower on the string workload, and its compile step takes about
0.8 s against about 0.45 s for the Cranelift path. These numbers are the only benchmark results recorded for 0.1.6.
