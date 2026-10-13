"""Correctness of startup elision, batched constant output and integer strength reduction."""
import os
import subprocess
import unittest
import test_borrowing as borrowing
from ryn_harness import REPO_ROOT, IS_WINDOWS


class NativePerformance(unittest.TestCase):
    setUp = borrowing.Borrowing.setUp
    compile = borrowing.Borrowing.compile

    def test_constant_multiplication_preserves_width_sign_and_wrapping(self):
        source = []
        lines = []
        output = []
        case = 0
        for width in (8, 16, 32, 64):
            for signed in (False, True):
                ty = ("i" if signed else "u") + str(width)
                multipliers = [0, 1, 3, 5, 7, 9, 17, 31, 33, 65, 127]
                if signed:
                    multipliers += [-1, -3, -7, -33, -(1 << (width - 1))]
                else:
                    multipliers += [255 if width > 8 else 255, (1 << width) - 1]
                if width >= 32:
                    multipliers += [1009, 2147483647]
                if width == 64:
                    multipliers += [(1 << 63) + 1 if not signed else -2147483648]
                value = -7 if signed else (1 << width) - 7
                for multiplier in multipliers:
                    case += 1
                    source.append(f"fun mul{case}(v: {ty}) -> {ty} {{ return v * {multiplier} }}")
                    lines.append(f"v{case}: {ty} = {value} + (arg_count() as {ty})")
                    lines.append(f"echo mul{case}(v{case})")
                    result = (value * multiplier) % (1 << width)
                    if signed and result >= 1 << (width - 1):
                        result -= 1 << width
                    output.append(str(result))
        self.compile("\n".join(source) + "\nfun main() {\n" + "\n".join(lines) + "\n}\n",
                     "\n".join(output) + "\n")

    def test_zero_and_one_multipliers_evaluate_the_operand_once(self):
        self.compile('''fun input() -> i64 { echo "once" return -7 }
fun main() { echo input() * 0 echo input() * 1 echo 33 * input() }
''', "once\n0\nonce\n-7\nonce\n-231\n")

    def test_constant_output_keeps_unicode_empty_lines_and_dynamic_effect_order(self):
        self.compile('''fun dynamic() -> i32 { echo "side effect" return 7 }
fun main() {
    name := "Ryn"
    echo "Hello, {name}!"
    echo ""
    echo "Żółw 🦀"
    value := dynamic()
    echo "before {value} after"
    mut text := "old"
    text = "new"
    echo "value {text}"
}
''', "Hello, Ryn!\n\nŻółw 🦀\nside effect\nbefore 7 after\nvalue new\n")

    @unittest.skipUnless(IS_WINDOWS, "Windows startup")
    def test_hello_does_not_import_shell32_or_allocate_an_unused_signal_stack(self):
        self.compile('''extern "C" fun ryn_native_live_allocations() -> u64;
fun main() { echo "Hello, Ryn!" echo ryn_native_live_allocations() }
''', "Hello, Ryn!\n0\n")
        data = (self.root / "app.exe").read_bytes().lower()
        self.assertNotIn(b"shell32.dll", data)
        self.assertNotIn(b"commandlinetoargvw", data)

    @unittest.skipUnless(IS_WINDOWS, "Windows argument parser")
    def test_programs_using_arguments_keep_windows_quoting_and_unicode(self):
        self.compile('''fun main() {
    echo arg_count()
    echo arg(0) echo arg(1) echo arg(2) echo arg(3)
}
''', "0\n\n\n\n\n")
        path = self.root / "app.exe"
        self.assertIn(b"shell32.dll", path.read_bytes().lower())
        p = subprocess.run([str(path), "two words", "Żółw 🦀", 'say "hello"', ""], capture_output=True, timeout=15)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(p.stderr, b"")
        self.assertEqual(p.stdout.decode(), '4\ntwo words\nŻółw 🦀\nsay "hello"\n\n')

    def test_exit_runs_the_destructor_and_preserves_the_main_result(self):
        source = self.root / "exit.ryn"
        source.write_text('''#[drop(release)]
struct Resource { handle: *u8, id: i32 }
fun release(value: Resource) { echo "released {value.id}" }
fun main() -> i32 { r := Resource { handle: 1 as *u8, id: 7 } return 23 }
''', encoding="utf-8")
        path = self.root / ("exit.exe" if IS_WINDOWS else "exit")
        p = subprocess.run([self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                            "-o", path.as_posix(), source.as_posix()], capture_output=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr.decode())
        p = subprocess.run([str(path)], capture_output=True, timeout=15)
        self.assertEqual(p.returncode, 23, p.stderr)
        self.assertEqual(p.stderr, b"")
        self.assertEqual(p.stdout, b"released 7\n")

    @unittest.skipUnless(IS_WINDOWS, "Windows exception handler")
    def test_access_violations_keep_the_runtime_diagnostic(self):
        source = self.root / "fault.ryn"
        source.write_text('''fun main() { echo "before" p: *i32 = null echo *p }
''', encoding="utf-8")
        path = self.root / "fault.exe"
        p = subprocess.run([self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                            "-o", path.as_posix(), source.as_posix()], capture_output=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr.decode())
        p = subprocess.run([str(path)], capture_output=True, timeout=15)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(p.stdout, b"before\n")
        self.assertIn(b"invalid memory access", p.stderr)
