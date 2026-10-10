"""Port of tests/vec_of_enum.rs: run-time checks for `Vec` with enum elements.

Each program is compiled and executed, and its exact output is compared, so the element callbacks that
clone, drop, and move enum handles are exercised for real. The Rust version also ran every program with
`--frontend rust`; that Rust-frontend case is not ported (see RUST_ONLY_misc.md).
"""

import shutil
import unittest

from ryn_harness import PROGRAMS, TempDir, run_ryn


def run_fixture(name):
    source = PROGRAMS / "pass" / name
    with TempDir("vec-enum") as directory:
        copy = directory / "main.ryn"
        shutil.copyfile(source, copy)
        return run_ryn("run", copy)


def assert_output(test, name, expected):
    result = run_fixture(name)
    test.assertEqual(
        result.code,
        0,
        f"{name} should run, stderr:\n{result.stderr}",
    )
    test.assertEqual(result.stdout, expected, f"{name} output")


class VecOfEnumTests(unittest.TestCase):
    def test_vec_of_enum_clones_sets_and_pops_elements(self):
        # Element reads, `set`, `clone`, and `pop` each copy or move the enum handle;
        # the snapshot must stay independent of the original vector.
        assert_output(self, "vec_of_enum.ryn", "3\n4\ntag\n9\n9\ndot\ndot\n2\n")

    def test_vec_of_enum_first_last_get_loop_take_and_insert(self):
        assert_output(self, "vec_of_enum_ops.ryn", "2\ndot\nnone\n2\ndot\n2\n2\nx\n")

    def test_vec_of_enum_survives_repeated_clone_and_drop_cycles(self):
        assert_output(self, "vec_of_enum_drops.ryn", "200000\n4000\n")

    def test_vec_of_option_holds_optional_values_and_nested_options(self):
        assert_output(self, "vec_of_option.ryn", "3\n8\n8\n")


if __name__ == "__main__":
    unittest.main()
