"""The language suite keeps its size: a test file that is deleted or lost fails here, so tests cannot disappear unnoticed.
Raise EXPECTED_TOTAL when entries are added; lower it only when a test is removed on purpose and say why in the change."""
import subprocess
import sys
import unittest

from ryn_harness import REPO_ROOT

EXPECTED_TOTAL = 636


class SuiteInventory(unittest.TestCase):
    def test_the_suite_has_exactly_the_expected_entries(self):
        listing = subprocess.run([sys.executable, str(REPO_ROOT / "tools/list-suite.py")], capture_output=True, text=True, check=True)
        entries = listing.stdout.split()
        self.assertEqual(len(entries), EXPECTED_TOTAL, "the suite must contain exactly %d tests" % EXPECTED_TOTAL)

    def test_every_entry_declares_how_it_is_checked(self):
        listing = subprocess.run([sys.executable, str(REPO_ROOT / "tools/list-suite.py")], capture_output=True, text=True, check=True)
        for entry in listing.stdout.split():
            path = REPO_ROOT / entry
            source = path / "src/main.ryn" if path.is_dir() else path
            head = source.read_text(encoding="utf-8").splitlines()[:1]
            self.assertTrue(head and head[0].startswith("// test:"), f"{entry} has no `// test:` header")


if __name__ == "__main__":
    unittest.main()
