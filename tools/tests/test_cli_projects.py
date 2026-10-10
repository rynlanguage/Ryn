"""Port of tests/cli_projects.rs: project new/check/build/run/clean through the ryn CLI."""

import os
import subprocess
import unittest

from ryn_harness import EXAMPLES, IS_WINDOWS, TempDir, run_ryn


class CliProjectTests(unittest.TestCase):
    def test_project_imports_generic_type_aliases(self):
        project = EXAMPLES / "module_project"
        result = run_ryn("run", project)
        self.assertEqual(result.code, 0, f"project failed: {result.stderr}")
        self.assertEqual(result.stdout, "7\n2\n17\n7\n1\n")

    def test_imported_module_keeps_private_custom_destructor_attached_to_its_type(self):
        project = EXAMPLES / "module_drop_project"
        result = run_ryn("run", project)
        self.assertEqual(result.code, 0, f"project failed: {result.stderr}")
        self.assertEqual(result.stdout, "7\n")

    def test_path_library_exposes_an_owned_path_type_through_a_project_dependency(self):
        project = EXAMPLES / "path_project"
        result = run_ryn("run", project)
        self.assertEqual(result.code, 0, f"project failed: {result.stderr}")
        self.assertEqual(result.stdout, "notes.txt\ntxt\nfalse\ntrue\nhello path\n")

    def test_new_project_can_be_checked_built_and_run_from_its_directory(self):
        with TempDir("project-workflow") as temporary:
            project = temporary / "hello Ryn"

            created = run_ryn("new", project)
            self.assertEqual(created.code, 0, f"project creation failed: {created.stderr}")

            entry = project / "src" / "main.ryn"
            self.assertEqual(
                entry.read_text(encoding="utf-8"),
                'fun main() {\n    echo "Hello, Ryn!"\n}\n',
            )
            manifest = (project / "ryn.yaml").read_text(encoding="utf-8")
            self.assertEqual(
                manifest,
                'name: "hello Ryn"\nversion: 0.1.0\nowner: guest\n\ndependencies:\n\nbuild:\n  optimize: speed\n',
            )
            self.assertFalse((project / "README.md").exists())
            for directory in ["cache", "debug", "release"]:
                self.assertTrue((project / "build" / directory).is_dir())

            checked = run_ryn("check", ".", cwd=project)
            self.assertEqual(checked.code, 0, f"project check failed: {checked.stderr}")
            self.assertIn("main.ryn: ok", checked.stdout)

            built = run_ryn("build", ".", cwd=project)
            self.assertEqual(built.code, 0, f"project build failed: {built.stderr}")
            executable = project / "build" / "debug" / "hello Ryn"
            if IS_WINDOWS:
                executable = executable.with_suffix(".exe")
            self.assertTrue(executable.is_file())

            released = run_ryn("build", "--release", ".", cwd=project)
            self.assertEqual(released.code, 0, f"release build failed: {released.stderr}")
            release_executable = project / "build" / "release" / "hello Ryn"
            if IS_WINDOWS:
                release_executable = release_executable.with_suffix(".exe")
            self.assertTrue(release_executable.is_file())

            run = run_ryn("run", ".", cwd=project)
            self.assertEqual(run.code, 0, f"project run failed: {run.stderr}")
            self.assertEqual(run.stdout, "Hello, Ryn!\n")

    def test_project_manifest_errors_are_reported_before_source_compilation(self):
        with TempDir("invalid-manifest") as temporary:
            (temporary / "ryn.yaml").write_text("version: 0.1.0\n", encoding="utf-8")
            (temporary / "src").mkdir()
            (temporary / "src" / "main.ryn").write_text("fun main() {}", encoding="utf-8")

            result = run_ryn("check", temporary)
            self.assertNotEqual(result.code, 0)
            self.assertIn("invalid", result.stderr, result.stderr)
            self.assertIn("missing field `name`", result.stderr, result.stderr)

    def test_run_propagates_the_i32_main_exit_code_after_flushing_output(self):
        source = EXAMPLES / "exit_code.ryn"
        with TempDir("exit-code") as temporary:
            output = temporary / ("program.exe" if IS_WINDOWS else "program")

            result = run_ryn("run", source, "--output", output)

            self.assertEqual(result.code, 7)
            self.assertEqual(result.stdout, "Ryn exit status: 7\n")
            # The first run after a compiler build prints the one-time note that the self-hosted
            # frontend is being built; any other stderr would be a diagnostic from the program.
            self.assertTrue(
                all(
                    line.startswith("note: building the self-hosted Ryn frontend")
                    for line in result.stderr.splitlines()
                ),
                f"unexpected stderr: {result.stderr}",
            )

    def test_new_refuses_to_overwrite_an_existing_project_directory(self):
        with TempDir("project-existing") as temporary:
            project = temporary / "existing"
            project.mkdir()
            marker = project / "keep.txt"
            marker.write_text("keep me", encoding="utf-8")

            result = run_ryn("new", project)

            self.assertNotEqual(result.code, 0)
            self.assertIn("error[R0400]", result.stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "keep me")

    def test_project_commands_explain_when_the_entry_point_is_missing(self):
        with TempDir("project-no-entry") as temporary:
            (temporary / "ryn.yaml").write_text(
                "name: missing-entry\nversion: 0.1.0\nowner: guest\ndependencies:\nbuild:\n  optimize: speed\n",
                encoding="utf-8",
            )

            result = run_ryn("check", temporary)

            self.assertNotEqual(result.code, 0)
            self.assertIn("project entry point not found:", result.stderr)
            self.assertTrue("src" in result.stderr and "main.ryn" in result.stderr)

    def test_clean_removes_only_the_project_build_directory(self):
        with TempDir("project-clean") as temporary:
            project = temporary / "app"
            source_dir = project / "src"
            build_dir = project / "build" / "nested"
            source_dir.mkdir(parents=True)
            build_dir.mkdir(parents=True)
            (source_dir / "main.ryn").write_text("fun main() {}", encoding="utf-8")
            (build_dir / "app.exe").write_text("generated", encoding="utf-8")
            custom_output = project / "custom-output.exe"
            custom_output.write_text("keep", encoding="utf-8")

            result = run_ryn("clean", project)

            self.assertEqual(result.code, 0, f"clean failed: {result.stderr}")
            self.assertFalse((project / "build").exists())
            self.assertTrue((source_dir / "main.ryn").is_file())
            self.assertEqual(custom_output.read_text(encoding="utf-8"), "keep")

    def test_clean_without_build_artifacts_succeeds_without_changing_the_project(self):
        with TempDir("project-clean-empty") as temporary:
            result = run_ryn("clean", temporary)

            self.assertEqual(result.code, 0)
            self.assertIn("no build artifacts", result.stdout)
            self.assertTrue(temporary.is_dir())

    def test_clean_refuses_to_delete_a_file_named_build(self):
        with TempDir("project-clean-build-file") as temporary:
            build_file = temporary / "build"
            build_file.write_text("keep", encoding="utf-8")

            result = run_ryn("clean", temporary)

            self.assertNotEqual(result.code, 0)
            self.assertIn("non-directory", result.stderr)
            self.assertEqual(build_file.read_text(encoding="utf-8"), "keep")

    @unittest.skipIf(IS_WINDOWS, "symbolic-link test is POSIX-only; see the junction test on Windows")
    def test_clean_refuses_a_build_symlink_and_preserves_its_target(self):
        with TempDir("project-clean-symlink") as temporary:
            project = temporary / "project"
            outside = temporary / "outside"
            project.mkdir()
            outside.mkdir()
            marker = outside / "keep.txt"
            marker.write_text("keep", encoding="utf-8")
            os.symlink(outside, project / "build")

            result = run_ryn("clean", project)

            self.assertNotEqual(result.code, 0)
            self.assertIn("symbolic link", result.stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "keep")

    @unittest.skipUnless(IS_WINDOWS, "junction test is Windows-only")
    def test_clean_refuses_a_build_junction_and_preserves_its_target(self):
        with TempDir("project-clean-junction") as temporary:
            project = temporary / "project"
            outside = temporary / "outside"
            project.mkdir()
            outside.mkdir()
            marker = outside / "keep.txt"
            marker.write_text("keep", encoding="utf-8")
            junction = project / "build"
            created = subprocess.run(
                ["cmd.exe", "/C", "mklink", "/J", str(junction), str(outside)],
                capture_output=True,
            )
            self.assertEqual(
                created.returncode,
                0,
                f"junction creation failed: {created.stderr.decode(errors='replace')}",
            )

            result = run_ryn("clean", project)

            self.assertNotEqual(result.code, 0)
            self.assertIn("symbolic link", result.stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "keep")
            # Removes the junction itself without touching its target.
            os.rmdir(junction)


if __name__ == "__main__":
    unittest.main()
