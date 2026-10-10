"""Port of tests/syntax_extensions.rs: run-time checks for the `|>` pipe, the `??` fallback and defer.

Both the pipe and the fallback are rewritten by the parser into existing calls and `choose` expressions,
so these tests compile and execute real programs and compare their exact output. The Rust version also
ran several programs with `--frontend rust`; those Rust-frontend cases are not ported, and
`bootstrap_frontend_matches_the_expected_output` (which only ran the Rust frontend) is not ported either.
See RUST_ONLY_misc.md.
"""

import unittest

from ryn_harness import PROGRAMS, TempDir, run_ryn


def run_program(source_text):
    with TempDir("syntax-extensions") as directory:
        source = directory / "main.ryn"
        source.write_text(source_text, encoding="utf-8")
        return run_ryn("run", source)


def read_pass_fixture(name):
    return (PROGRAMS / "pass" / name).read_text(encoding="utf-8")


PIPE_AND_COALESCE_OUTPUT = "36\n100\n28\n41\n0\n5\n"


class SyntaxExtensionTests(unittest.TestCase):
    def test_pipe_and_coalesce_run_with_the_expected_output(self):
        result = run_program(read_pass_fixture("pipe_and_coalesce.ryn"))
        self.assertEqual(
            result.code,
            0,
            f"pipe and coalesce program should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, PIPE_AND_COALESCE_OUTPUT)

    def test_coalesce_evaluates_the_fallback_only_for_none(self):
        source = """
fun fallback_value(counter: i32) -> i32 {
    echo "fallback"
    return counter
}

fun main() -> i32 {
    present: Option<i32> = Option::Some(3)
    missing: Option<i32> = Option::None
    echo present ?? fallback_value(9)
    echo missing ?? fallback_value(11)
    return 0
}
"""
        result = run_program(source)
        self.assertEqual(
            result.code,
            0,
            f"lazy coalesce should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, "3\nfallback\n11\n")

    def test_pipe_is_left_associative_and_binds_loosest(self):
        source = """
fun inc(x: i32) -> i32 => x + 1

fun main() -> i32 {
    echo 1 + 2 |> inc
    echo 1 |> inc |> inc
    return 0
}
"""
        result = run_program(source)
        self.assertEqual(
            result.code,
            0,
            f"pipe precedence program should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, "4\n3\n")

    def test_inclusive_ranges_include_their_end_and_never_wrap(self):
        source = read_pass_fixture("inclusive_ranges.ryn")
        expected = "10\n6\n256\n-3\n-2\n-1\n5\n"
        result = run_program(source)
        self.assertEqual(
            result.code,
            0,
            f"inclusive ranges should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, expected)

    def test_defer_runs_on_every_exit_in_reverse_order(self):
        source = read_pass_fixture("defer_blocks.ryn")
        expected = (
            "body\n"
            "second registered\n"
            "first registered\n"
            "7\n"
            "cleanup sees 99\n"
            "1\n"
            "0\n"
            "end iteration\n"
            "end iteration\n"
            "2\n"
            "end iteration\n"
            "left loop\n"
            "1\n"
        )
        result = run_program(source)
        self.assertEqual(
            result.code,
            0,
            f"defer blocks should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, expected)

    def test_defer_keeps_owned_values_alive_until_the_block_ends(self):
        source = read_pass_fixture("defer_owned_values.ryn")
        result = run_program(source)
        self.assertEqual(
            result.code,
            0,
            f"owned defer program should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, "body\nowned\n")

    def test_question_mark_runs_pending_defer_blocks_on_its_early_return(self):
        option = read_pass_fixture("defer_propagate_option.ryn")
        result = read_pass_fixture("defer_propagate_result.ryn")

        option_run = run_program(option)
        self.assertEqual(
            option_run.code,
            0,
            f"option propagation with defer should run, stderr:\n{option_run.stderr}",
        )
        self.assertEqual(option_run.stdout, "cleanup\n5\ncleanup\n-1\n")

        result_run = run_program(result)
        self.assertEqual(
            result_run.code,
            0,
            f"result propagation with defer should run, stderr:\n{result_run.stderr}",
        )
        self.assertEqual(result_run.stdout, "result cleanup\n11\nresult cleanup\n9\n")


if __name__ == "__main__":
    unittest.main()
