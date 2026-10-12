# AArch64 backend (Linux)

`ryn build --target aarch64-linux` and `rync --target aarch64-linux` build a static Linux executable for ARM64
(`e_machine` 183). The backend is deliberately small. It takes the same IR text as the x86-64 backend, and it compiles
only the subset below. Every other construct is refused with an error that names it (`the AArch64 backend does not
handle <what> yet`), so a program is either compiled with the x86-64 backend's meaning or not compiled at all.

## What is compiled

- functions with up to eight parameters; locals, parameters and results of the integer types and `bool`
- statements: block, `let`, assignment, `if`, `while`, `for` over an integer range (`..` and `..=`), `break`, `continue`,
  `return`, a call of a function, `echo` of an integer or a `bool`
- expressions: integer and `bool` literals, locals, `+ - * / %`, `& | ^`, `<< >>`, comparisons, unary `-`, `!`, `~`,
  casts between integers, `if` expressions, calls of functions
- a self tail call (`return f(...)` inside `f`) replaces the parameters and restarts the body, as on x86-64

Refused, with the message naming them: owned values (`String`, `Vec`, ...), addressed locals (references and pointers),
floating-point values, foreign (`extern "C"`) functions, `echo` of strings, and the other statement and expression kinds
that the subset does not list.

## Semantics

The results follow the x86-64 backend:

- integers are kept canonical in 64-bit registers (sign-extended for signed types, zero-extended otherwise); a narrower
  result is normalized after each operation, so arithmetic wraps like the scalar operators
- `x / 0` and `x % 0` print `Ryn runtime error: integer division or remainder by zero` to standard error and exit with
  status 1; the smallest value divided by -1 is the smallest value, and its remainder is zero
- a shift count at or above the width gives zero, or all ones for a signed right shift of a negative value
- recursion deeper than the 7 MiB stack budget prints `Ryn runtime error: stack overflow (recursion is too deep)` and
  exits with status 1
- `echo` writes the value and a newline directly with `write`; there is no buffering
- a program that returns an integer from `main` exits with its low byte; without a result, status 0

## How the code is made

Each local lives in the frame, an expression leaves its value in x0, and a temporary is pushed as a 16-byte unit, so sp
stays 16-byte aligned (Linux checks it at EL0). The encodings are the base of each instruction plus its register and
immediate fields (`selfhost/src/back/arm64.ryn`, constants `OP_*`). The base values were computed from the A64 field
layout and checked against `llvm-mc` (`tools/tests/test_arm64_encodings.py`).

## Verification

- `tools/tests/test_arm64_encodings.py`: every encoding used by the backend is the one `llvm-mc` assembles.
- `tools/tests/test_arm64_run.py` runs the programs under the Unicorn CPU emulator (`tools/tests/arm64_unicorn.py`):
  - the language suite (`tests/suite`, Linux, no stdin): every program the backend accepts must print the expected
    standard output and exit with the expected status. On this tree: **70 of 538 run programs are accepted and all 70
    match**; the other 468 are refused with the reason named above.
  - small programs for the division, narrow-integer and stack-overflow behaviour.
  - Every instruction is checked for a 16-byte aligned sp, except in programs too long for that check (the tail-call
    test runs about 2.5 billion instructions); those are run at emulator speed, and their alignment rests on the
    construction above.

Emulation is not execution on an ARM processor. It shows that the instructions do what the architecture defines, which
is what the backend's code needs; it does not measure speed, and it does not exercise the kernel. The emulator needs the
`unicorn` package (`pip install unicorn`); without it the emulation tests are skipped.

Running the tests needs the compiler under test: `RYN_TEST_RYNC=<rync> python3 tools/tests/test_arm64_run.py`, with a
Python that has `unicorn`.

## Debugging

`RYN_DUMP_IR=<path>` writes the IR text of a program before it is generated (for both backends).
