"""Regression programs for resource results, character casts, map inference and project imports."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from ryn_harness import REPO_ROOT, ryn_binary, IS_WINDOWS


class LanguageRegressions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ryn-language-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.compiler = os.environ.get("RYN_TEST_RYNC") or str(Path(ryn_binary()).with_name("rync.exe" if IS_WINDOWS else "rync"))

    def program(self, source, stdout):
        path = self.root / "main.ryn"
        path.write_text(source, encoding="utf-8")
        self.compile_run(path, stdout)

    def compile_run(self, path, stdout):
        output = self.root / ("app.exe" if IS_WINDOWS else "app")
        common = [self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix()]
        for arguments in (["check", path.as_posix()], ["-o", output.as_posix(), path.as_posix()]):
            result = subprocess.run([*common, *arguments], cwd=self.root, capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        run = subprocess.run([str(output)], cwd=self.root, capture_output=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stderr.decode() + repr(run.stdout))
        self.assertEqual(run.stderr, b"")
        self.assertEqual(run.stdout.decode(), stdout)

    def test_destructor_in_result_payload_moves_and_drops_once(self):
        self.program('''namespace c;
extern "C" fun malloc(size: u64) -> *u8;
extern "C" fun free(pointer: *u8);
namespace;
#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) { echo value.id c::free(value.handle) }
fun create(id: i32) -> Result<Gpu, String> {
    Result::Ok(Gpu { handle: c::malloc(8), id: id })
}
fun main() {
    result := create(1)
    when true {
        gpu := choose result { Result::Ok(gpu) => gpu, Result::Err(error) => Gpu { handle: null, id: 0 } }
        echo gpu.id
    }
    when true { gpu := create(2).unwrap() echo gpu.id }
    _ := create(3)
}
''', "1\n1\n2\n2\n3\n")

    def test_integer_cast_to_char_builds_text(self):
        self.program('''fun main() {
    bytes: [u8; 3] = [82, 121, 110]
    mut text := String()
    for byte in bytes { text.push(byte as char) }
    echo text
    echo 0x1F980 as char
    letter := take(String("owned")) as char
    echo letter
}
fun take(value: String) -> u8 { echo value return 65 }
''', "Ryn\n🦀\nowned\nA\n")

    def test_invalid_integer_character_casts_do_not_truncate(self):
        for value in ("55296", "1114112", "4294967361", "-4294967231"):
            source = self.root / "invalid.ryn"
            source.write_text(f'fun main() {{ value: i64 = {value} echo value as char }}\n')
            output = self.root / ("invalid.exe" if IS_WINDOWS else "invalid")
            build = subprocess.run([self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                                    "-o", output.as_posix(), source.as_posix()], capture_output=True, timeout=120)
            self.assertEqual(build.returncode, 0, build.stderr.decode())
            run = subprocess.run([str(output)], capture_output=True, timeout=15)
            self.assertEqual(run.returncode, 1, run.stderr.decode())
            self.assertEqual(run.stdout, b"")
            self.assertIn(b"invalid Unicode scalar value", run.stderr)

    def test_resource_option_result_propagation_and_extraction(self):
        self.program('''namespace c;
extern "C" fun malloc(size: u64) -> *u8;
extern "C" fun free(pointer: *u8);
namespace;
#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) { echo value.id c::free(value.handle) }
fun gpu(id: i32) -> Gpu { Gpu { handle: c::malloc(8), id: id } }
fun create(ok: bool) -> Result<Gpu, String> {
    when ok { return Result::Ok(gpu(10)) }
    Result::Err(String("failed"))
}
fun forward(ok: bool) -> Result<Gpu, String> {
    resource := create(ok)?
    Result::Ok(resource)
}
fun broken() -> Result<i32, Gpu> { Result::Err(gpu(40)) }
fun forward_error() -> Result<i32, Gpu> {
    value := broken()?
    Result::Ok(value)
}
fun main() {
    when true { result := forward(true) resource := result.expect("gpu") echo resource.id }
    echo forward(false).unwrap_err()
    when true { option: Option<Gpu> = Option::Some(gpu(20)) resource := option.unwrap() echo resource.id }
    when true { error: Result<String, Gpu> = Result::Err(gpu(30)) resource := error.unwrap_err() echo resource.id }
    when true { resource := forward_error().unwrap_err() echo resource.id }
}
''', "10\n10\nfailed\n20\n20\n30\n30\n40\n40\n")

    def test_resource_result_use_after_unwrap_is_rejected(self):
        source = self.root / "moved.ryn"
        source.write_text('''#[drop(release)]
struct Gpu { handle: *u8 }
fun release(value: Gpu) {}
fun main() {
    result: Result<Gpu, String> = Result::Ok(Gpu { handle: null })
    gpu := result.unwrap()
    echo result.is_ok()
}
''')
        check = subprocess.run([self.compiler, "check", "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                                source.as_posix()], capture_output=True, timeout=120)
        self.assertEqual(check.returncode, 1, check.stderr.decode())
        self.assertIn(b"R0240", check.stderr)

    def test_map_constructor_uses_declared_context(self):
        self.program('''fun main() {
    mut values: Map<String, i32> = Map()
    values.insert(String("answer"), 42)
    echo values.get(String("answer")) ?? 0
}
''', "42\n")

    def test_map_constructor_infers_from_struct_field(self):
        self.program('''struct Registry { slots: Vec<i32>, names: Map<u64, String>, total: u64 }
fun create() -> Registry { Registry { slots: Vec(), names: Map(), total: 0 } }
fun main() {
    mut registry := create()
    registry.names.insert(1, String("Ryn"))
    echo registry.names.get(1) ?? String("missing")
}
''', "Ryn\n")

    def test_map_without_context_reports_inference_diagnostic(self):
        source = self.root / "main.ryn"
        source.write_text("fun main() { values := Map() }\n")
        check = subprocess.run([self.compiler, "check", "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                                source.as_posix()], capture_output=True, timeout=120)
        self.assertEqual(check.returncode, 1, check.stderr.decode())
        self.assertIn(b"R0262", check.stderr)

    def test_use_imports_public_type(self):
        self.program('''use std::arena::Arena
fun main() {
    arena := Arena::with_capacity(128)
    echo arena.used()
}
''', "0\n")

    def test_use_imports_function_and_type_alias(self):
        (self.root / "util.ryn").write_text("pub type Count = u64\npub fun answer() -> Count { 42 }\n")
        self.program('''use util::Count
use util::answer as value
fun main() { n: Count = value() echo n }
''', "42\n")

    def test_use_rejects_missing_and_private_items(self):
        (self.root / "util.ryn").write_text("struct Secret { id: i32 }\n")
        for name in ("Secret", "Missing"):
            source = self.root / "main.ryn"
            source.write_text(f'use util::{name}\nfun main() {{ echo "must not run" }}\n')
            check = subprocess.run([self.compiler, "check", "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                                    source.as_posix()], capture_output=True, timeout=120)
            self.assertEqual(check.returncode, 1, check.stderr.decode())
            self.assertIn(b"R0425", check.stderr)

    def test_existing_ownership_programs_keep_output_and_allocation_counts(self):
        cases = [
            "raii/native_enum_array_and_struct_payloads_leave_no_allocations.ryn",
            "raii/native_array_owners_leave_no_allocations.ryn",
            "raii/native_destructor_collections_leave_no_allocations.ryn",
            "raii/native_drop_order_and_moves.ryn",
            "result_option/unwrap_err_extracts_message.ryn",
            "result_option/unwrap_or_fallback.ryn",
        ]
        for case in cases:
            with self.subTest(case=case):
                source = REPO_ROOT / "tests/suite" / case
                expected = ""
                for line in source.read_text(encoding="utf-8").splitlines():
                    if not line.startswith("//"): break
                    if line.startswith("// out: "): expected += line[8:] + "\n"
                self.compile_run(source, expected)

    def test_path_dependency_is_relative_to_manifest_from_other_directory(self):
        project = self.root / "project"
        (project / "src").mkdir(parents=True)
        (project / "vendor/math/src").mkdir(parents=True)
        (project / "ryn.yaml").write_text("name: app\nversion: 0.1.0\ndependencies:\n  math:\n    path: ./vendor/math\n")
        (project / "src/main.ryn").write_text("use math::ops\nfun main() { echo ops::answer() }\n")
        (project / "vendor/math/ryn.yaml").write_text("name: math\nversion: 0.1.0\ndependencies:\n")
        (project / "vendor/math/src/ops.ryn").write_text("pub fun answer() -> i32 { 42 }\n")
        self.compile_run(project, "42\n")
        self.compile_run(project / "src/main.ryn", "42\n")
        # CLI commands must agree for both a project argument and a source-file argument.
        environment = os.environ.copy()
        environment["RYN_STDLIB"] = (REPO_ROOT / "stdlib/std/src").as_posix()
        for target in (project, project / "src/main.ryn"):
            for command in ("check", "build", "run"):
                arguments = [ryn_binary(), command, target.as_posix()]
                if command == "build": arguments += ["-o", (self.root / "cli.exe").as_posix()]
                result = subprocess.run(arguments, cwd=self.root, env=environment, capture_output=True, timeout=120)
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                if command == "run": self.assertEqual(result.stdout, b"42\n")


if __name__ == "__main__":
    unittest.main()
