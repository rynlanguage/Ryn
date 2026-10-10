"""Port of the native cases of tests/backend_selection.rs and of the build API cases of tests/compiler_api.rs.

The cranelift half of backend_selection.rs is gone with the Rust code generator: `--backend cranelift` is now refused
(R0907), and the test for that refusal is in this file. The native programs are checked against their `// out:` and
`// exit:` headers, as the Rust test did.

Cases marked GAP are real compiler gaps (see tools/tests/DIAGNOSTIC_GAPS.md).
"""

import os
import subprocess
import unittest
from pathlib import Path

from ryn_harness import REPO_ROOT, TempDir, run_ryn

SUITE = REPO_ROOT / "tests" / "suite"
NATIVE_PROGRAMS = [
    SUITE / "collections" / "str_view_len_and_byte_at.ryn",
    SUITE / "collections" / "str_byte_at_out_of_bounds.ryn",
    SUITE / "collections" / "string_byte_at_out_of_bounds.ryn",
    SUITE / "collections" / "discarded_destructor_values_drop_once.ryn",
    SUITE / "collections" / "native_nested_destructor_elements.ryn",
    SUITE / "ffi" / "float_record_shims_round_trip.ryn",
]


def expectations(source: Path):
    """The expected stdout and exit status, from the program's `// out:` and `// exit:` headers."""
    stdout = ""
    exit_status = 0
    for line in source.read_text(encoding="utf-8").splitlines():
        if not line.startswith("//"):
            break
        if line.startswith("// out: "):
            stdout += line[len("// out: ") :] + "\n"
        elif line.startswith("// exit: "):
            exit_status = int(line[len("// exit: ") :].strip())
    return stdout, exit_status


class BuildNativeTests(unittest.TestCase):
    def setUp(self):
        temp = TempDir("build-native")
        self.dir = temp.__enter__()
        self.addCleanup(temp.__exit__, None, None, None)

    def write(self, name, text):
        path = self.dir / name
        path.write_text(text, encoding="utf-8")
        return path

    def test_native_backend_output_and_exit_status_match_the_program_headers(self):
        for source in NATIVE_PROGRAMS:
            with self.subTest(program=source.name):
                executable = self.dir / (source.stem + ".bin")
                build = run_ryn("build", "--backend", "native", source, "-o", executable)
                self.assertEqual(build.code, 0, f"{source.name} must build:\n{build.stderr}")
                run = subprocess.run([str(executable)], capture_output=True)
                expected_stdout, expected_exit = expectations(source)
                self.assertEqual(run.stdout.decode("utf-8"), expected_stdout)
                self.assertEqual(run.returncode, expected_exit)

    def test_run_of_a_single_file_leaves_no_executable_beside_it(self):
        source = self.write("program.ryn", "fun main() {\n    echo 7\n}\n")

        result = run_ryn("run", source)

        self.assertEqual(result.code, 0, result.stderr)
        self.assertEqual(result.stdout, "7\n")
        self.assertEqual(sorted(entry.name for entry in self.dir.iterdir()), ["program.ryn"])

    def test_native_build_rejects_what_check_rejects_and_writes_no_executable(self):
        source = SUITE / "collections" / "immutable_self_string_push_is_r0204.ryn"
        executable = self.dir / "program"

        result = run_ryn("build", source, "-o", executable)

        self.assertEqual(result.code, 1, result.stderr)
        self.assertIn("error[R0204]", result.stderr)
        self.assertFalse(executable.exists(), "a rejected program writes no executable")

    def test_an_unknown_backend_is_rejected_with_its_diagnostic(self):
        source = SUITE / "collections" / "str_view_len_and_byte_at.ryn"

        result = run_ryn("build", "--backend", "llvm", source, "-o", self.dir / "program")

        self.assertEqual(result.code, 1, result.stderr)
        self.assertIn("error[R0907]", result.stderr)
        self.assertIn("unknown backend `llvm`", result.stderr)

    def test_the_removed_cranelift_backend_is_refused_like_any_unknown_backend(self):
        source = SUITE / "collections" / "str_view_len_and_byte_at.ryn"

        result = run_ryn("build", "--backend", "cranelift", source, "-o", self.dir / "program")

        self.assertEqual(result.code, 1, result.stderr)
        self.assertIn("error[R0907]", result.stderr)
        self.assertIn("unknown backend `cranelift`", result.stderr)

    def test_build_refuses_to_overwrite_its_source_at_the_same_path(self):
        original = "fun main() {\n    echo(42)\n}\n"
        source = self.write("fixture.ryn", original)

        result = run_ryn("build", source, "-o", source)

        self.assertEqual(result.code, 1, result.stderr)
        self.assertIn("error[R0303]", result.stderr)
        self.assertEqual(source.read_text(encoding="utf-8"), original)

    def test_build_through_a_symbolic_link_to_its_source_keeps_the_source(self):
        original = "fun main() {\n    echo(42)\n}\n"
        source = self.write("fixture.ryn", original)
        link = self.dir / "fixture-symbolic-link.ryn"
        os.symlink(source, link)

        result = run_ryn("build", source, "-o", link)

        # The executable is written beside the output and renamed over it, so a link is replaced and its target is not
        # written through: the source survives whether the build succeeds or is refused.
        self.assertEqual(source.read_text(encoding="utf-8"), original, "the source must survive")

    def test_build_through_a_hard_link_to_its_source_keeps_the_source(self):
        original = "fun main() {\n    echo(42)\n}\n"
        source = self.write("fixture.ryn", original)
        link = self.dir / "fixture-hard-link.ryn"
        os.link(source, link)

        result = run_ryn("build", source, "-o", link)

        # The executable is written beside the output and renamed over it, so a link is replaced and its target is not
        # written through: the source survives whether the build succeeds or is refused.
        self.assertEqual(source.read_text(encoding="utf-8"), original, "the source must survive")

    def test_a_native_setup_failure_is_reported_and_leaves_the_source_alone(self):
        blocker = self.write("blocker", "file blocks the output directory")
        source = self.write("program.ryn", "fun main() {}\n")

        result = run_ryn("build", source, "-o", blocker / "program.exe")

        self.assertNotEqual(result.code, 0, result.stderr)
        self.assertIn("error[R0301]", result.stderr)
        self.assertEqual(source.read_text(encoding="utf-8"), "fun main() {}\n")


if __name__ == "__main__":
    unittest.main()
