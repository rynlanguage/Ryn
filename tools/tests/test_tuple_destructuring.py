"""Port of tests/tuple_destructuring.rs: run-time checks for `(a, b) := value` tuple destructuring.

The declaration is expanded into a temporary plus one `:=` per name, and each program's exact output is
compared. The Rust version also ran every program with `--frontend rust`; that Rust-frontend case is not
ported (see RUST_ONLY_misc.md).
"""

import shutil
import unittest

from ryn_harness import PROGRAMS, TempDir, run_ryn


def run_fixture(name):
    source = PROGRAMS / "pass" / name
    with TempDir("tuple-destructuring") as directory:
        copy = directory / "main.ryn"
        shutil.copyfile(source, copy)
        return run_ryn("run", copy)


def assert_fixture_output(test, name, expected):
    result = run_fixture(name)
    test.assertEqual(
        result.code,
        0,
        f"{name} should run, stderr:\n{result.stderr}",
    )
    test.assertEqual(result.stdout, expected, f"{name} output")


class TupleDestructuringTests(unittest.TestCase):
    def test_tuple_destructuring_binds_names_and_skips_underscores(self):
        # Covers a tuple local, a call result, `mut` bindings, `_` placeholders, and a
        # declaration inside a nested block after an expression statement.
        assert_fixture_output(self, "tuple_destructuring.ryn", "3\n3\nx\n11\nr\n4\nhalf\n4\n")

    def test_struct_destructuring_binds_fields_with_renames_nesting_and_rest(self):
        # Shorthand and renamed fields, `mut` bindings, nested structure and tuple
        # patterns, and `..` that ignores the remaining fields.
        assert_fixture_output(self, "struct_destructuring.ryn", "7\n31\n3\ndeep\nbox\n24\n")

    def test_struct_destructuring_releases_owned_fields_on_every_iteration(self):
        # Each iteration moves a String and a Vec out of a temporary structure; repeated
        # runs would crash or corrupt the total if a field were freed twice.
        assert_fixture_output(self, "struct_destructuring_drops.ryn", "1250125000\n")


if __name__ == "__main__":
    unittest.main()
