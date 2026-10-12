"""Ryn 0.1.9 compiler optimizations seen from the command line of `rync` (RYN_TEST_RYNC names the compiler):
profile-guided code layout (`--profile-generate`, `--profile-use`), deterministic output, lazy lowering and the phase timings.
"""

import hashlib
import os
import subprocess
import unittest
from pathlib import Path

from ryn_harness import TempDir

REPO_ROOT = Path(__file__).resolve().parents[2]
STDLIB = REPO_ROOT / "stdlib" / "std" / "src"
TIMEOUT = 120

# A hot function (called often), a warm one and a cold one, so that a profile has something to order.
# Each function has a loop, so none of them is inlined (a small function of statements would be) and each one counts its
# real entries.
PROGRAM = """fun cold(n: i64) -> i64 {
    mut r: i64 = n
    mut once: i64 = 0
    while once < 1 {
        r += 1
        once += 1
    }
    return r
}

fun warm(n: i64) -> i64 {
    mut r: i64 = n * 3
    mut once: i64 = 0
    while once < 1 {
        r += cold(n)
        once += 1
    }
    return r
}

fun hot(n: i64) -> i64 {
    mut r: i64 = n % 7
    mut once: i64 = 0
    while once < 1 {
        r += 1
        once += 1
    }
    return r
}

fun main() {
    mut sum: i64 = 0
    mut i: i64 = 0
    while i < 20000 {
        sum += hot(i)
        when i % 1000 == 0 {
            sum += warm(i)
        }
        i += 1
    }
    echo sum
}
"""


def rync() -> Path:
    override = os.environ.get("RYN_TEST_RYNC")
    if not override:
        raise unittest.SkipTest("set RYN_TEST_RYNC to a native rync")
    return Path(override)


def run(args, **kwargs):
    return subprocess.run([str(rync()), *args], capture_output=True, timeout=TIMEOUT, **kwargs)


def build(source, output, *extra):
    return run(["build", "--stdlib", str(STDLIB), *extra, str(source), "-o", str(output)])


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class PerfTests(unittest.TestCase):
    def setUp(self):
        self.temp = TempDir("perf019")
        self.dir = self.temp.__enter__()
        self.source = self.dir / "work.ryn"
        self.source.write_text(PROGRAM, encoding="utf-8")

    def tearDown(self):
        self.temp.__exit__(None, None, None)

    def test_two_builds_of_one_program_are_identical(self):
        first, second = self.dir / "a", self.dir / "b"
        self.assertEqual(build(self.source, first).returncode, 0)
        self.assertEqual(build(self.source, second).returncode, 0)
        self.assertEqual(digest(first), digest(second))

    def test_profile_round_trip_keeps_the_behaviour_and_changes_the_layout(self):
        plain, instrumented, profiled = self.dir / "plain", self.dir / "instr", self.dir / "pgo"
        profile = self.dir / "work.prof"
        self.assertEqual(build(self.source, plain).returncode, 0)
        self.assertEqual(build(self.source, instrumented, "--profile-generate", str(profile)).returncode, 0)
        expected = subprocess.run([str(plain)], capture_output=True, timeout=TIMEOUT)
        ran = subprocess.run([str(instrumented)], capture_output=True, timeout=TIMEOUT)
        self.assertEqual(ran.stdout, expected.stdout)
        counts = {}
        for line in profile.read_text(encoding="utf-8").splitlines():
            number, _, name = line.partition("\t")
            counts[name] = int(number)
        self.assertEqual(counts["hot"], 20000)
        self.assertEqual(counts["warm"], 20)
        self.assertEqual(counts["cold"], 20)
        self.assertEqual(build(self.source, profiled, "--profile-use", str(profile)).returncode, 0)
        result = subprocess.run([str(profiled)], capture_output=True, timeout=TIMEOUT)
        self.assertEqual(result.stdout, expected.stdout)
        self.assertNotEqual(digest(plain), digest(profiled))

    def test_a_damaged_profile_changes_nothing_but_the_layout(self):
        plain, profiled = self.dir / "plain", self.dir / "pgo"
        profile = self.dir / "bad.prof"
        profile.write_bytes(b"not a profile\n12\tno_such_function\n\xff\xfe\n\t\n")
        self.assertEqual(build(self.source, plain).returncode, 0)
        self.assertEqual(build(self.source, profiled, "--profile-use", str(profile)).returncode, 0)
        expected = subprocess.run([str(plain)], capture_output=True, timeout=TIMEOUT)
        result = subprocess.run([str(profiled)], capture_output=True, timeout=TIMEOUT)
        self.assertEqual(result.stdout, expected.stdout)

    def test_a_missing_profile_warns_and_builds_without_it(self):
        plain, profiled = self.dir / "plain", self.dir / "x"
        self.assertEqual(build(self.source, plain).returncode, 0)
        result = build(self.source, profiled, "--profile-use", str(self.dir / "absent.prof"))
        self.assertEqual(result.returncode, 0)
        self.assertIn("R0913", result.stderr.decode("utf-8", errors="replace"))
        self.assertEqual(digest(plain), digest(profiled))

    def test_profile_flags_are_for_build_only(self):
        result = run(["check", "--stdlib", str(STDLIB), "--profile-use", str(self.dir / "p"), str(self.source)])
        self.assertEqual(result.returncode, 2)

    def test_timings_are_written_only_when_asked(self):
        quiet = build(self.source, self.dir / "q")
        self.assertNotIn(b"timing", quiet.stderr)
        loud = run(["build", "--stdlib", str(STDLIB), str(self.source), "-o", str(self.dir / "l")],
                   env={**os.environ, "RYN_TIMINGS": "1"})
        self.assertIn(b"timing lower:", loud.stderr)

    def test_lazy_lowering_gives_the_same_program_as_lowering_everything(self):
        lazy, everything = self.dir / "lazy", self.dir / "all"
        self.assertEqual(build(self.source, lazy).returncode, 0)
        full = run(["build", "--stdlib", str(STDLIB), str(self.source), "-o", str(everything)],
                   env={**os.environ, "RYN_LOWER_ALL": "1"})
        self.assertEqual(full.returncode, 0, full.stderr)
        expected = subprocess.run([str(everything)], capture_output=True, timeout=TIMEOUT)
        result = subprocess.run([str(lazy)], capture_output=True, timeout=TIMEOUT)
        self.assertEqual(result.stdout, expected.stdout)


if __name__ == "__main__":
    unittest.main()
