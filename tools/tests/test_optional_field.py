"""Port of tests/optional_field.rs: run-time checks for `?.` optional field access.

`value?.field` reads the field of an `Option` payload and yields `Option` of the field type. The program's
exact output is compared. The Rust version also ran it with `--frontend rust`; that Rust-frontend case is
not ported (see RUST_ONLY_misc.md).
"""

import shutil
import unittest

from ryn_harness import PROGRAMS, TempDir, run_ryn


def run_fixture(name):
    source = PROGRAMS / "pass" / name
    with TempDir("optional-field") as directory:
        copy = directory / "main.ryn"
        shutil.copyfile(source, copy)
        return run_ryn("run", copy)


class OptionalFieldTests(unittest.TestCase):
    def test_optional_field_chains_through_options_and_option_payloads(self):
        # A present payload yields its field, `None` stays `None` at any step of the
        # chain, and a String field is cloned out of the payload. `Option::Some(3)` is
        # typed from its payload when no expected type is given.
        result = run_fixture("optional_field.ryn")
        self.assertEqual(
            result.code,
            0,
            f"optional field access should run, stderr:\n{result.stderr}",
        )
        self.assertEqual(result.stdout, "4\n-1\n8\nbox\nnone\n3\ntext\n")


if __name__ == "__main__":
    unittest.main()
