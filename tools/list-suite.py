#!/usr/bin/env python3
"""Prints the entries of tests/suite, one per line: every `.ryn` file, and every directory that holds a project
(`ryn.yaml`). The entries are the arguments of tools/test-native-sources.py."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "tests/suite"


def entries():
    for category in sorted(p for p in SUITE.iterdir() if p.is_dir()):
        for child in sorted(category.iterdir()):
            if child.is_file() and child.suffix == ".ryn":
                yield child
            elif child.is_dir() and (child / "ryn.yaml").is_file():
                yield child


if __name__ == "__main__":
    for entry in entries():
        print(entry.relative_to(ROOT).as_posix())
