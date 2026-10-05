use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("ryn-{label}-{}-{id}", std::process::id()));
    fs::create_dir(&directory).expect("unique temporary directory is created");
    directory
}

fn run_source(source: &Path) -> std::process::Output {
    let directory = temp_dir("test-run");
    let staged_source = directory.join(source.file_name().unwrap_or_default());
    fs::copy(source, &staged_source).expect("Ryn source is staged in the temporary directory");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(staged_source)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    result
}

fn run_source_text(label: &str, source_text: &str) -> std::process::Output {
    let directory = temp_dir(label);
    let source = directory.join("main.ryn");
    fs::write(&source, source_text).expect("Ryn source is written");
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&source)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    result
}

#[test]
fn hello_ryn_runs_through_native_backend() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/hello.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "Hello, Ryn!"
    );
}

#[test]
fn quick_start_example_runs_through_native_backend() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/quick_start.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "alive\nHello, Ryn!\n42\n80\n"
    );
}

#[test]
fn run_forwards_arguments_to_native_program_including_empty_and_unicode_values() {
    let directory = temp_dir("process-arguments");
    let source = directory.join("arguments.ryn");
    fs::write(
        &source,
        "fn selected_index() -> u32 { print(\"index evaluated once\") 0 }\nfn main() { print(arg_count()) print(arg(0)) print(arg(1)) print(arg(2)) print(arg(3)) print(arg(selected_index())) }",
    )
    .expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&source)
        .arg("--")
        .args(["with spaces", "", "Zażółć 🦀"])
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "3\nwith spaces\n\nZażółć 🦀\n\nindex evaluated once\nwith spaces\n"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn string_escapes_are_preserved_in_native_output() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/string_escapes.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        result.stdout,
        "first\0second\nquote: \" and slash: \\\nUnicode: 🦀\n".as_bytes()
    );
}

#[test]
fn structures_are_constructed_accessed_mutated_and_returned_by_value() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/structs.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Veni\n65\ntrue\n100\nPlayer { name: Veni, vitals: Vitals { health: 100, alive: true } }\nupdated=Player { name: Veni, vitals: Vitals { health: 65, alive: true } }\n"
    );
}

#[test]
fn print_interpolation_writes_text_and_typed_values_in_order() {
    let directory = temp_dir("interpolation");
    let source = directory.join("interpolation.ryn");
    fs::write(
        &source,
        "fn main() { let name = \"Ryn\" let version: f32 = 0.5 let ready = true print(\"Hello {name}: {version} {ready} {{done}}\") }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "Hello Ryn: 0.5 true {done}\n");
}

#[test]
fn compound_assignments_update_native_integer_locals() {
    let directory = temp_dir("compound-assign");
    let source = directory.join("compound.ryn");
    fs::write(
        &source,
        "fn main() { let mut value: i32 = 3 value += 4 value *= 2 value -= 4 value /= 2 value %= 2 value |= 8 value ^= 3 value &= 6 print(value) }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "2\n");
}

#[test]
fn equality_compares_boolean_values_and_string_contents() {
    let directory = temp_dir("equality");
    let source = directory.join("equality.ryn");
    fs::write(
        &source,
        "fn same(left: str, right: str) -> bool { left == right } fn main() { let first = \"cat\" let same = \"cat\" let other = \"car\" print(first == same) print(first != other) print(\"\" == \"\") print(true == true) print(false != true) print(same(\"cat\", \"cat\")) }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
fn pass_fixture_runs_as_a_native_program() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/programs/pass/compound_interpolation.ryn");
    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "health=80\n");
}

#[test]
fn build_preserves_existing_sibling_intermediate_named_files() {
    let directory = temp_dir("build-output");

    let source = directory.join("fixture.ryn");
    let object_paths = [directory.join("fixture.obj"), directory.join("fixture.o")];
    let wrapper = directory.join("fixture.ryn-wrapper.rs");
    let pdb = directory.join("fixture.pdb");
    fs::write(&source, "fn main() { print(\"built\") }").expect("source is written");
    for object in &object_paths {
        fs::write(object, "keep object").expect("object sentinel is written");
    }
    fs::write(&wrapper, "keep wrapper").expect("wrapper sentinel is written");
    fs::write(&pdb, "keep debug symbols").expect("PDB sentinel is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg("fixture.ryn")
        .current_dir(&directory)
        .output()
        .expect("ryn process starts");
    let output = if cfg!(windows) {
        directory.join("fixture.exe")
    } else {
        directory.join("fixture")
    };
    let output_exists = output.exists();
    let object_contents: Vec<_> = object_paths
        .iter()
        .map(|object| fs::read_to_string(object).expect("object sentinel remains"))
        .collect();
    let wrapper_contents = fs::read_to_string(&wrapper).expect("wrapper sentinel remains");
    let pdb_contents = fs::read_to_string(&pdb).expect("PDB sentinel remains");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler failed: {stderr}");
    assert!(output_exists, "native executable was not created");
    assert_eq!(object_contents, ["keep object", "keep object"]);
    assert_eq!(wrapper_contents, "keep wrapper");
    assert_eq!(pdb_contents, "keep debug symbols");
}

#[test]
fn build_accepts_a_custom_output_path() {
    let directory = temp_dir("custom-output");
    let source = directory.join("fixture.ryn");
    let output = directory.join("artifacts").join(if cfg!(windows) {
        "custom.exe"
    } else {
        "custom"
    });
    fs::write(&source, "fn main() { print(\"custom build\") }").expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "compiler failed: {stderr}");
    assert!(output.exists(), "custom executable was not created");

    let run = Command::new(&output)
        .output()
        .expect("custom executable starts");
    let _ = fs::remove_dir_all(&directory);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "custom build\n");
}

#[test]
fn build_and_run_accept_source_and_output_paths_with_spaces() {
    let directory = temp_dir("paths-with-spaces");
    let workspace = directory.join("workspace with spaces");
    fs::create_dir(&workspace).expect("workspace with spaces is created");
    let source = workspace.join("source program.ryn");
    let output = workspace.join("output directory").join(if cfg!(windows) {
        "native program.exe"
    } else {
        "native program"
    });
    fs::write(&source, "fn main() { print(\"spaces-ok\") }").expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "compiler failed: {stderr}");
    assert!(output.exists(), "native executable was not created");

    let run = Command::new(&output)
        .output()
        .expect("native executable with spaces in its path starts");
    let _ = fs::remove_dir_all(&directory);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "spaces-ok\n");
}

#[test]
fn run_accepts_a_custom_output_path_and_keeps_the_executable() {
    let directory = temp_dir("custom-run-output");
    let source = directory.join("fixture with spaces.ryn");
    let output = directory
        .join("named executable")
        .with_extension(if cfg!(windows) { "exe" } else { "" });
    fs::write(&source, "fn main() { print(\"custom run\") }").expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "custom run\n");
    assert!(output.exists(), "run did not keep the custom executable");

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn run_accepts_a_relative_output_filename_without_a_directory() {
    let directory = temp_dir("relative-run-output");
    let source = directory.join("fixture.ryn");
    let output = if cfg!(windows) {
        "ryn-relative-output.exe"
    } else {
        "ryn-relative-output"
    };
    fs::write(&source, "fn main() { print(\"relative run\") }").expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .current_dir(&directory)
        .args(["run", "fixture.ryn", "--output", output])
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "relative run\n");
    assert!(
        directory.join(output).exists(),
        "run did not keep the executable"
    );

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn custom_output_cannot_overwrite_the_source_file() {
    let directory = temp_dir("source-output-collision");
    let source = directory.join("fixture.ryn");
    let original = "fn main() { print(\"source remains intact\") }";
    fs::write(&source, original).expect("Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&source)
        .arg("--output")
        .arg(&source)
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    if cfg!(windows) {
        let case_variant = PathBuf::from(source.to_string_lossy().to_uppercase());
        let alias_result = Command::new(env!("CARGO_BIN_EXE_ryn"))
            .arg("build")
            .arg(&source)
            .arg("-o")
            .arg(case_variant)
            .output()
            .expect("ryn process starts");
        let alias_stderr = String::from_utf8_lossy(&alias_result.stderr);
        assert!(!alias_result.status.success());
        assert!(alias_stderr.contains("error[R0303]"), "{alias_stderr}");
    }
    let contents = fs::read_to_string(&source).expect("source remains readable");
    let _ = fs::remove_dir_all(&directory);

    assert!(!result.status.success());
    assert!(stderr.contains("error[R0303]"), "{stderr}");
    assert_eq!(contents, original);
}

#[test]
fn failed_linker_removes_its_temporary_build_directory() {
    let directory = temp_dir("failed-link");
    fs::write(
        directory.join("fixture.ryn"),
        "fn main() { print(\"built\") }",
    )
    .expect("source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg("fixture.ryn")
        .current_dir(&directory)
        .env("RUSTC", env!("CARGO_BIN_EXE_ryn"))
        .env("TEMP", &directory)
        .env("TMP", &directory)
        .env("TMPDIR", &directory)
        .output()
        .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let leaked_build_dirs: Vec<_> = fs::read_dir(&directory)
        .expect("temporary workspace remains readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("ryn-build-"))
        .collect();
    let _ = fs::remove_dir_all(&directory);

    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0300]: native link step failed"),
        "{stderr}"
    );
    assert!(
        leaked_build_dirs.is_empty(),
        "temporary build directories leaked: {leaked_build_dirs:?}"
    );
}

#[test]
fn failed_link_preserves_an_existing_output_file() {
    const CHILD_OUTPUT_PATH: &str = "RYN_FAILED_LINK_OUTPUT_PATH";
    if let Some(output) = std::env::var_os(CHILD_OUTPUT_PATH) {
        let output = PathBuf::from(output);
        let error = ryn::codegen::link_object(b"not a valid object file", &output)
            .expect_err("invalid object data must fail to link");
        let contents = fs::read(&output).expect("existing output remains readable");
        assert!(error.to_string().contains("error[R0300]"), "{error}");
        assert_eq!(contents, b"existing executable contents");
        return;
    }

    let directory = temp_dir("failed-link-preserves-output");
    let output = directory.join(if cfg!(windows) {
        "existing.exe"
    } else {
        "existing"
    });
    let original = b"existing executable contents";
    fs::write(&output, original).expect("existing output is written");

    let child = Command::new(std::env::current_exe().expect("test executable path is available"))
        .arg("--exact")
        .arg("failed_link_preserves_an_existing_output_file")
        .arg("--nocapture")
        .env(CHILD_OUTPUT_PATH, &output)
        .output()
        .expect("link-failure child test starts");
    let stderr = String::from_utf8_lossy(&child.stderr);
    assert!(child.status.success(), "child test failed: {stderr}");
    let contents = fs::read(&output).expect("existing output remains readable");
    assert_eq!(contents, original);

    let ir = ryn::check("fn main() { print(\"new executable\") }")
        .expect("replacement Ryn source checks");
    ryn::codegen::build_native(&ir, &output).expect("successful link replaces the old output");
    let run = Command::new(&output)
        .output()
        .expect("replacement executable starts");
    let _ = fs::remove_dir_all(&directory);
    assert!(run.status.success());
    assert_eq!(run.stdout, b"new executable\n");
}

#[test]
fn concurrent_builds_to_the_same_output_leave_a_complete_executable() {
    let directory = temp_dir("concurrent-output");
    let first_source = directory.join("first.ryn");
    let second_source = directory.join("second.ryn");
    let output = directory.join(if cfg!(windows) {
        "shared.exe"
    } else {
        "shared"
    });
    fs::write(&first_source, "fn main() { print(\"first build\") }")
        .expect("first source is written");
    fs::write(&second_source, "fn main() { print(\"second build\") }")
        .expect("second source is written");

    let first = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&first_source)
        .arg("--output")
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("first ryn process starts");
    let second = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&second_source)
        .arg("--output")
        .arg(&output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("second ryn process starts");
    let first = first.wait_with_output().expect("first build completes");
    let second = second.wait_with_output().expect("second build completes");
    assert!(
        first.status.success(),
        "first build failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "second build failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );

    let run = Command::new(&output)
        .output()
        .expect("shared output executable starts");
    let _ = fs::remove_dir_all(&directory);
    assert!(run.status.success());
    assert!(
        run.stdout == b"first build\n" || run.stdout == b"second build\n",
        "unexpected output from concurrent build: {:?}",
        String::from_utf8_lossy(&run.stdout)
    );
}

#[test]
fn variables_mutation_and_arithmetic_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/variables.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "Ryn\n80\n42\n");
}

#[test]
fn fixed_width_integer_arithmetic_wraps_and_signed_division_overflow_is_defined() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/integer_semantics.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "-128\n127\n-128\n-128\n-3\n-1\n-128\n0\n-32768\n0\n-2147483648\n0\n-9223372036854775808\n0\n127\n1\n255\n255\n429496729\n5\n1844674407370955161\n5\n"
    );
}

#[test]
fn integer_division_and_remainder_by_zero_exit_with_a_runtime_diagnostic() {
    for (label, source, expected_stdout) in [
        (
            "signed-division-by-zero",
            "fn main() {\n    let numerator = 8\n    let zero = 0\n    print(numerator / zero)\n}",
            &b""[..],
        ),
        (
            "unsigned-division-by-zero",
            "fn main() {\n    let numerator: u8 = 8\n    let zero: u8 = 0\n    print(numerator / zero)\n}",
            &b""[..],
        ),
        (
            "signed-remainder-by-zero",
            "fn main() {\n    let numerator = 8\n    let zero = 0\n    print(numerator % zero)\n}",
            &b""[..],
        ),
        (
            "unsigned-remainder-by-zero",
            "fn main() {\n    let numerator: u8 = 8\n    let zero: u8 = 0\n    print(numerator % zero)\n}",
            &b""[..],
        ),
        (
            "output-before-division-by-zero",
            "fn main() {\n    print(123)\n    let numerator = 8\n    let zero = 0\n    print(numerator / zero)\n}",
            &b"123\n"[..],
        ),
    ] {
        let result = run_source_text(label, source);
        assert_eq!(result.status.code(), Some(1), "{label}");
        assert_eq!(result.stdout, expected_stdout, "{label}");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("Ryn runtime error: integer division or remainder by zero"),
            "{label}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn short_circuit_skips_integer_division_by_zero() {
    let result = run_source_text(
        "short-circuit-division",
        "fn divide_by_zero() -> bool {\n    let numerator = 1\n    let zero = 0\n    numerator / zero == 0\n}\nfn main() {\n    print(false && divide_by_zero())\n    print(true || divide_by_zero())\n}",
    );

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "false\ntrue\n");
    assert!(result.stderr.is_empty());
}

#[test]
fn for_ranges_are_exclusive_evaluate_bounds_once_and_route_loop_control() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/for_ranges.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "0\n1\n3\n4\n8\nlower\nupper\n1\n2\n3\n254\n"
    );
}

#[test]
fn nested_for_loops_route_break_and_continue_to_the_innermost_range() {
    let result = run_source_text(
        "nested-for-loops",
        "fn main() {\n    let mut total = 0\n    for outer in 0..3 {\n        for inner in 0..4 {\n            if inner == 1 { continue }\n            if inner == 3 { break }\n            total += 1\n        }\n        if outer == 1 { continue }\n        print(outer)\n    }\n    print(total)\n}",
    );

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "0\n2\n6\n");
}

#[test]
fn conditions_and_comparisons_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/conditions.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "rank up\n15\ntrue\n1\nboolean works\n"
    );
}

#[test]
fn while_loop_repeats_and_updates_native_locals() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/loops.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "0\n2\n4\n10\n");
}

#[test]
fn break_and_continue_target_the_innermost_while_loop() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/loop_control.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "1\n3\n4\n11\n21\n2\n"
    );
}

#[test]
fn early_returns_run_from_branches_and_loop_bodies() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/early_return.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "7\n4\n10\n20\nenabled\ndisabled\n3\n-1\nnotification\n"
    );
}

#[test]
fn if_expressions_return_native_values_for_each_supported_type() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/if_expression.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "selected\n1.5\ntrue\n1\n1\n22\n7\n"
    );
}

#[test]
fn else_if_chains_run_through_nested_if_lowering() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/else_if.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "negative\nzero\npositive\n-1\n0\n1\n"
    );
}

#[test]
fn typed_functions_calls_and_return_values_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/functions.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "void call\n42\n42\nnative calls\ntrue\nfalse\ntrue\nrhs\ntrue\nrhs\ntrue\n"
    );
}

#[test]
fn recursive_functions_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/recursion.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "3\n2\n1\n3\n");
}

#[test]
fn typed_functions_can_call_void_functions_before_their_result() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/function_statements.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "before result\n40\n41\n"
    );
}

#[test]
fn string_arguments_and_results_preserve_utf8_and_empty_values() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/string_abi.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Łódź 🦀\nUTF-8 ✓\n\n"
    );
}

#[test]
fn function_arguments_and_binary_operands_evaluate_left_to_right() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/evaluation_order.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "1\n2\n12\n3\n4\n7\n"
    );
}

#[test]
fn structure_field_initializers_evaluate_in_source_order() {
    let directory = temp_dir("structure-evaluation-order");
    let source = directory.join("evaluation_order.ryn");
    fs::write(
        &source,
        "struct Pair { first: i32 second: i32 } fn first() -> i32 { print(\"first\") 1 } fn second() -> i32 { print(\"second\") 2 } fn main() { let pair = Pair { second: second(), first: first() } print(pair) }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "second\nfirst\nPair { first: 1, second: 2 }\n");
}

#[test]
fn structures_compare_all_nested_fields_by_value() {
    let directory = temp_dir("structure-equality");
    let source = directory.join("equality.ryn");
    fs::write(
        &source,
        "struct Meta { label: str active: bool weight: f32 ratio: f64 } struct Item { id: i32 meta: Meta } fn identity(value: Item) -> Item { value } fn main() { let first = Item { id: 7, meta: Meta { label: \"Ryn\", active: true, weight: 1.25, ratio: 2.5 } } let same = Item { id: 7, meta: Meta { label: \"Ryn\", active: true, weight: 1.25, ratio: 2.5 } } let different = Item { id: 7, meta: Meta { label: \"Ryn\", active: false, weight: 1.25, ratio: 2.5 } } print(first == same) print(first != different) print(first == different) print(first != same) print(first == identity(same)) }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "true\ntrue\nfalse\nfalse\ntrue\n");
}

#[test]
fn compound_assignments_update_nested_structure_fields() {
    let directory = temp_dir("structure-compound-assign");
    let source = directory.join("compound.ryn");
    fs::write(
        &source,
        "struct Counter { value: i32 } fn main() { let mut counter = Counter { value: 3 } counter.value += 4 counter.value *= 2 counter.value -= 4 counter.value /= 2 counter.value %= 3 counter.value |= 8 counter.value ^= 3 counter.value &= 11 counter.value <<= 1 counter.value >>= 2 print(counter.value) }",
    )
    .expect("Ryn source is written");

    let result = run_source(&source);
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let _ = fs::remove_dir_all(&directory);

    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(stdout, "4\n");
}

#[test]
fn i32_values_arithmetic_comparisons_and_abi_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/i32.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "43\n-5\n-2147483648\n2147483647\ntrue\n"
    );
}

#[test]
fn f32_and_f64_values_run_through_native_abi_and_codegen() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/floats.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "2.5\n-2.5\n3\n-3\n0.125\ntrue\ntrue\n1.25\ninf\n-inf\nNaN\n-0\nfalse\ntrue\nfalse\nfalse\nfalse\nfalse\ntrue\ninf\n-inf\nNaN\nfalse\ntrue\nfalse\n"
    );
}

#[test]
fn all_signed_and_unsigned_integer_widths_run_as_native_code() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/integer_widths.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "127\n32767\n2147483647\n9223372036854775807\n255\n65535\n4294967295\n18446744073709551615\n9223372036854775807\n-5\n5\n5\ntrue\ntrue\ntrue\ntrue\ntrue\n-128\n-32768\n-2147483648\n-9223372036854775808\n"
    );
}

#[test]
fn numeric_separators_preserve_integer_and_float_values_in_native_code() {
    let result = run_source_text(
        "numeric-separators",
        "fn main() { let count: i64 = 1_000_000 print(count) let value: f64 = 1_234.5_6e2 print(value == 123456.0) }",
    );

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "1000000\ntrue\n");
}

#[test]
fn binary_and_hexadecimal_literals_run_as_native_integers() {
    let result = run_source_text(
        "radix-integers",
        "/* radix literals /* nested comment */ compile natively */ fn main() { let byte: u8 = 0b1111_1111 /* between statements */ let mask: u16 = 0xCA_FE print(byte) print(mask) print(0x2a) }",
    );

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "255\n51966\n42\n");
}

#[test]
fn bitwise_operations_preserve_integer_width_and_operator_precedence() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/bitwise.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "8\n14\n6\n245\n4\ntrue\n11\n48\n3\n0\n24\n-16\n-1\n3\n6\n4\n2\nfalse\n"
    );
}

#[test]
fn shifts_cover_each_integer_width_and_out_of_range_counts() {
    let result = run_source_text(
        "integer-shifts",
        "fn main() { let si16: i16 = 1 print(si16 << 15) let sn16: i16 = -1 print(sn16 >> 16) let ui16: u16 = 1 print(ui16 << 15) let un16: u16 = 65535 print(un16 >> 16) let si32: i32 = 1 print(si32 << 31) let sn32: i32 = -1 print(sn32 >> 32) let ui32: u32 = 1 print(ui32 << 31) let un32: u32 = 4294967295 print(un32 >> 32) let si64: i64 = 1 print(si64 << 63) let sn64: i64 = -1 print(sn64 >> 64) let ui64: u64 = 1 print(ui64 << 63) let un64: u64 = 18446744073709551615 print(un64 >> 64) let huge: u32 = 4294967295 let signed: i8 = -1 print(signed >> huge) let unsigned: u8 = 1 print(unsigned << huge) }",
    );

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "-32768\n-1\n32768\n0\n-2147483648\n-1\n2147483648\n0\n-9223372036854775808\n-1\n9223372036854775808\n0\n-1\n0\n"
    );
}

#[test]
fn integer_casts_preserve_or_extend_the_expected_bits() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/integer_casts.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "-1\n65535\n18446744073709551615\n255\n-1\n255\n4\n52\ntrue\ntrue\ntrue\ntrue\ninf\n12\n-12\n255\n-128\n0\n0\n0\n9223372036854775807\n18446744073709551615\n0\n"
    );
}
