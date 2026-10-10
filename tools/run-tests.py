#!/usr/bin/env python3
"""Runs the cargo-free Ryn CLI test suite (tools/tests) against a ryn binary.

usage: tools/run-tests.py <ryn-binary> [-v]

The binary is exported to the tests as RYN_TEST_BIN. Python 3 standard library only.
"""

import argparse
import os
import sys
import unittest
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the Ryn CLI tests against a ryn binary.")
    parser.add_argument("ryn_binary", help="path to the ryn executable under test")
    parser.add_argument("-v", "--verbose", action="store_true", help="list each test as it runs")
    args = parser.parse_args()

    binary = Path(args.ryn_binary).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        print(f"error: not an executable file: {binary}", file=sys.stderr)
        return 2

    os.environ["RYN_TEST_BIN"] = str(binary)
    tests_dir = Path(__file__).resolve().parent / "tests"
    sys.path.insert(0, str(tests_dir))

    suite = unittest.defaultTestLoader.discover(
        str(tests_dir), pattern="test_*.py", top_level_dir=str(tests_dir)
    )
    print(f"testing {binary}")
    result = unittest.TextTestRunner(verbosity=2 if args.verbose else 1).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
