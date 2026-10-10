"""Port of tests/range_values.rs: run-time checks for range values such as `span := 5..20`.

A range value is a structure with `start`, `end` and `inclusive` fields, and `for` loops over it without a
literal header. The program's exact output is compared. The Rust version also ran it with
`--frontend rust`; that Rust-frontend case is not ported (see RUST_ONLY_misc.md).
"""

import shutil
import unittest

from ryn_harness import PROGRAMS, TempDir, run_ryn


def run_fixture(name):
    source = PROGRAMS / "pass" / name
    with TempDir("range-values") as directory:
        copy = directory / "main.ryn"
        shutil.copyfile(source, copy)
        return run_ryn("run", copy)


class RangeValueTests(unittest.TestCase):
    def test_range_values_hold_bounds_and_iterate_without_overflow(self):
        # Bounds and fields are readable, a range value can be looped twice, an empty
        # range runs no iterations, and `..=` reaches the maximum of `u8` without wrapping.
        result = run_fixture("range_values.ryn")
        self.assertEqual(
            result.code,
            0,
            f"range values should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(
            result.stdout,
            "5\n20\n180\n195\n1\n2\n3\n250\n251\n252\n253\n254\n255\n6\n0\n1\n2\n",
        )


if __name__ == "__main__":
    unittest.main()
