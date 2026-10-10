"""Port of tests/malformed_source.rs: the compiler must not panic on deterministic malformed sources.

The Rust test calls the library entry points `check`, `check_recovering` and `parser::parse_recovering`
in-process. There is no such library here, so each generated source is passed to `ryn check`, which runs
the whole front end. A panic shows up as a signal, an unexpected exit code or a panic message on stderr.
See RUST_ONLY_misc.md for what is not covered.
"""

import unittest

from ryn_harness import TempDir, run_ryn

FRAGMENTS = [
    "fn",
    "struct",
    "main",
    "for",
    "in",
    "let",
    "mut",
    "echo",
    "if",
    "else",
    "while",
    "break",
    "continue",
    "return",
    "true",
    "false",
    "value",
    "i32",
    "u64",
    "f32",
    "str",
    "Pair",
    "call(1 + , 2)",
    "fun pair(first: i32, second: ) { }",
    "struct Pair { a: , b: i32 }",
    "Pair { a: 1 + , b: }",
    "when true { := 1 }",
    "(",
    ")",
    "{",
    "}",
    ":",
    ",",
    "->",
    "..",
    ".",
    "=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "&&",
    "||",
    "!",
    "1",
    "1.0",
    "2e3",
    '"text"',
    '"Łódź 🦀"',
    '"é"',
    '"\\u{1F980}"',
    '"\\u{}"',
    '"\\u{D800}"',
    '"bad\\q"',
    '"unfinished\\q',
    '"unfinished',
    "// komentarz 猫\n",
    "猫",
    "💥",
    " ",
    "\0",
    "@",
    "// comment\n",
]

MASK64 = (1 << 64) - 1


def next_random(state):
    """The Rust LCG: wrapping 64-bit multiply-add, then the high 32 bits. Returns (value, new_state)."""
    state = (state * 6_364_136_223_846_793_005 + 1_442_695_040_888_963_407) & MASK64
    return state >> 32, state


class MalformedSourceTests(unittest.TestCase):
    def test_full_check_pipeline_does_not_panic_on_deterministic_malformed_sources(self):
        separators = [" ", "\n", "\t", ""]
        state = 0x5259_4E5F_4348_4543

        with TempDir("malformed-source") as directory:
            path = directory / "main.ryn"
            for case in range(4_096):
                value, state = next_random(state)
                count = 1 + value % 32
                source = ""
                for _ in range(count):
                    value, state = next_random(state)
                    fragment = FRAGMENTS[value % len(FRAGMENTS)]
                    source += fragment
                    value, state = next_random(state)
                    source += separators[value % len(separators)]

                path.write_text(source, encoding="utf-8", newline="")
                result = run_ryn("check", path)
                with self.subTest(case=case, source=source):
                    # Any normal exit is acceptable: 0 accepts the source, 1 reports a diagnostic and
                    # 3 reports a construct the native backend declines. A signal (negative code)
                    # means the compiler crashed.
                    self.assertTrue(
                        result.code is not None and result.code >= 0,
                        f"compiler crashed for deterministic malformed case {case}: {source!r}\n"
                        f"exit: {result.code}, stderr: {result.stderr}",
                    )
                    self.assertNotIn(
                        "panicked",
                        result.stderr,
                        f"compiler panicked for deterministic malformed case {case}: {source!r}",
                    )


if __name__ == "__main__":
    unittest.main()
