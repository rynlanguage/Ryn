"""Port of tests/compile_cases.rs: example and pass fixtures must check, fail fixtures must be rejected."""

import unittest

from ryn_harness import EXAMPLES, PROGRAMS, run_ryn


class CompileCaseTests(unittest.TestCase):
    def test_examples_and_pass_fixtures_pass_semantic_checking(self):
        sources = []
        for directory in [EXAMPLES, PROGRAMS / "pass"]:
            sources.extend(path for path in directory.iterdir() if path.suffix == ".ryn")
        sources.sort()
        self.assertTrue(sources, "the pass-program suite must not be empty")

        for source in sources:
            with self.subTest(source=source.name):
                result = run_ryn("check", source)
                self.assertEqual(
                    result.code,
                    0,
                    f"{source} should pass `ryn check`, stderr:\n{result.stderr}",
                )

    def test_fail_fixtures_are_rejected_with_the_expected_diagnostic_codes(self):
        cases = [
            ("unknown_variable.ryn", "R0203"),
            ("operator_type_mismatch.ryn", "R0206"),
            ("malformed_interpolation.ryn", "R0014"),
            ("mut_ref_immutable.ryn", "R0206"),
            ("write_through_shared_ref.ryn", "R0206"),
            ("return_local_reference.ryn", "R0248"),
            ("return_aliased_local_reference.ryn", "R0248"),
            ("ffi_unsupported_record.ryn", "R0247"),
            ("array_repeat_string.ryn", "R0256"),
            ("array_repeat_length.ryn", "R0240"),
            ("array_repeat_bad_length.ryn", "R0012"),
            ("pipe_bad_target.ryn", "R0265"),
            ("coalesce_not_option.ryn", "R0234"),
            ("defer_return_inside.ryn", "R0267"),
            ("defer_break_outside.ryn", "R0268"),
            ("defer_reference.ryn", "R0269"),
            ("defer_owned_moved.ryn", "R0240"),
            ("tuple_destructuring_arity.ryn", "R0225"),
            ("struct_destructuring_wrong_type.ryn", "R0205"),
            ("struct_destructuring_unknown_field.ryn", "R0225"),
            ("optional_field_on_struct.ryn", "R0234"),
            ("range_mixed_types.ryn", "R0205"),
            ("w5_choose_missing_variant.ryn", "R0235"),
            ("w5_choose_literal_no_wildcard.ryn", "R0235"),
            ("w5_choose_unknown_variant.ryn", "R0233"),
            ("w5_unknown_enum_variant.ryn", "R0233"),
            ("w5_missing_struct_field.ryn", "R0229"),
            ("w5_duplicate_struct_field.ryn", "R0228"),
            ("w5_missing_return_value.ryn", "R0213"),
            ("w5_main_wrong_return.ryn", "R0208"),
            ("w5_duplicate_main.ryn", "R0201"),
            ("w5_argument_count.ryn", "R0211"),
            ("w5_argument_type.ryn", "R0212"),
            ("w5_use_after_move.ryn", "R0240"),
            ("w5_unsigned_negation.ryn", "R0206"),
            ("w5_recursive_struct.ryn", "R0232"),
            ("w5_unknown_type.ryn", "R0230"),
            ("shape_bound_unknown_shape.ryn", "R0452"),
        ]
        failures = PROGRAMS / "fail"

        for filename, expected_code in cases:
            with self.subTest(fixture=filename):
                source = failures / filename
                result = run_ryn("check", source)
                self.assertNotEqual(
                    result.code, 0, f"{source} should fail `ryn check`"
                )
                self.assertIn(
                    f"error[{expected_code}]:",
                    result.stderr,
                    f"{source} should report {expected_code}, stderr:\n{result.stderr}",
                )


if __name__ == "__main__":
    unittest.main()
