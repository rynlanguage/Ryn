"""Port of tests/diagnostics.rs: error text, codes, caret rendering and CLI usage via `ryn check`."""

import unittest

from ryn_harness import EXAMPLES, TempDir, expected_version, run_ryn

# Each test gets its own directory; the Rust tests used uniquely named files in the system temp
# directory, and the names only matter where a test asserts on the path (the missing-file test).


class DiagnosticsTests(unittest.TestCase):
    def setUp(self):
        temp = TempDir("diagnostics")
        self.dir = temp.__enter__()
        self.addCleanup(temp.__exit__, None, None, None)

    def write_source(self, text, name="source.ryn"):
        path = self.dir / name
        data = text if isinstance(text, bytes) else text.encode("utf-8")
        path.write_bytes(data)
        return path

    def check(self, text, name="source.ryn"):
        return run_ryn("check", self.write_source(text, name))

    def test_check_command_reports_a_missing_source_file_with_its_path(self):
        source_path = self.dir / "ryn-missing-source.ryn"

        result = run_ryn("check", source_path)

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0001]: cannot read", result.stderr)
        self.assertIn(str(source_path), result.stderr)

    def test_check_command_reports_non_utf8_source_as_a_read_error(self):
        source_path = self.write_source(bytes([0xFF, 0xFE]), "ryn-non-utf8-source.ryn")

        result = run_ryn("check", source_path)

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0001]: cannot read", result.stderr)
        self.assertIn(str(source_path), result.stderr)

    def test_check_command_highlights_a_misplaced_numeric_separator(self):
        result = self.check("fun main() { echo(1__2) }")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0008]: numeric separators must appear between digits",
            result.stderr,
            result.stderr,
        )
        self.assertIn("echo(1__2)", result.stderr, result.stderr)
        self.assertIn("^", result.stderr, result.stderr)

    def test_check_command_highlights_a_digit_invalid_for_the_literal_base(self):
        result = self.check("fun main() { echo(0b2) }")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0009]: digit `2` is not valid in a base-2 integer literal",
            result.stderr,
            result.stderr,
        )
        self.assertIn("use only `0` and `1`", result.stderr, result.stderr)
        self.assertIn("echo(0b2)", result.stderr, result.stderr)
        self.assertIn("^", result.stderr, result.stderr)

    def test_check_command_reports_an_unterminated_block_comment(self):
        result = self.check("fun main() { /* unfinished")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0018]: unterminated block comment", result.stderr, result.stderr
        )
        self.assertIn("close the comment with `*/`", result.stderr, result.stderr)
        self.assertIn("/* unfinished", result.stderr, result.stderr)

    def test_check_command_shows_source_and_caret_for_semantic_errors(self):
        result = self.check("fun main() {\n    echo(missing)\n}")

        self.assertNotEqual(result.code, 0, "invalid source must fail `ryn check`")
        self.assertIn("error[R0203]: unknown variable `missing`", result.stderr)
        self.assertIn(":2:10", result.stderr)
        self.assertIn("2 |     echo(missing)", result.stderr)
        self.assertIn("|          ^~~~~~~", result.stderr)

    def test_check_command_reports_independent_recursive_structure_layout_cycles(self):
        result = self.check(
            "struct A { b: B }\nstruct B { a: A }\nstruct C { c: C }\nfun main() {}\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0232]"), 2, result.stderr)
        self.assertIn("structure `A` contains itself by value", result.stderr)
        self.assertIn("structure `C` contains itself by value", result.stderr)
        self.assertIn(":2:", result.stderr)
        self.assertIn(":3:", result.stderr)

    def test_check_command_renders_diagnostics_for_lone_cr_line_endings(self):
        result = self.check("fun main() {\r value := missing\r}\r")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0203]: unknown variable `missing`", result.stderr)
        self.assertIn(":2:11", result.stderr)
        self.assertIn("2 |  value := missing", result.stderr)
        self.assertIn(f"| {' ' * 10}^~~~~~~", result.stderr)

    def test_check_command_reports_multiple_independent_syntax_errors(self):
        result = self.check("fun first() { mut := 1 }\nfun second( { }\nfun main() {}")

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 2, result.stderr)
        self.assertIn("expected variable name", result.stderr)
        self.assertIn("expected parameter name", result.stderr)
        self.assertIn(":1:19", result.stderr)
        self.assertIn(":2:13", result.stderr)

    def test_check_command_reports_syntax_errors_inside_nested_blocks_together(self):
        result = self.check(
            "fun main() {\n    mut := 1\n    when true {\n        echo(1 + )\n        mut := 2\n        echo(3)\n    }\n    mut := 4\n}\nfun other() { echo(5 + ) }"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 3, result.stderr)
        self.assertEqual(result.stderr.count("error[R0012]"), 2, result.stderr)
        self.assertEqual(result.stderr.count("expected variable name"), 3, result.stderr)
        self.assertEqual(result.stderr.count("expected expression"), 2, result.stderr)
        self.assertIn(":2:9", result.stderr)
        self.assertIn(":4:18", result.stderr)
        self.assertIn(":10:24", result.stderr)

    def test_check_command_reports_body_errors_after_malformed_control_flow_headers(self):
        result = self.check(
            "fun main() { when ) { mut := 1 echo(2 + ) } while { mut := 3 } for index in 0.. { mut := 4 echo(5 + ) } for index in 0.. when true { 6 + } else { 7 } { mut := 8 } while when true { 9 + } else { true } { mut := 10 } for index in 0.. when ) { 11 } else { 12 } { mut := 13 } while when ) { true } else { true } { mut := 14 } }"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 7, result.stderr)
        self.assertEqual(result.stderr.count("error[R0012]"), 9, result.stderr)
        self.assertEqual(result.stderr.count("expected variable name"), 7, result.stderr)
        self.assertEqual(result.stderr.count("expected expression"), 9, result.stderr)

    def test_check_command_reports_semantic_range_errors_and_body_errors_together(self):
        result = self.check(
            "fun main() {\n    item := true\n    for item in missing_start..2 {\n        echo(missing_body)\n    }\n}\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0202]"), 1, result.stderr)
        self.assertEqual(result.stderr.count("error[R0203]"), 2, result.stderr)
        self.assertIn("`item` is already declared in this scope", result.stderr)
        self.assertIn("unknown variable `missing_start`", result.stderr)
        self.assertIn("unknown variable `missing_body`", result.stderr)
        self.assertIn(":3:9", result.stderr)
        self.assertIn(":3:17", result.stderr)
        self.assertIn(":4:14", result.stderr)

    def test_check_command_reports_structure_field_syntax_errors_together(self):
        result = self.check(
            "struct Config { good: i32, broken: , next: str, : bool, last: f64 } fun main() {}"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 2, result.stderr)
        self.assertIn("expected type name", result.stderr)
        self.assertIn("expected field name", result.stderr)

    def test_check_command_reports_function_parameter_syntax_errors_together(self):
        result = self.check(
            "fun broken(a i32, : bool, c: ) -> i32 { echo(1 + ) 1 } fun main() {}"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 3, result.stderr)
        self.assertEqual(result.stderr.count("error[R0012]"), 1, result.stderr)
        self.assertIn("expected `:` after parameter name", result.stderr)
        self.assertIn("expected parameter name", result.stderr)
        self.assertIn("expected type name", result.stderr)
        self.assertIn("expected expression", result.stderr)

    def test_check_command_reports_call_and_structure_literal_errors_together(self):
        result = self.check(
            "struct Pair { a: i32, b: i32, c: i32 } fun combine(a: i32, b: i32, c: i32) {} fun main() { echo(combine(1 + , 2 + , 3)) echo(combine(1 2)) pair := Pair { a: 1 + , b: 2 + , c: } other := Pair { a: 1 b: 2 } }"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0010]"), 2, result.stderr)
        self.assertEqual(result.stderr.count("error[R0012]"), 5, result.stderr)
        self.assertEqual(result.stderr.count("expected expression"), 5, result.stderr)
        self.assertEqual(result.stderr.count("expected `,`"), 2, result.stderr)

    def test_check_command_reports_multiple_independent_lexical_errors(self):
        result = self.check("@ fun main() { 💥 }")

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0005]"), 2, result.stderr)
        self.assertIn(":1:1", result.stderr)
        self.assertIn(":1:16", result.stderr)

    def test_check_command_reports_semantic_errors_from_independent_functions(self):
        result = self.check(
            "fun first() {\n    echo(missing_first)\n}\nfun second() {\n    echo(missing_second)\n}\nfun main() {}\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0203]"), 2, result.stderr)
        self.assertIn("`missing_first`", result.stderr)
        self.assertIn("`missing_second`", result.stderr)
        self.assertIn(":2:10", result.stderr)
        self.assertIn(":5:10", result.stderr)

    def test_check_command_reports_all_duplicate_global_declarations(self):
        result = self.check(
            "struct First { value: i32 }\nfun duplicate() {}\nstruct First { value: i32 }\nfun main() {}\nfun duplicate() {}\nstruct Second { value: i32 }\nstruct Second { value: i32 }\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0220]"), 2, result.stderr)
        self.assertEqual(result.stderr.count("error[R0201]"), 1, result.stderr)
        first_structure = result.stderr.find("error[R0220]")
        function = result.stderr.find("error[R0201]")
        second_structure = result.stderr.rfind("error[R0220]")
        self.assertTrue(
            first_structure < function < second_structure, result.stderr
        )

    def test_check_command_reports_independent_structure_declaration_errors(self):
        result = self.check(
            "struct First { value: Missing, other: MissingToo }\nstruct Second { value: i32, value: MissingAgain }\nstruct Empty {}\nfun main() {}\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertEqual(result.stderr.count("error[R0230]"), 3, result.stderr)
        self.assertEqual(result.stderr.count("error[R0222]"), 1, result.stderr)
        self.assertEqual(result.stderr.count("error[R0221]"), 1, result.stderr)
        unknown_type = result.stderr.find("error[R0230]")
        duplicate_field = result.stderr.find("error[R0222]")
        empty_structure = result.stderr.find("error[R0221]")
        self.assertTrue(
            unknown_type < duplicate_field < empty_structure, result.stderr
        )

    def test_check_command_still_accepts_a_valid_program_with_recovery_enabled(self):
        source_path = EXAMPLES / "hello.ryn"

        result = run_ryn("check", source_path)

        self.assertEqual(result.code, 0, f"valid source failed `ryn check`: {result.stderr}")
        self.assertIn(str(source_path), result.stdout)
        self.assertTrue(result.stdout.rstrip().endswith(": ok"), result.stdout)

    def test_check_command_explains_the_required_main_signature(self):
        result = self.check("fun main(value: i32) {}")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0208]: `main` must take no parameters and return either no value or `i32`",
            result.stderr,
        )
        self.assertIn(
            "help: use `fun main() { ... }` or `fun main() -> i32 { ... }`", result.stderr
        )

    def test_check_command_highlights_the_incompatible_operator_operand(self):
        result = self.check(
            "fun main() {\n    left: i32 = 1\n    right: i64 = 2\n    echo(left + right)\n}\n"
        )

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0206]: arithmetic operands must have matching numeric types, found `i32` and `i64`",
            result.stderr,
        )
        self.assertIn("4 |     echo(left + right)", result.stderr)
        self.assertIn(f"  | {' ' * 16}^~~~~", result.stderr)
        self.assertIn(
            "help: use matching numeric types; Ryn does not implicitly convert numeric values",
            result.stderr,
        )

    def test_check_command_shows_actionable_help_for_immutable_assignment(self):
        result = self.check("fun main() {\n    value := 1\n    value = 2\n}")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0204]: `value` is immutable", result.stderr)
        self.assertIn(
            "help: declare `mut value := ...` if this binding must be mutable",
            result.stderr,
        )

    def test_check_command_suggests_the_required_type_for_a_bad_argument(self):
        result = self.check('fun take(value: i32) {}\nfun main() {\n    take("wrong")\n}')

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0212]: argument to `take` has type `str` but `i32` is required",
            result.stderr,
        )
        self.assertIn("  |          ^~~~~~~", result.stderr)
        self.assertIn("help: pass a `i32` value to `take`", result.stderr)

    def test_check_command_suggests_a_boolean_condition(self):
        result = self.check("fun main() { when 1 { echo(1) } }")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0207]: `when` condition must have type `bool`", result.stderr
        )
        self.assertIn(
            "help: use a boolean expression, such as a comparison, for the condition",
            result.stderr,
        )

    def test_check_command_shows_the_valid_integer_literal_range(self):
        result = self.check("fun main() { value: i8 = 128 }")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0206]: integer literal is outside the `i8` range", result.stderr
        )
        self.assertIn(
            "help: use a value from `-128` to `127`, or choose a wider integer type",
            result.stderr,
        )

    def test_check_command_suggests_a_close_variable_name(self):
        result = self.check("fun main() { mut count := 1 echo(cout) }")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0203]: unknown variable `cout`", result.stderr)
        self.assertIn("help: did you mean `count`?", result.stderr)

    def test_check_command_suggests_process_argument_builtins(self):
        result = self.check("fun main() { arg_coun(0) }")

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0210]: unknown function `arg_coun`", result.stderr, result.stderr
        )
        self.assertIn("help: did you mean `arg_count`?", result.stderr, result.stderr)

    def test_check_command_suggests_a_close_variable_name_for_assignment_targets(self):
        result = self.check("fun main() { mut count := 1 cout += 2 }")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0203]: unknown variable `cout`", result.stderr)
        self.assertIn("help: did you mean `count`?", result.stderr)

    def test_check_command_suggests_close_structure_field_names(self):
        cases = [
            (
                "struct Point { width: i32 height: i32 }\nfun main() { point := Point { widht: 10, height: 20 } }",
                "structure `Point` has no field `widht`",
                "did you mean `width`?",
            ),
            (
                "struct Point { width: i32 height: i32 }\nfun main() { point := Point { width: 10, height: 20 } echo(point.heigth) }",
                "structure `Point` has no field `heigth`",
                "did you mean `height`?",
            ),
            (
                "struct Point { width: i32 } struct Bounds { min: Point }\nfun main() { mut bounds := Bounds { min: Point { width: 0 } } bounds.min.widht = 1 }",
                "structure `Point` has no field `widht`",
                "did you mean `width`?",
            ),
        ]

        for index, (source, expected_error, expected_help) in enumerate(cases):
            with self.subTest(case=index):
                result = self.check(source, f"ryn-structure-field-{index}.ryn")

                self.assertNotEqual(result.code, 0)
                self.assertIn(f"error[R0225]: {expected_error}", result.stderr, result.stderr)
                self.assertIn(f"help: {expected_help}", result.stderr, result.stderr)
                if index == 2:
                    self.assertIn(
                        "^~~~",
                        result.stderr,
                        f"the diagnostic should highlight only the misspelled field: {result.stderr}",
                    )

    def test_check_command_reports_truncated_function_declarations_without_panicking(self):
        result = self.check("fun")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0010]: expected function name", result.stderr)
        self.assertNotIn("panicked", result.stderr)

    def test_check_command_reports_excessive_nesting_without_panicking(self):
        source = "fun main() { echo(" + "(" * 129 + "true" + ")" * 129 + ") }"

        result = self.check(source)

        self.assertNotEqual(result.code, 0)
        self.assertIn(
            "error[R0015]: maximum parser nesting depth exceeded in expression",
            result.stderr,
        )
        self.assertIn("help: reduce the amount of nesting in expressions", result.stderr)
        self.assertNotIn("panicked", result.stderr)

    def test_check_command_rejects_loop_control_outside_a_while_loop(self):
        for keyword, code in [("break", "R0016"), ("continue", "R0017")]:
            with self.subTest(keyword=keyword):
                result = self.check(f"fun main() {{ {keyword} }}")

                self.assertNotEqual(result.code, 0)
                self.assertIn(
                    f"error[{code}]: `{keyword}` can only be used inside a loop",
                    result.stderr,
                )
                self.assertIn(
                    f"help: move `{keyword}` into the body of a `while` or `for` loop",
                    result.stderr,
                )

    def test_check_command_suggests_valid_expression_forms(self):
        result = self.check("fun main() {\n    value :=\n}\n")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0012]: expected expression", result.stderr)
        self.assertIn(
            "help: start with a literal, variable, function call, unary operator, or `(`.",
            result.stderr,
        )

    def test_unknown_cli_command_shows_usage_without_reading_the_input_file(self):
        missing_path = self.dir / "ryn-unknown-command.ryn"

        result = run_ryn("wat", missing_path)

        self.assertNotEqual(result.code, 0)
        self.assertIn("usage:\n  ryn new <path>", result.stderr)
        self.assertIn("ryn check <file.ryn|project-dir>", result.stderr)
        self.assertIn(
            "ryn build <file.ryn|project-dir> [--release] [-o|--output <path>]",
            result.stderr,
        )
        self.assertIn(
            "ryn run <file.ryn|project-dir> [--release] [-o|--output <path>]",
            result.stderr,
        )
        self.assertIn("ryn clean <project-dir>", result.stderr)
        self.assertNotIn("error[R0001]", result.stderr)

    def test_help_option_lists_commands_and_exits_successfully(self):
        result = run_ryn("--help")

        self.assertEqual(result.code, 0)
        stdout = result.stdout
        self.assertIn("new <path>", stdout)
        self.assertIn("check <source>", stdout)
        self.assertIn("build <source>", stdout)
        self.assertIn("run <source>", stdout)
        self.assertIn("clean <project>", stdout)
        self.assertIn("src/main.ryn", stdout)
        self.assertIn("i32 result from main becomes the process exit code for run", stdout)
        self.assertIn("-o, --output <path>", stdout)
        self.assertIn("-V, --version", stdout)
        self.assertEqual(result.stderr, "")

    def test_output_option_is_rejected_for_check_and_when_missing_its_path(self):
        source = self.write_source("fun main() {}", "fixture.ryn")

        check = run_ryn("check", source, "-o", self.dir / "ignored")
        missing_path = run_ryn("build", source, "--output")

        for result in [check, missing_path]:
            stderr = result.stderr
            self.assertNotEqual(result.code, 0)
            self.assertIn("ryn new <path>", stderr, stderr)
            self.assertIn("ryn check <file.ryn|project-dir>", stderr, stderr)
            self.assertIn(
                "ryn build <file.ryn|project-dir> [--release] [-o|--output <path>]",
                stderr,
                stderr,
            )
            self.assertIn(
                "ryn run <file.ryn|project-dir> [--release] [-o|--output <path>]",
                stderr,
                stderr,
            )
            self.assertIn("ryn clean <project-dir>", stderr, stderr)

    def test_version_option_reports_the_package_version(self):
        result = run_ryn("--version")

        self.assertEqual(result.code, 0)
        self.assertEqual(result.stdout.strip(), f"ryn {expected_version()}")
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
