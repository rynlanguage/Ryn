"""Port of tests/ryn_sema_recovery.rs and the recovering-check cases of tests/compiler_api.rs, through `ryn check`.

Recovery means that one `ryn check` reports every independent error of a file, in source order: a failed statement
changes neither the statements after it nor the functions after its function. The Rust tests ran the bootstrap and the
Ryn analysis; with the Rust analysis gone, the expected lists below are the oracle.

Cases marked GAP are real compiler gaps (see tools/tests/DIAGNOSTIC_GAPS.md): `ryn check` stops after the first error of
a `for` range whose start is invalid. They fail until the compiler reports those errors.
"""

import re
import unittest

from ryn_harness import TempDir, run_ryn

ERROR_LINE = re.compile(r"^error\[(R\d{4})\]: (.*)$", re.MULTILINE)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        temp = TempDir("recovery")
        self.dir = temp.__enter__()
        self.addCleanup(temp.__exit__, None, None, None)
        self.counter = 0

    def errors(self, text):
        """The (code, message) of every error `ryn check` reports for `text`, in order."""
        self.counter += 1
        path = self.dir / f"recovery{self.counter}.ryn"
        path.write_text(text, encoding="utf-8")
        result = run_ryn("check", path)
        self.assertNotEqual(result.code, 0, f"{text!r} must be rejected")
        return ERROR_LINE.findall(result.stderr)

    def assert_reports(self, text, expected):
        """`expected` is a list of (code, message-substring) pairs, checked in order and in full."""
        reported = self.errors(text)
        self.assertEqual(
            [code for code, _ in reported],
            [code for code, _ in expected],
            f"error codes for:\n{text}\nreported: {reported}",
        )
        for (code, message), (_, substring) in zip(reported, expected):
            self.assertIn(substring, message, f"message of {code} for:\n{text}")

    def test_each_failed_statement_is_reported_and_the_next_one_is_still_analyzed(self):
        self.assert_reports(
            "fun main() {\n    echo(first_missing)\n    echo(second_missing)\n}\n",
            [("R0203", "`first_missing`"), ("R0203", "`second_missing`")],
        )

    def test_a_failed_let_hides_its_name_from_the_rest_of_the_block_without_a_second_error(self):
        self.assert_reports(
            "fun main() {\n    value := missing\n    echo(value)\n}\n",
            [("R0203", "`missing`")],
        )

    def test_a_failed_block_does_not_leak_its_locals_into_the_next_function(self):
        self.assert_reports(
            "fun first() {\n    when true {\n        value := missing_one\n    }\n}\n"
            "fun second() {\n    value := 1\n    echo(value)\n}\nfun main() {}\n",
            [("R0203", "`missing_one`")],
        )

    def test_a_failed_return_is_not_followed_by_a_missing_return_error(self):
        self.assert_reports(
            "fun f() -> i32 {\n    return missing\n}\nfun main() {}\n",
            [("R0203", "`missing`")],
        )

    def test_a_loop_body_is_analyzed_after_its_header_has_failed(self):
        self.assert_reports(
            "fun main() {\n    for i in 0..3 {\n        echo(missing_in_loop)\n    }\n"
            "    echo(after_loop_missing)\n}\n",
            [("R0203", "`missing_in_loop`"), ("R0203", "`after_loop_missing`")],
        )

    def test_lexer_errors_are_all_reported(self):
        self.assert_reports("@ @", [("R0005", "'@'"), ("R0005", "'@'")])

    def test_parser_errors_in_independent_declarations_are_all_reported(self):
        self.assert_reports(
            "fun first() { mut := 1 }\nfun second( { }\nfun main() {}",
            [("R0010", "variable name"), ("R0010", "parameter name")],
        )

    def test_independent_by_value_structure_cycles_are_each_reported(self):
        self.assert_reports(
            "struct A { b: B } struct B { a: A } struct C { c: C } fun main() {}",
            [("R0232", "`A`"), ("R0232", "`C`")],
        )

    def test_a_duplicate_loop_variable_does_not_hide_loop_body_errors(self):
        self.assert_reports(
            "fun main() { item := true for item in 0..2 { echo(missing_body) } }",
            [("R0202", "already declared"), ("R0203", "`missing_body`")],
        )

    def test_duplicate_names_do_not_hide_end_bound_or_body_errors(self):
        self.assert_reports(
            "fun main() { item := true for item in 0..missing_end { echo(missing_body) } }",
            [
                ("R0202", "already declared"),
                ("R0203", "`missing_end`"),
                ("R0203", "`missing_body`"),
            ],
        )

    def test_duplicate_names_do_not_hide_independent_range_errors(self):
        self.assert_reports(
            "fun main() { item := true for item in missing_start..2 { echo(missing_body) } }",
            [
                ("R0202", "already declared"),
                ("R0203", "`missing_start`"),
                ("R0203", "`missing_body`"),
            ],
        )

    def test_GAP_an_invalid_range_start_does_not_hide_loop_body_errors(self):
        self.assert_reports(
            "fun main() { for item in missing_start..2 { echo(missing_body) } }",
            [("R0203", "`missing_start`"), ("R0203", "`missing_body`")],
        )

    def test_GAP_a_non_integer_range_start_does_not_hide_loop_body_errors(self):
        self.assert_reports(
            "fun main() { for item in true..2 { echo(missing_body) } }",
            [("R0206", "found `bool`"), ("R0203", "`missing_body`")],
        )

    def test_GAP_both_range_bounds_are_reported_when_the_start_is_invalid(self):
        self.assert_reports(
            "fun main() { for item in missing_start..missing_end { echo(missing_body) } }",
            [("R0203", "`missing_start`"), ("R0203", "`missing_end`")],
        )


if __name__ == "__main__":
    unittest.main()
