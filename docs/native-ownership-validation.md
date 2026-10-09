# Native ownership validation — 2026-10-09

The selected option is deterministic ownership and moves. No garbage collector is implemented. All reported compiler
and stress runs used a 1,073,741,824-byte `RLIMIT_AS`; this limit was never raised. RSS is measured with a small C
`fork`/`wait4` launcher, so it does not include a Python interpreter before `exec`.

## Verified gates

- Original `runlist2.txt`: 404 passed, 17 known freestanding float-text declines. With the nested Result regression:
  **405 passed, 17 declined**. No regression tests were removed; the new test lives in `tests/native/` and is executed
  by `native_backend`, preserving the unrelated fixed-size suite census.
- `cargo test --test native_backend`: all five tests pass. These include zero growth of live allocation count and
  rounded live payload bytes, nested reference/payload reads, overwrite/remove/clear, loop exit cleanup, early `?`
  cleanup and a repeated-free probe that must terminate with SIGILL.
- `RUST_MIN_STACK=16777216 cargo test`: **381 passed, zero failed, zero ignored**, including the corpus suite and doc test.
  A run with the default Rust test-thread stack aborted with stack overflow in `ryn_sema_projects`; the successful run
  changes thread stack size, not the bootstrap's 1 GiB address limit.
- The integrated 30,000,000-iteration temporary-allocation test passes under 1 GiB and prints `480000000`, `0`, `0`
  (total length, net live allocations, net live bytes). The same test compiled by native stage2 also passes.
- Native stage2 compiles and runs hello, `ownership_paths.ryn` (outputs `0`, `0`) and `nested_result_ownership.ryn`
  (outputs `total 42`). These checks execute generated binaries, not only semantic analysis.

## Native bootstrap

The seed was built from freshly dumped IR after the generator changes, then used to compile `selfhost/src/rync.ryn`.
Reusing an older seed IR embeds the older generator even if the outer driver is current, and is not a valid verification.

| Run | Exit | Peak RSS (KiB) | Seconds |
| --- | ---: | ---: | ---: |
| Native seed → stage2 | 0 | 374388 | 64.091316185 |
| Native stage2 → stage3 | 0 | 370988 | 74.415420584 |
| Native stage2 → driver containing `middle/lower.ryn` | 0 | 158100 | 13.874738671 |
| Rust-built `rync` compiling the same compiler source | 134 | 1045048 | 53.472864471 |

Stage2 and stage3 are byte-identical, with SHA-256:

`2e396bbdb65b1c6a0d058c0bcbe05e85ae9d3e1b32e70f1f49014a6a544f8da4`

The lowerer was compiled through a root entry importing `middle::lower`, because that library source has no main and
its imports resolve from `selfhost/src`. Its hello IR matches `RYN_DUMP_SEMA` byte-for-byte after removing the output
newline. The copied lowerer source was checked byte-identical to the repository file.

The Rust-built full compiler comparison **did not finish under the same 1 GiB limit**: its allocator aborted
(`memory allocation of 3 bytes failed`, exit 134). Consequently there is no claim of a Rust/native full-compiler binary
comparison. Functional output comparisons below and lowerer IR parity did complete. Freestanding float text still has
17 known declines; libc object mode remains available to the native object tests.

## Stress output and memory parity

Every case runs 30,000,000 iterations. Native and Rust stdout files compare byte-for-byte.
The old scratchpad p3 omitted `mut` on a Vec that is pushed to; both measurements here use the same corrected copy.

| Case | Native RSS (KiB) | Rust RSS (KiB) | Native seconds | Rust seconds |
| --- | ---: | ---: | ---: | ---: |
| p1 | 684 | 2728 | 0.699984492 | 1.214602648 |
| p2 | 672 | 2764 | 0.456352213 | 0.779063029 |
| p3 | 704 | 2912 | 1.148210110 | 0.792158458 |
| p4 | 684 | 2856 | 1.067466045 | 2.774441325 |
| p5 | 676 | 2820 | 0.464847495 | 0.873150129 |
| p6 | 672 | 2872 | 1.663839513 | 3.694737192 |
| p7 | 700 | 3008 | 0.904581466 | 2.154230937 |

## Reproduction

From the repository root, build a current native object driver and dump current compiler IR:

```sh
cargo build --release
mkdir -p /tmp/ryn-bootstrap/dump
./target/release/ryn build selfhost/src/native.ryn -o /tmp/ryn-bootstrap/native
RYN_DUMP_SEMA=/tmp/ryn-bootstrap/dump ./target/release/ryn check selfhost/src/rync.ryn
/tmp/ryn-bootstrap/native /tmp/ryn-bootstrap/dump/ryn.ir exe > /tmp/ryn-bootstrap/seed
chmod +x /tmp/ryn-bootstrap/seed
(ulimit -v 1048576; /tmp/ryn-bootstrap/seed selfhost/src/rync.ryn > /tmp/ryn-bootstrap/stage2)
chmod +x /tmp/ryn-bootstrap/stage2
(ulimit -v 1048576; /tmp/ryn-bootstrap/stage2 selfhost/src/rync.ryn > /tmp/ryn-bootstrap/stage3)
cmp /tmp/ryn-bootstrap/stage2 /tmp/ryn-bootstrap/stage3
cargo test --test native_backend
RUST_MIN_STACK=16777216 cargo test
```

Session diagnostics, measured outputs and executables are retained in `/tmp/ryn-ownership-work`. The earlier compiler, documentation and test changes are included with the user's explicit authorization so this snapshot includes the validated dependencies. Generated executables, caches and logs are excluded. This work is on `selfhost/native-ownership`.
