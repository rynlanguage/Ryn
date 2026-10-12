"""Port of tests/native_hello.rs: native-backend programs driven through the ryn CLI.

Every test runs the `ryn` binary under test (RYN_TEST_BIN) and checks stdout, stderr and
the exit status, as the Rust version did. Tests that need Rust-only features (the Cranelift
backend, RYN_LINKER, the Rust library API, or Windows-only `#[cfg(windows)]` gating) are
skipped with a reason; see tools/tests/RUST_ONLY_native_hello.md.
"""

import shutil
import subprocess
import unittest
from pathlib import Path

from ryn_harness import IS_WINDOWS, REPO_ROOT, Run, TempDir, ryn_binary, run_ryn

WINDOWS_ONLY = "Windows-only (#[cfg(windows)] in tests/native_hello.rs); needs a Windows host"
CRANELIFT_AND_RYN_LINKER = (
    "Rust-only: uses `--backend cranelift` and RYN_LINKER=rustc, the external Cranelift link driver"
)
RUST_LIBRARY_API = (
    "Rust-only: calls the Rust library API ryn::codegen::link_object / build_native "
    "and re-executes the Rust test binary"
)


def repo_path(relative: str) -> Path:
    """Resolves a repository-relative path such as 'examples/hello.ryn'."""
    return REPO_ROOT.joinpath(*relative.split("/"))


def write_bytes_text(path: Path, text: str) -> None:
    """Writes UTF-8 text without newline translation, like `fs::write`."""
    path.write_bytes(text.encode("utf-8"))


def run_program(path: Path) -> Run:
    """Runs a built executable and captures its output, like `Command::new(path).output()`."""
    proc = subprocess.run([str(path)], capture_output=True)
    return Run(
        proc.returncode,
        proc.stdout.decode("utf-8", errors="replace"),
        proc.stderr.decode("utf-8", errors="replace"),
    )


class NativeHelloTests(unittest.TestCase):
    def temp_dir(self, label: str) -> Path:
        """A fresh directory removed when the test finishes, like `temp_dir` plus cleanup."""
        directory = TempDir(label).path
        self.addCleanup(shutil.rmtree, directory, True)
        return directory

    def run_source(self, source: Path) -> Run:
        """Stages the source in a temporary directory and runs `ryn run` on the copy."""
        directory = self.temp_dir("test-run")
        staged_source = directory / source.name
        shutil.copyfile(source, staged_source)
        return run_ryn("run", staged_source)

    def run_source_text(self, label: str, source_text: str) -> Run:
        """Writes the source text to main.ryn in a temporary directory and runs `ryn run` on it."""
        directory = self.temp_dir(label)
        source = directory / "main.ryn"
        write_bytes_text(source, source_text)
        return run_ryn("run", source)

    def assertRunSucceeded(self, result: Run) -> None:
        self.assertEqual(result.code, 0, f"compiler/runtime failed: {result.stderr}")

    def test_array_repeat_literals_copy_one_evaluation(self):
        result = self.run_source(repo_path("examples/array_repeat.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "65\n65\n0\n1\nryn\n4\n21\n1\n7\n7\n1\n0\nowned\n3\n",
        )

    def test_language_additions_from_the_self_hosted_frontend_run_natively(self):
        result = self.run_source(repo_path("examples/language_0_1.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "2\nfun\nkeyword\nfun!\n233\n3\nR1\n3\npath call\n",
        )

    def test_hello_ryn_runs_through_native_backend(self):
        result = self.run_source(repo_path("examples/hello.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout.strip(), "Hello, Ryn!")

    def test_quick_start_example_runs_through_native_backend(self):
        result = self.run_source(repo_path("examples/quick_start.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "alive\nHello, Ryn!\n42\n80\n")

    def test_enum_payloads_and_choose_execute_through_the_native_backend(self):
        result = self.run_source(repo_path("examples/enums.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\n4\n2\n0\nmoved out\n9\n8\n")

    def test_repr_c_small_integer_records_use_the_native_c_aggregate_abi(self):
        result = self.run_source(repo_path("examples/ffi_repr_c.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "42\n42\n4\n4\n42\n20\n22\n42\n3579\n1000\n2345\n3345\n42\n12\n30\n42\n42\n20\n22\n42\n",
        )

    def test_raw_pointer_to_repr_c_record_with_array_keeps_all_fields_addressable(self):
        result = self.run_source(repo_path("examples/raw_pointer_repr_c_array.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "7\n11\n22\n33\n44\n99\n")

    @unittest.skipUnless(IS_WINDOWS, WINDOWS_ONLY)
    def test_dynamic_library_symbols_can_be_called_with_a_declared_c_signature(self):
        result = self.run_source(repo_path("examples/dynamic_library.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\n")

    def test_raw_pointer_casts_reinterpret_the_same_address(self):
        result = self.run_source_text(
            "pointer-reinterpret",
            r"""
            fun main() {
                mut value: i32 = 42
                raw := &raw mut value
                bytes := raw as *u8
                restored := bytes as *i32
                echo *restored
                echo pointer_is_null(bytes)
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\nfalse\n")

    def test_integer_address_casts_to_a_raw_pointer(self):
        result = self.run_source_text(
            "int-to-pointer",
            r"""
            fun main() {
                echo pointer_is_null(32512 as *u8)
                echo pointer_is_null(0 as *u8)
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "false\ntrue\n")

    @unittest.skipUnless(IS_WINDOWS, WINDOWS_ONLY)
    def test_borrowed_raw_pointer_field_casts_to_a_function_pointer(self):
        result = self.run_source_text(
            "pointer-field-cast",
            r"""
            type AbsFn = extern "C" fun(i32) -> i32

            struct Handle { ptr: *u8 }

            extend Handle {
                fun apply(self, value: i32) -> i32 {
                    function: AbsFn = self.ptr as AbsFn
                    return function(value)
                }
            }

            fun main() {
                library := load_library("ucrtbase.dll")
                handle := Handle { ptr: load_symbol(library, "abs") }
                echo handle.apply(-42)
                unload_library(library)
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\n")

    def test_generic_struct_instantiations_run_through_the_native_backend(self):
        result = self.run_source(repo_path("examples/generic_structs.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\nryn-lang\n1\n9\n")

    def test_generic_enum_instantiations_run_through_the_native_backend(self):
        result = self.run_source(repo_path("examples/generic_enums.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\n11\n3\n0\n6\n3\n0\n")

    def test_slices_of_owned_vec_elements_clone_on_index_and_iteration(self):
        result = self.run_source(repo_path("examples/slices.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "32\n16\nRyn\n4\nRyn\nnested\n27\n")

    def test_arrays_of_owned_elements_can_be_borrowed_as_slices(self):
        result = self.run_source(repo_path("examples/array_slices.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "20\n9\n0\nhi\n")

    def test_self_hosted_lexer_checks_integer_literal_overflow(self):
        result = self.run_source(repo_path("examples/self_hosted/lexer.ryn"))
        self.assertRunSucceeded(result)
        self.assertIn(
            "0\n18446744073709551615\n12\n12\n12\n0\n",
            result.stdout,
            f"integer boundary and overflow results were missing: {result.stdout}",
        )

    def test_self_hosted_lexer_checks_numeric_ranges_and_unicode_escapes(self):
        result = self.run_source(repo_path("examples/self_hosted/lexer.ryn"))
        self.assertRunSucceeded(result)

    def test_filesystem_path_components_use_host_path_rules(self):
        result = self.run_source(repo_path("examples/filesystem_paths.ryn"))
        self.assertRunSucceeded(result)
        self.assertTrue(
            result.stdout.startswith(
                "build/ryn_fs_read_dir_contract\na.ryn\nryn\nfalse\n.env\n\ngz\n\n2\n"
            ),
            f"path component output was unexpected: {result.stdout}",
        )

    def test_references_and_raw_pointers_access_scalar_locals(self):
        result = self.run_source(repo_path("examples/references.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "10\n27\n32\n33\n")

    def test_run_forwards_arguments_to_native_program_including_empty_and_unicode_values(self):
        directory = self.temp_dir("process-arguments")
        source = directory / "arguments.ryn"
        write_bytes_text(
            source,
            'fun selected_index() -> u32 { echo("index evaluated once") 0 }\n'
            "fun main() { echo(arg_count()) echo(arg(0)) echo(arg(1)) echo(arg(2)) echo(arg(3)) "
            "echo(arg(selected_index())) }",
        )

        result = run_ryn("run", source, "--", "with spaces", "", "Zażółć 🦀")
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "3\nwith spaces\n\nZażółć 🦀\n\nindex evaluated once\nwith spaces\n",
        )

    def test_string_escapes_are_preserved_in_native_output(self):
        result = self.run_source(repo_path("examples/string_escapes.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "first\0second\nquote: \" and slash: \\\nUnicode: 🦀\n")

    def test_structures_are_constructed_accessed_mutated_and_returned_by_value(self):
        result = self.run_source(repo_path("examples/structs.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "Veni\n65\ntrue\n100\n"
            "Player { name: Veni, vitals: Vitals { health: 100, alive: true } }\n"
            "updated=Player { name: Veni, vitals: Vitals { health: 65, alive: true } }\n",
        )

    def test_echo_interpolation_writes_text_and_typed_values_in_order(self):
        directory = self.temp_dir("interpolation")
        source = directory / "interpolation.ryn"
        write_bytes_text(
            source,
            'fun main() { name := "Ryn" version: f32 = 0.5 ready := true '
            'echo("Hello {name}: {version} {ready} {{done}}") }',
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "Hello Ryn: 0.5 true {done}\n")

    def test_compound_assignments_update_native_integer_locals(self):
        directory = self.temp_dir("compound-assign")
        source = directory / "compound.ryn"
        write_bytes_text(
            source,
            "fun main() { mut value: i32 = 3 value += 4 value *= 2 value -= 4 value /= 2 "
            "value %= 2 value |= 8 value ^= 3 value &= 6 echo(value) }",
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "2\n")

    def test_equality_compares_boolean_values_and_string_contents(self):
        directory = self.temp_dir("equality")
        source = directory / "equality.ryn"
        write_bytes_text(
            source,
            "fun same(left: str, right: str) -> bool { left == right } fun main() { "
            'first := "cat" same := "cat" other := "car" echo(first == same) '
            'echo(first != other) echo("" == "") echo(true == true) echo(false != true) '
            'echo(same("cat", "cat")) }',
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n")

    def test_pass_fixture_runs_as_a_native_program(self):
        result = self.run_source(repo_path("tests/programs/pass/compound_interpolation.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "health=80\n")

    def test_build_preserves_existing_sibling_intermediate_named_files(self):
        directory = self.temp_dir("build-output")

        source = directory / "fixture.ryn"
        object_paths = [directory / "fixture.obj", directory / "fixture.o"]
        wrapper = directory / "fixture.ryn-wrapper.rs"
        pdb = directory / "fixture.pdb"
        write_bytes_text(source, 'fun main() { echo("built") }')
        for obj in object_paths:
            write_bytes_text(obj, "keep object")
        write_bytes_text(wrapper, "keep wrapper")
        write_bytes_text(pdb, "keep debug symbols")

        result = run_ryn("build", "fixture.ryn", cwd=directory)
        output = directory / ("fixture.exe" if IS_WINDOWS else "fixture")
        output_exists = output.exists()
        object_contents = [obj.read_text(encoding="utf-8") for obj in object_paths]
        wrapper_contents = wrapper.read_text(encoding="utf-8")
        pdb_contents = pdb.read_text(encoding="utf-8")

        self.assertEqual(result.code, 0, f"compiler failed: {result.stderr}")
        self.assertTrue(output_exists, "native executable was not created")
        self.assertEqual(object_contents, ["keep object", "keep object"])
        self.assertEqual(wrapper_contents, "keep wrapper")
        self.assertEqual(pdb_contents, "keep debug symbols")

    def test_build_accepts_a_custom_output_path(self):
        directory = self.temp_dir("custom-output")
        source = directory / "fixture.ryn"
        output = directory / "artifacts" / ("custom.exe" if IS_WINDOWS else "custom")
        write_bytes_text(source, 'fun main() { echo("custom build") }')

        result = run_ryn("build", source, "--output", output)
        self.assertEqual(result.code, 0, f"compiler failed: {result.stderr}")
        self.assertTrue(output.exists(), "custom executable was not created")

        run = run_program(output)
        self.assertEqual(run.code, 0)
        self.assertEqual(run.stdout, "custom build\n")

    def test_build_and_run_accept_source_and_output_paths_with_spaces(self):
        directory = self.temp_dir("paths-with-spaces")
        workspace = directory / "workspace with spaces"
        workspace.mkdir()
        source = workspace / "source program.ryn"
        output = workspace / "output directory" / ("native program.exe" if IS_WINDOWS else "native program")
        write_bytes_text(source, 'fun main() { echo("spaces-ok") }')

        result = run_ryn("build", source, "--output", output)
        self.assertEqual(result.code, 0, f"compiler failed: {result.stderr}")
        self.assertTrue(output.exists(), "native executable was not created")

        run = run_program(output)
        self.assertEqual(run.code, 0)
        self.assertEqual(run.stdout, "spaces-ok\n")

    def test_run_accepts_a_custom_output_path_and_keeps_the_executable(self):
        directory = self.temp_dir("custom-run-output")
        source = directory / "fixture with spaces.ryn"
        output = directory / ("named executable.exe" if IS_WINDOWS else "named executable")
        write_bytes_text(source, 'fun main() { echo("custom run") }')

        result = run_ryn("run", source, "-o", output)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "custom run\n")
        self.assertTrue(output.exists(), "run did not keep the custom executable")

    def test_run_accepts_a_relative_output_filename_without_a_directory(self):
        directory = self.temp_dir("relative-run-output")
        source = directory / "fixture.ryn"
        output = "ryn-relative-output.exe" if IS_WINDOWS else "ryn-relative-output"
        write_bytes_text(source, 'fun main() { echo("relative run") }')

        result = run_ryn("run", "fixture.ryn", "--output", output, cwd=directory)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "relative run\n")
        self.assertTrue((directory / output).exists(), "run did not keep the executable")

    def test_custom_output_cannot_overwrite_the_source_file(self):
        directory = self.temp_dir("source-output-collision")
        source = directory / "fixture.ryn"
        original = 'fun main() { echo("source remains intact") }'
        write_bytes_text(source, original)

        result = run_ryn("build", source, "--output", source)
        if IS_WINDOWS:
            case_variant = Path(str(source).upper())
            alias_result = run_ryn("build", source, "-o", case_variant)
            self.assertNotEqual(alias_result.code, 0)
            self.assertIn("error[R0303]", alias_result.stderr, alias_result.stderr)
        contents = source.read_text(encoding="utf-8")

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0303]", result.stderr, result.stderr)
        self.assertEqual(contents, original)

    @unittest.skip(CRANELIFT_AND_RYN_LINKER)
    def test_failed_linker_removes_its_temporary_build_directory(self):
        # Ported shape, kept for the record: `ryn build --backend cranelift` with
        # RYN_LINKER=rustc and RUSTC pointing at the ryn binary. The native backend
        # writes the executable itself, so this path has no equivalent to test here.
        pass

    def test_failed_native_write_reports_r0301_and_leaves_no_build_directory(self):
        directory = self.temp_dir("failed-native-write")
        write_bytes_text(directory / "fixture.ryn", 'fun main() { echo("built") }')
        # An existing directory cannot be opened for writing as the executable.
        (directory / "occupied").mkdir()

        result = run_ryn(
            "build",
            "--backend",
            "native",
            "fixture.ryn",
            "-o",
            "occupied",
            cwd=directory,
            env={"TEMP": str(directory), "TMP": str(directory), "TMPDIR": str(directory)},
        )
        entries = sorted(p.name for p in directory.iterdir())
        occupied_is_still_a_directory = (directory / "occupied").is_dir()

        self.assertNotEqual(result.code, 0)
        self.assertIn("error[R0301]: cannot write the executable occupied", result.stderr, result.stderr)
        self.assertTrue(occupied_is_still_a_directory)
        self.assertEqual(entries, ["fixture.ryn", "occupied"])

    @unittest.skip(RUST_LIBRARY_API)
    def test_failed_link_preserves_an_existing_output_file(self):
        pass

    def test_concurrent_builds_to_the_same_output_leave_a_complete_executable(self):
        directory = self.temp_dir("concurrent-output")
        first_source = directory / "first.ryn"
        second_source = directory / "second.ryn"
        output = directory / ("shared.exe" if IS_WINDOWS else "shared")
        write_bytes_text(first_source, 'fun main() { echo("first build") }')
        write_bytes_text(second_source, 'fun main() { echo("second build") }')

        binary = ryn_binary()
        first = subprocess.Popen(
            [binary, "build", str(first_source), "--output", str(output)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        second = subprocess.Popen(
            [binary, "build", str(second_source), "--output", str(output)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        first_out, first_err = first.communicate()
        second_out, second_err = second.communicate()
        self.assertEqual(
            first.returncode, 0,
            f"first build failed: {first_err.decode('utf-8', errors='replace')}",
        )
        self.assertEqual(
            second.returncode, 0,
            f"second build failed: {second_err.decode('utf-8', errors='replace')}",
        )

        run = run_program(output)
        self.assertEqual(run.code, 0)
        self.assertTrue(
            run.stdout == "first build\n" or run.stdout == "second build\n",
            f"unexpected output from concurrent build: {run.stdout!r}",
        )

    def test_variables_mutation_and_arithmetic_run_as_native_code(self):
        result = self.run_source(repo_path("examples/variables.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "Ryn\n80\n42\n")

    def test_fixed_width_integer_arithmetic_wraps_and_signed_division_overflow_is_defined(self):
        result = self.run_source(repo_path("examples/integer_semantics.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "-128\n127\n-128\n-128\n-3\n-1\n-128\n0\n-32768\n0\n-2147483648\n0\n"
            "-9223372036854775808\n0\n127\n1\n255\n255\n429496729\n5\n1844674407370955161\n5\n",
        )

    def test_integer_division_and_remainder_by_zero_exit_with_a_runtime_diagnostic(self):
        cases = [
            (
                "signed-division-by-zero",
                "fun main() {\n    numerator := 8\n    zero := 0\n    echo(numerator / zero)\n}",
                "",
            ),
            (
                "unsigned-division-by-zero",
                "fun main() {\n    numerator: u8 = 8\n    zero: u8 = 0\n    echo(numerator / zero)\n}",
                "",
            ),
            (
                "signed-remainder-by-zero",
                "fun main() {\n    numerator := 8\n    zero := 0\n    echo(numerator % zero)\n}",
                "",
            ),
            (
                "unsigned-remainder-by-zero",
                "fun main() {\n    numerator: u8 = 8\n    zero: u8 = 0\n    echo(numerator % zero)\n}",
                "",
            ),
            (
                "output-before-division-by-zero",
                "fun main() {\n    echo(123)\n    numerator := 8\n    zero := 0\n    echo(numerator / zero)\n}",
                "123\n",
            ),
        ]
        for label, source, expected_stdout in cases:
            with self.subTest(label=label):
                result = self.run_source_text(label, source)
                self.assertEqual(result.code, 1, label)
                self.assertEqual(result.stdout, expected_stdout, label)
                self.assertIn(
                    "Ryn runtime error: integer division or remainder by zero",
                    result.stderr,
                    f"{label}: {result.stderr}",
                )

    def test_short_circuit_skips_integer_division_by_zero(self):
        result = self.run_source_text(
            "short-circuit-division",
            "fun divide_by_zero() -> bool {\n    numerator := 1\n    zero := 0\n"
            "    numerator / zero == 0\n}\nfun main() {\n    echo(false && divide_by_zero())\n"
            "    echo(true || divide_by_zero())\n}",
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "false\ntrue\n")
        self.assertEqual(result.stderr, "")

    def test_for_ranges_are_exclusive_evaluate_bounds_once_and_route_loop_control(self):
        result = self.run_source(repo_path("examples/for_ranges.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "0\n1\n3\n4\n8\nlower\nupper\n1\n2\n3\n254\n")

    def test_nested_for_loops_route_break_and_continue_to_the_innermost_range(self):
        result = self.run_source_text(
            "nested-for-loops",
            "fun main() {\n    mut total := 0\n    for outer in 0..3 {\n        for inner in 0..4 {\n"
            "            when inner == 1 { continue }\n            when inner == 3 { break }\n"
            "            total += 1\n        }\n        when outer == 1 { continue }\n"
            "        echo(outer)\n    }\n    echo(total)\n}",
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "0\n2\n6\n")

    def test_conditions_and_comparisons_run_as_native_code(self):
        result = self.run_source(repo_path("examples/conditions.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "rank up\n15\ntrue\n1\nboolean works\n")

    def test_while_loop_repeats_and_updates_native_locals(self):
        result = self.run_source(repo_path("examples/loops.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "0\n2\n4\n10\n")

    def test_break_and_continue_target_the_innermost_while_loop(self):
        result = self.run_source(repo_path("examples/loop_control.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "1\n3\n4\n11\n21\n2\n")

    def test_early_returns_run_from_branches_and_loop_bodies(self):
        result = self.run_source(repo_path("examples/early_return.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "7\n4\n10\n20\nenabled\ndisabled\n3\n-1\nnotification\n",
        )

    def test_if_expressions_return_native_values_for_each_supported_type(self):
        result = self.run_source(repo_path("examples/if_expression.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "selected\n1.5\ntrue\n1\n1\n22\n7\n")

    def test_else_if_chains_run_through_nested_if_lowering(self):
        result = self.run_source(repo_path("examples/else_if.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "negative\nzero\npositive\n-1\n0\n1\n")

    def test_typed_functions_calls_and_return_values_run_as_native_code(self):
        result = self.run_source(repo_path("examples/functions.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "void call\n42\n42\nnative calls\ntrue\nfalse\ntrue\nrhs\ntrue\nrhs\ntrue\n",
        )

    def test_recursive_functions_run_as_native_code(self):
        result = self.run_source(repo_path("examples/recursion.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "3\n2\n1\n3\n")

    def test_typed_functions_can_call_void_functions_before_their_result(self):
        result = self.run_source(repo_path("examples/function_statements.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "before result\n40\n41\n")

    def test_string_arguments_and_results_preserve_utf8_and_empty_values(self):
        result = self.run_source(repo_path("examples/string_abi.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "Łódź 🦀\nUTF-8 ✓\n\n")

    def test_function_arguments_and_binary_operands_evaluate_left_to_right(self):
        result = self.run_source(repo_path("examples/evaluation_order.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "1\n2\n12\n3\n4\n7\n")

    def test_structure_field_initializers_evaluate_in_source_order(self):
        directory = self.temp_dir("structure-evaluation-order")
        source = directory / "evaluation_order.ryn"
        write_bytes_text(
            source,
            "struct Pair { first: i32 second: i32 } "
            'fun first() -> i32 { echo("first") 1 } '
            'fun second() -> i32 { echo("second") 2 } '
            "fun main() { pair := Pair { second: second(), first: first() } echo(pair) }",
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "second\nfirst\nPair { first: 1, second: 2 }\n")

    def test_structures_compare_all_nested_fields_by_value(self):
        directory = self.temp_dir("structure-equality")
        source = directory / "equality.ryn"
        write_bytes_text(
            source,
            "struct Meta { label: str active: bool weight: f32 ratio: f64 } "
            "struct Item { id: i32 meta: Meta } "
            "fun identity(value: Item) -> Item { value } "
            "fun main() { "
            'first := Item { id: 7, meta: Meta { label: "Ryn", active: true, weight: 1.25, ratio: 2.5 } } '
            'same := Item { id: 7, meta: Meta { label: "Ryn", active: true, weight: 1.25, ratio: 2.5 } } '
            'different := Item { id: 7, meta: Meta { label: "Ryn", active: false, weight: 1.25, ratio: 2.5 } } '
            "echo(first == same) echo(first != different) echo(first == different) "
            "echo(first != same) echo(first == identity(same)) }",
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "true\ntrue\nfalse\nfalse\ntrue\n")

    def test_compound_assignments_update_nested_structure_fields(self):
        directory = self.temp_dir("structure-compound-assign")
        source = directory / "compound.ryn"
        write_bytes_text(
            source,
            "struct Counter { value: i32 } fun main() { mut counter := Counter { value: 3 } "
            "counter.value += 4 counter.value *= 2 counter.value -= 4 counter.value /= 2 "
            "counter.value %= 3 counter.value |= 8 counter.value ^= 3 counter.value &= 11 "
            "counter.value <<= 1 counter.value >>= 2 echo(counter.value) }",
        )

        result = self.run_source(source)
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "4\n")

    def test_i32_values_arithmetic_comparisons_and_abi_run_as_native_code(self):
        result = self.run_source(repo_path("examples/i32.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "43\n-5\n-2147483648\n2147483647\ntrue\n")

    def test_f32_and_f64_values_run_through_native_abi_and_codegen(self):
        result = self.run_source(repo_path("examples/floats.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "2.5\n-2.5\n3\n-3\n0.125\ntrue\ntrue\n1.25\ninf\n-inf\nNaN\n-0\nfalse\ntrue\n"
            "false\nfalse\nfalse\nfalse\ntrue\ninf\n-inf\nNaN\nfalse\ntrue\nfalse\n",
        )

    def test_all_signed_and_unsigned_integer_widths_run_as_native_code(self):
        result = self.run_source(repo_path("examples/integer_widths.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "127\n32767\n2147483647\n9223372036854775807\n255\n65535\n4294967295\n"
            "18446744073709551615\n9223372036854775807\n-5\n5\n5\ntrue\ntrue\ntrue\ntrue\ntrue\n"
            "-128\n-32768\n-2147483648\n-9223372036854775808\n",
        )

    def test_numeric_separators_preserve_integer_and_float_values_in_native_code(self):
        result = self.run_source_text(
            "numeric-separators",
            "fun main() { count: i64 = 1_000_000 echo(count) value: f64 = 1_234.5_6e2 "
            "echo(value == 123456.0) }",
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "1000000\ntrue\n")

    def test_binary_and_hexadecimal_literals_run_as_native_integers(self):
        result = self.run_source_text(
            "radix-integers",
            "/* radix literals /* nested comment */ compile natively */ fun main() { "
            "byte: u8 = 0b1111_1111 /* between statements */ mask: u16 = 0xCA_FE "
            "echo(byte) echo(mask) echo(0x2a) }",
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "255\n51966\n42\n")

    def test_bitwise_operations_preserve_integer_width_and_operator_precedence(self):
        result = self.run_source(repo_path("examples/bitwise.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "8\n14\n6\n245\n4\ntrue\n11\n48\n3\n0\n24\n-16\n-1\n3\n6\n4\n2\nfalse\n",
        )

    def test_shifts_cover_each_integer_width_and_out_of_range_counts(self):
        result = self.run_source_text(
            "integer-shifts",
            "fun main() { si16: i16 = 1 echo(si16 << 15) sn16: i16 = -1 echo(sn16 >> 16) "
            "ui16: u16 = 1 echo(ui16 << 15) un16: u16 = 65535 echo(un16 >> 16) "
            "si32: i32 = 1 echo(si32 << 31) sn32: i32 = -1 echo(sn32 >> 32) "
            "ui32: u32 = 1 echo(ui32 << 31) un32: u32 = 4294967295 echo(un32 >> 32) "
            "si64: i64 = 1 echo(si64 << 63) sn64: i64 = -1 echo(sn64 >> 64) "
            "ui64: u64 = 1 echo(ui64 << 63) un64: u64 = 18446744073709551615 echo(un64 >> 64) "
            "huge: u32 = 4294967295 signed: i8 = -1 echo(signed >> huge) "
            "unsigned: u8 = 1 echo(unsigned << huge) }",
        )
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "-32768\n-1\n32768\n0\n-2147483648\n-1\n2147483648\n0\n"
            "-9223372036854775808\n-1\n9223372036854775808\n0\n-1\n0\n",
        )

    def test_integer_casts_preserve_or_extend_the_expected_bits(self):
        result = self.run_source(repo_path("examples/integer_casts.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "-1\n65535\n18446744073709551615\n255\n-1\n255\n4\n52\ntrue\ntrue\ntrue\ntrue\n"
            "inf\n12\n-12\n255\n-128\n0\n0\n0\n9223372036854775807\n18446744073709551615\n0\n",
        )

    def test_map_keys_and_values_iterate_with_clone_and_drop_glue(self):
        result = self.run_source(repo_path("examples/map_iteration.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "3\n111\n3\n60\n3\n0\n0\nwindowRyn\n2\n308\n")

    @unittest.skipUnless(IS_WINDOWS, WINDOWS_ONLY)
    def test_ryn_functions_coerce_to_function_pointers_and_receive_native_callbacks(self):
        result = self.run_source(repo_path("examples/function_pointers.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "42\n42\n3\ntrue\n")

    def test_enums_print_directly_and_inside_interpolation(self):
        result = self.run_source(repo_path("examples/enum_formatting.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(
            result.stdout,
            "Empty\nCircle(7)\ncircle: Circle(7)\nNamed(box, 3)\nlabel: Named(box, 3)\n"
            "Circle(9)\nNamed(moved, 8)\n",
        )

    def test_extend_methods_borrow_receivers_and_associated_items_resolve(self):
        result = self.run_source(repo_path("examples/objects.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "100\n75\n100\nHP: 75\nv3nn7\n")

    def test_derived_clone_hash_and_eq_power_struct_keys_and_copies(self):
        result = self.run_source(repo_path("examples/derive.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "2\ntrue\nSome(70)\ntrue\n1\ntag\ntag\n")

    def test_operator_methods_resolve_struct_arithmetic(self):
        result = self.run_source(repo_path("examples/operators.ryn"))
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "11\n22\n33\n2\n27\n1\n")

    def test_copyable_expression_receivers_chain_methods(self):
        result = self.run_source_text(
            "method-chain",
            r"""
            struct Pair { x: f32, y: f32 }

            #[repr(C)]
            struct Wide {
                c0: f32,
                c1: f32,
                c2: f32,
                c3: f32,
                c4: f32,
                c5: f32,
            }

            extend Pair {
                fun make(x: f32, y: f32) -> Pair => Pair { x: x, y: y }
                fun add(self, rhs: Pair) -> Pair => Pair { x: self.x + rhs.x, y: self.y + rhs.y }
            }

            extend Wide {
                fun make(first: f32, last: f32) -> Wide => Wide {
                    c0: first, c1: 10.0, c2: 20.0, c3: 30.0, c4: 40.0, c5: last,
                }
                fun scaled(self, factor: f32) -> f32 {
                    return self.c0 * factor + self.c5
                }
            }

            fun main() {
                sum := Pair::make(1.0, 2.0).add(Pair::make(3.0, 4.0)).add(Pair::make(0.5, 0.5))
                echo sum.x
                echo sum.y
                echo Wide::make(2.0, 3.0).scaled(2.0)
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "4.5\n6.5\n7\n")

    def test_while_condition_reruns_copy_receivers_without_dropping_owners(self):
        result = self.run_source_text(
            "while-receiver",
            r"""
            struct Pair { x: f32, y: f32 }

            extend Pair {
                fun make(x: f32, y: f32) -> Pair => Pair { x: x, y: y }
                fun add(self, rhs: Pair) -> Pair => Pair { x: self.x + rhs.x, y: self.y + rhs.y }
                fun below(self, limit: f32) -> bool => self.x < limit
            }

            fun main() {
                text := String("ab")
                mut count: u64 = 0
                while count < text.len() {
                    count = count + 1
                }
                echo count
                mut base: f32 = 0.0
                mut steps: u64 = 0
                while Pair::make(base, 0.0).add(Pair::make(1.0, 0.0)).below(3.0) && steps < 5 {
                    steps = steps + 1
                    base = base + 1.0
                }
                echo steps
                echo text.len()
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "2\n2\n2\n")

    def test_repr_c_method_fields_use_packed_byte_offsets(self):
        result = self.run_source_text(
            "repr-c-method-fields",
            r"""
            #[repr(C)]
            struct Wide {
                c0: f32,
                c1: f32,
                c2: f32,
                c3: f32,
                c4: f32,
                c5: f32,
            }

            struct Plain {
                c0: f32,
                c1: f32,
                c5: f32,
            }

            extend Wide {
                fun diag(self) -> f32 {
                    return self.c0 * 2.0 + self.c5
                }

                fun set_tail(mut self, value: f32) {
                    self.c5 = value
                }
            }

            extend Plain {
                fun pick(self) -> f32 {
                    return self.c0 + self.c5
                }
            }

            fun main() {
                mut wide := Wide { c0: 2.0, c1: 10.0, c2: 20.0, c3: 30.0, c4: 40.0, c5: 3.0 }
                echo wide.diag()
                wide.set_tail(7.0)
                echo wide.c5
                echo wide.c0
                plain := Plain { c0: 2.0, c1: 10.0, c5: 3.0 }
                echo plain.pick()
            }
        """,
        )
        self.assertRunSucceeded(result)
        self.assertEqual(result.stdout, "7\n7\n2\n5\n")

    def test_receiver_temporaries_in_conditions_run_before_the_branch(self):
        # A copyable receiver such as `V::ZERO` is stored in a temporary. That temporary used to
        # be emitted into the first statement of the `when` branch or the loop body, so the
        # condition read an uninitialized slot.
        result = self.run_source_text(
            "receiver-temporaries",
            r"""struct V { x: i32 }

extend V {
    pub fun add(self, rhs: V) -> V => V { x: self.x + rhs.x }
    pub fun get(self) -> i32 => self.x
    pub const THREE: V = V { x: 3 }
    pub const ZERO: V = V { x: 0 }
}

fun first(v: V) -> i32 => v.x

fun main() -> i32 {
    when first(V::ZERO.add(V::THREE)) != 3 { return 1 }
    when V::ZERO.get() == 1 { return 2 } else when V::ZERO.add(V::THREE).get() == 3 { echo "else-when" } else { return 3 }
    mut total: i32 = 0
    for i in (0 as i32)..V::ZERO.add(V::THREE).get() { total += 1 }
    when total != 3 { return 4 }
    mut n: i32 = 0
    while n < V::ZERO.add(V::THREE).get() { n += 1 }
    when n != 3 { return 5 }
    echo "ok"
    return 0
}
""",
        )
        self.assertEqual(
            result.code,
            0,
            f"exit {result.code}, stderr: {result.stderr}",
        )
        self.assertEqual(result.stdout, "else-when\nok\n")

    def test_raw_pointers_compare_and_literal_range_starts_follow_the_end_type(self):
        result = self.run_source_text(
            "pointer-equality",
            r"""struct Counter { items: [i32; 4] }

extend Counter {
    pub fun count(self) -> u64 => 4
}

fun main() -> i32 {
    lib := load_library("kernel32.dll")
    same := load_library("kernel32.dll")
    missing := load_symbol(lib, "DefinitelyNotAnExport")
    when lib != same { return 1 }
    when lib == missing { return 2 }
    when !pointer_is_null(missing) { return 3 }
    c := Counter { items: [1, 2, 3, 4] }
    mut sum: i32 = 0
    for i in 0..c.count() { sum += c.items[i] }
    when sum != 10 { return 4 }
    unload_library(same)
    unload_library(lib)
    echo "ok"
    return 0
}
""",
        )
        # The library and symbol lookups only exist on Windows; the assertions are Windows-only,
        # as in the Rust test (`if cfg!(windows)`).
        if IS_WINDOWS:
            self.assertEqual(result.code, 0, f"exit {result.code}, stderr: {result.stderr}")
            self.assertEqual(result.stdout, "ok\n")

    def test_standalone_files_import_std_and_sibling_modules(self):
        directory = self.temp_dir("file-imports")
        write_bytes_text(directory / "shapes.ryn", "pub fun sides() -> i32 => 4\n")
        main = directory / "main.ryn"
        write_bytes_text(
            main,
            "use shapes\nuse std::math\n\nfun main() {\n    echo shapes::sides()\n"
            "    echo math::sqrt(16.0)\n}\n",
        )
        result = run_ryn("run", main)
        self.assertEqual(result.code, 0, f"stderr: {result.stderr}")
        self.assertEqual(result.stdout, "4\n4\n")


if __name__ == "__main__":
    unittest.main()
