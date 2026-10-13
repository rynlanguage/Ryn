"""Windows keyboard regression: PE imports, real polling and deterministic transitions.

The transition test substitutes only GetAsyncKeyState in a temporary stdlib. No real
keypresses are injected into the user's desktop. RYN_TEST_RYNC can select a rebuilt compiler.
"""
import os
import hashlib
import shutil
import subprocess
import tempfile
import unittest
import sys
import zipfile
from pathlib import Path

from ryn_harness import IS_WINDOWS, REPO_ROOT, ryn_binary


class WindowsKeyboardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ryn-keyboard-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        override = os.environ.get("RYN_TEST_RYNC")
        self.compiler = Path(override) if override else Path(ryn_binary()).with_name(
            "rync.exe" if IS_WINDOWS else "rync"
        )

    def compile(self, source, stdlib=None):
        path = self.directory / "main.ryn"
        path.write_text(source, encoding="utf-8")
        library = stdlib or REPO_ROOT / "stdlib/std/src"
        common = [str(self.compiler), "--stdlib", str(library), "--target", "windows"]
        check = subprocess.run([*common, "check", str(path)], capture_output=True, timeout=120)
        self.assertEqual(check.returncode, 0, check.stderr.decode())
        output = self.directory / "main.exe"
        build = subprocess.run([*common, "-o", str(output), str(path)], capture_output=True, timeout=120)
        self.assertEqual(build.returncode, 0, build.stderr.decode())
        self.assertEqual(build.stderr, b"")
        return output

    def execute(self, output, expected):
        run = subprocess.run([str(output)], capture_output=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stderr.decode())
        self.assertEqual(run.stderr, b"")
        self.assertEqual(run.stdout, expected)

    def test_realtime_example_imports_user32_without_linux_directory_functions(self):
        source = (REPO_ROOT / "examples/realtime_input/src/main.ryn").read_text(encoding="utf-8")
        output = self.compile(source)
        image = output.read_bytes()
        self.assertEqual(image[:2], b"MZ")
        self.assertTrue(b"user32.dll\0" in image, "keyboard must import user32.dll")
        self.assertIn(b"GetAsyncKeyState\0", image)
        for function in (b"opendir\0", b"readdir\0", b"closedir\0"):
            self.assertNotIn(function, image)

    @unittest.skipUnless(IS_WINDOWS, "executes a Windows program")
    def test_windows_release_package_builds_and_runs_with_its_bundled_stdlib(self):
        dist = self.directory / "dist"
        result = subprocess.run(
            [sys.executable, str(REPO_ROOT / "tools/make-dist.py"), str(self.compiler),
             "0.1.13", str(dist), "--windows"], capture_output=True, timeout=180,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        archive = dist / "ryn-0.1.13-windows-x86_64.zip"
        checksum = Path(str(archive) + ".sha256").read_text().split()[0]
        self.assertEqual(checksum, hashlib.sha256(archive.read_bytes()).hexdigest())
        with zipfile.ZipFile(archive) as bundle:
            bundle.extractall(self.directory / "unpacked")
        package = self.directory / "unpacked/ryn-0.1.13-windows-x86_64"
        program = package / "smoke.ryn"
        program.write_text('use std::keyboard\nfun main() { _ := keyboard::key_down(Key::W) echo "package ready" }\n')
        environment = os.environ.copy()
        environment.pop("RYN_STDLIB", None)
        run = subprocess.run([str(package / "ryn.exe"), "run", "smoke.ryn"],
                             cwd=package, env=environment, capture_output=True, timeout=120)
        self.assertEqual(run.returncode, 0, run.stderr.decode())
        self.assertEqual(run.stderr, b"")
        self.assertEqual(run.stdout, b"package ready\n")

    @unittest.skipUnless(IS_WINDOWS, "executes a Windows program")
    def test_cli_check_build_and_run_with_windows_keyboard(self):
        path = self.directory / "main.ryn"
        path.write_text('''use std::keyboard
fun main() {
    _ := keyboard::key_down(Key::W)
    _ := keyboard::key_pressed(Key::Space)
    _ := keyboard::key_released(Key::W)
    echo "cli ready"
}
''', encoding="utf-8")
        environment = os.environ.copy()
        environment["RYN_STDLIB"] = (REPO_ROOT / "stdlib/std/src").as_posix()
        for command in ("check", "build", "run"):
            with self.subTest(command=command):
                result = subprocess.run(
                    [ryn_binary(), command, "main.ryn"], cwd=self.directory,
                    env=environment, capture_output=True, timeout=120,
                )
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertEqual(result.stderr, b"")
                if command == "run":
                    self.assertEqual(result.stdout, b"cli ready\n")

    @unittest.skipUnless(IS_WINDOWS, "executes a Windows program")
    def test_real_windows_polling_finishes_without_waiting_for_enter(self):
        output = self.compile('''use std::keyboard
fun main() {
    for i in 0..3 {
        _ := keyboard::key_down(Key::W)
        _ := keyboard::key_pressed(Key::Space)
        _ := keyboard::key_released(Key::W)
        assert(!keyboard::key_down(Key::Unknown))
        assert(!keyboard::key_pressed(Key::Unknown))
        assert(!keyboard::key_released(Key::Unknown))
        sleep(1ms)
    }
    echo "polling ready"
}
''')
        self.execute(output, b"polling ready\n")

    @unittest.skipUnless(IS_WINDOWS, "executes a Windows program")
    def test_latched_edges_holds_release_read_key_and_virtual_key_mapping(self):
        library = self.directory / "stdlib"
        shutil.copytree(REPO_ROOT / "stdlib/std/src", library)
        runtime = library / "native_windows.ryn"
        source = runtime.read_text(encoding="utf-8")
        declaration = 'extern "C" fun GetAsyncKeyState(key: i32) -> i16;'
        self.assertEqual(source.count(declaration), 1)
        source = source.replace(declaration, '''fun GetAsyncKeyState(key: i32) -> i16 {
    mask := word64_at(keyboard_state(), 16)
    when mask == 4 && key == 87 {
        count := word64_at(keyboard_state(), 8) + 1
        set_word64_at(keyboard_state(), 8, count)
        when count >= 2 { return -32768 }
    }
    when (key == 87 && (mask & 1) != 0) || (key == 32 && (mask & 2) != 0) {
        return -32768
    }
    return 1 // A low transition bit alone must never count as down.
}
pub fun test_keyboard_mask(mask: u64) {
    set_word64_at(keyboard_state(), 16, mask)
    set_word64_at(keyboard_state(), 8, 0)
}
pub fun test_keyboard_code(key: u32) -> i32 { return windows_key_code(key) }
''')
        runtime.write_text(source, encoding="utf-8")
        # Expected VK values in public Key enum order, from Microsoft's virtual-key table.
        codes = ([0] + list(range(65, 91)) + list(range(48, 58)) +
                 [32, 13, 27, 9, 8, 46, 45, 36, 35, 33, 34, 38, 40, 37, 39,
                  16, 160, 161, 17, 162, 163, 18, 164, 165, 20, 144, 145] +
                 list(range(112, 124)) +
                 [192, 189, 187, 219, 221, 220, 186, 222, 188, 190, 191] +
                 list(range(96, 106)) + [107, 109, 106, 111, 110, 13, 44, 19, 93, 91, 92])
        self.assertEqual(len(codes), 108)
        mapping = "\n".join(
            f"assert(native_windows::test_keyboard_code({key}) == {code})"
            for key, code in enumerate(codes)
        )
        output = self.compile('''use std::keyboard
use std::native_windows
fun expect_key(actual: Key, expected: u32) {
    code: u32 = choose actual { Key::W => 23, Key::Space => 37, _ => 0 }
    assert(code == expected)
}
fun main() {
    assert(!keyboard::key_down(Key::W))
    assert(!keyboard::key_pressed(Key::W))
    native_windows::test_keyboard_mask(3)
    assert(keyboard::key_down(Key::W))
    assert(keyboard::key_pressed(Key::W))
    assert(!keyboard::key_pressed(Key::W))
    assert(keyboard::key_down(Key::W))
    expect_key(keyboard::read_key(), 37)
    assert(!keyboard::key_pressed(Key::Space))
    native_windows::test_keyboard_mask(0)
    assert(!keyboard::key_down(Key::W))
    assert(keyboard::key_released(Key::W))
    assert(!keyboard::key_released(Key::W))
    assert(keyboard::key_released(Key::Space))
    native_windows::test_keyboard_mask(3)
    expect_key(keyboard::read_key(), 23)
    expect_key(keyboard::read_key(), 37)
    assert(!keyboard::key_pressed(Key::W))
    assert(!__input_key_down(999))
    assert(!__input_key_pressed(999))
    assert(!__input_key_released(999))
    native_windows::test_keyboard_mask(0)
    assert(!keyboard::key_down(Key::W))
    native_windows::test_keyboard_mask(4)
    expect_key(keyboard::read_key(), 23)
''' + mapping + '\n    echo "transitions ready"\n}\n', library)
        self.execute(output, b"transitions ready\n")


if __name__ == "__main__":
    unittest.main()
