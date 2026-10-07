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
fn array_repeat_literals_copy_one_evaluation() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/array_repeat.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "65\n65\n0\n1\nryn\n4\n21\n1\n7\n7\n1\n0\nowned\n3\n"
    );
}

#[test]
fn language_additions_from_the_self_hosted_frontend_run_natively() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/language_0_1.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "2
fun
keyword
fun!
233
3
R1
3
path call
"
    );
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
fn enum_payloads_and_choose_execute_through_the_native_backend() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/enums.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "42\n4\n2\n0\nmoved out\n9\n8\n"
    );
}

#[test]
fn repr_c_small_integer_records_use_the_native_c_aggregate_abi() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/ffi_repr_c.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "42\n42\n4\n4\n42\n20\n22\n42\n3579\n1000\n2345\n3345\n42\n12\n30\n42\n42\n20\n22\n42\n"
    );
}

#[test]
fn raw_pointer_to_repr_c_record_with_array_keeps_all_fields_addressable() {
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/raw_pointer_repr_c_array.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "7\n11\n22\n33\n44\n99\n"
    );
}

#[cfg(windows)]
#[test]
fn dynamic_library_symbols_can_be_called_with_a_declared_c_signature() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/dynamic_library.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\n");
}

#[test]
fn raw_pointer_casts_reinterpret_the_same_address() {
    let result = run_source_text(
        "pointer-reinterpret",
        r#"
            fun main() {
                mut value: i32 = 42
                raw := &raw mut value
                bytes := raw as *u8
                restored := bytes as *i32
                echo *restored
                echo pointer_is_null(bytes)
            }
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\nfalse\n");
}

#[test]
fn integer_address_casts_to_a_raw_pointer() {
    let result = run_source_text(
        "int-to-pointer",
        r#"
            fun main() {
                echo pointer_is_null(32512 as *u8)
                echo pointer_is_null(0 as *u8)
            }
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "false\ntrue\n");
}

#[cfg(windows)]
#[test]
fn borrowed_raw_pointer_field_casts_to_a_function_pointer() {
    let result = run_source_text(
        "pointer-field-cast",
        r#"
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
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\n");
}

#[test]
fn generic_struct_instantiations_run_through_the_native_backend() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/generic_structs.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "42\nryn-lang\n1\n9\n"
    );
}

#[test]
fn generic_enum_instantiations_run_through_the_native_backend() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/generic_enums.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "42\n11\n3\n0\n6\n3\n0\n"
    );
}

#[test]
fn slices_of_owned_vec_elements_clone_on_index_and_iteration() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/slices.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "32\n16\nRyn\n4\nRyn\nnested\n27\n"
    );
}

#[test]
fn arrays_of_owned_elements_can_be_borrowed_as_slices() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/array_slices.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "20\n9\n0\nhi\n");
}

#[test]
fn self_hosted_lexer_checks_integer_literal_overflow() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/self_hosted/lexer.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("0\n18446744073709551615\n12\n12\n12\n0\n"),
        "integer boundary and overflow results were missing: {stdout}"
    );
}

#[test]
fn self_hosted_lexer_checks_numeric_ranges_and_unicode_escapes() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/self_hosted/lexer.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
}

#[test]
fn filesystem_path_components_use_host_path_rules() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/filesystem_paths.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.starts_with("build/ryn_fs_read_dir_contract\na.ryn\nryn\nfalse\n.env\n\ngz\n\n2\n"),
        "path component output was unexpected: {stdout}"
    );
}

#[test]
fn references_and_raw_pointers_access_scalar_locals() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/references.ryn");
    let result = run_source(&source);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "10\n27\n32\n33\n");
}

#[test]
fn run_forwards_arguments_to_native_program_including_empty_and_unicode_values() {
    let directory = temp_dir("process-arguments");
    let source = directory.join("arguments.ryn");
    fs::write(
        &source,
        "fun selected_index() -> u32 { echo(\"index evaluated once\") 0 }\nfun main() { echo(arg_count()) echo(arg(0)) echo(arg(1)) echo(arg(2)) echo(arg(3)) echo(arg(selected_index())) }",
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
fn echo_interpolation_writes_text_and_typed_values_in_order() {
    let directory = temp_dir("interpolation");
    let source = directory.join("interpolation.ryn");
    fs::write(
        &source,
        "fun main() { name := \"Ryn\" version: f32 = 0.5 ready := true echo(\"Hello {name}: {version} {ready} {{done}}\") }",
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
        "fun main() { mut value: i32 = 3 value += 4 value *= 2 value -= 4 value /= 2 value %= 2 value |= 8 value ^= 3 value &= 6 echo(value) }",
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
        "fun same(left: str, right: str) -> bool { left == right } fun main() { first := \"cat\" same := \"cat\" other := \"car\" echo(first == same) echo(first != other) echo(\"\" == \"\") echo(true == true) echo(false != true) echo(same(\"cat\", \"cat\")) }",
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
    fs::write(&source, "fun main() { echo(\"built\") }").expect("source is written");
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
    fs::write(&source, "fun main() { echo(\"custom build\") }").expect("Ryn source is written");

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
    fs::write(&source, "fun main() { echo(\"spaces-ok\") }").expect("Ryn source is written");

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
    fs::write(&source, "fun main() { echo(\"custom run\") }").expect("Ryn source is written");

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
    fs::write(&source, "fun main() { echo(\"relative run\") }").expect("Ryn source is written");

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
    let original = "fun main() { echo(\"source remains intact\") }";
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
        "fun main() { echo(\"built\") }",
    )
    .expect("source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg("fixture.ryn")
        .current_dir(&directory)
        .env("RUSTC", env!("CARGO_BIN_EXE_ryn"))
        // Windows links in-process; select the rustc driver so it can fail.
        .env("RYN_LINKER", "rustc")
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

    let ir = ryn::check("fun main() { echo(\"new executable\") }")
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
    fs::write(&first_source, "fun main() { echo(\"first build\") }")
        .expect("first source is written");
    fs::write(&second_source, "fun main() { echo(\"second build\") }")
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
            "fun main() {\n    numerator := 8\n    zero := 0\n    echo(numerator / zero)\n}",
            &b""[..],
        ),
        (
            "unsigned-division-by-zero",
            "fun main() {\n    numerator: u8 = 8\n    zero: u8 = 0\n    echo(numerator / zero)\n}",
            &b""[..],
        ),
        (
            "signed-remainder-by-zero",
            "fun main() {\n    numerator := 8\n    zero := 0\n    echo(numerator % zero)\n}",
            &b""[..],
        ),
        (
            "unsigned-remainder-by-zero",
            "fun main() {\n    numerator: u8 = 8\n    zero: u8 = 0\n    echo(numerator % zero)\n}",
            &b""[..],
        ),
        (
            "output-before-division-by-zero",
            "fun main() {\n    echo(123)\n    numerator := 8\n    zero := 0\n    echo(numerator / zero)\n}",
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
        "fun divide_by_zero() -> bool {\n    numerator := 1\n    zero := 0\n    numerator / zero == 0\n}\nfun main() {\n    echo(false && divide_by_zero())\n    echo(true || divide_by_zero())\n}",
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
        "fun main() {\n    mut total := 0\n    for outer in 0..3 {\n        for inner in 0..4 {\n            when inner == 1 { continue }\n            when inner == 3 { break }\n            total += 1\n        }\n        when outer == 1 { continue }\n        echo(outer)\n    }\n    echo(total)\n}",
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
        "struct Pair { first: i32 second: i32 } fun first() -> i32 { echo(\"first\") 1 } fun second() -> i32 { echo(\"second\") 2 } fun main() { pair := Pair { second: second(), first: first() } echo(pair) }",
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
        "struct Meta { label: str active: bool weight: f32 ratio: f64 } struct Item { id: i32 meta: Meta } fun identity(value: Item) -> Item { value } fun main() { first := Item { id: 7, meta: Meta { label: \"Ryn\", active: true, weight: 1.25, ratio: 2.5 } } same := Item { id: 7, meta: Meta { label: \"Ryn\", active: true, weight: 1.25, ratio: 2.5 } } different := Item { id: 7, meta: Meta { label: \"Ryn\", active: false, weight: 1.25, ratio: 2.5 } } echo(first == same) echo(first != different) echo(first == different) echo(first != same) echo(first == identity(same)) }",
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
        "struct Counter { value: i32 } fun main() { mut counter := Counter { value: 3 } counter.value += 4 counter.value *= 2 counter.value -= 4 counter.value /= 2 counter.value %= 3 counter.value |= 8 counter.value ^= 3 counter.value &= 11 counter.value <<= 1 counter.value >>= 2 echo(counter.value) }",
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
        "fun main() { count: i64 = 1_000_000 echo(count) value: f64 = 1_234.5_6e2 echo(value == 123456.0) }",
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
        "/* radix literals /* nested comment */ compile natively */ fun main() { byte: u8 = 0b1111_1111 /* between statements */ mask: u16 = 0xCA_FE echo(byte) echo(mask) echo(0x2a) }",
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
        "fun main() { si16: i16 = 1 echo(si16 << 15) sn16: i16 = -1 echo(sn16 >> 16) ui16: u16 = 1 echo(ui16 << 15) un16: u16 = 65535 echo(un16 >> 16) si32: i32 = 1 echo(si32 << 31) sn32: i32 = -1 echo(sn32 >> 32) ui32: u32 = 1 echo(ui32 << 31) un32: u32 = 4294967295 echo(un32 >> 32) si64: i64 = 1 echo(si64 << 63) sn64: i64 = -1 echo(sn64 >> 64) ui64: u64 = 1 echo(ui64 << 63) un64: u64 = 18446744073709551615 echo(un64 >> 64) huge: u32 = 4294967295 signed: i8 = -1 echo(signed >> huge) unsigned: u8 = 1 echo(unsigned << huge) }",
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

#[test]
fn map_keys_and_values_iterate_with_clone_and_drop_glue() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/map_iteration.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "3\n111\n3\n60\n3\n0\n0\nRynwindow\n2\n308\n"
    );
}

#[cfg(windows)]
#[test]
fn ryn_functions_coerce_to_function_pointers_and_receive_native_callbacks() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/function_pointers.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\n42\n3\ntrue\n");
}

#[test]
fn enums_print_directly_and_inside_interpolation() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/enum_formatting.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Empty\nCircle(7)\ncircle: Circle(7)\nNamed(box, 3)\nlabel: Named(box, 3)\nCircle(9)\nNamed(moved, 8)\n"
    );
}

#[test]
fn extend_methods_borrow_receivers_and_associated_items_resolve() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/objects.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "100\n75\n100\nHP: 75\nv3nn7\n"
    );
}

#[test]
fn derived_clone_hash_and_eq_power_struct_keys_and_copies() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/derive.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "2\ntrue\nSome(70)\ntrue\n1\ntag\ntag\n"
    );
}

#[test]
fn operator_methods_resolve_struct_arithmetic() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/operators.ryn");
    let result = run_source(&source);

    assert!(
        result.status.success(),
        "compiler/runtime failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "11\n22\n33\n2\n27\n1\n"
    );
}

#[test]
fn copyable_expression_receivers_chain_methods() {
    let result = run_source_text(
        "method-chain",
        r#"
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
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "4.5\n6.5\n7\n");
}

#[test]
fn while_condition_reruns_copy_receivers_without_dropping_owners() {
    let result = run_source_text(
        "while-receiver",
        r#"
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
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "2\n2\n2\n");
}

#[test]
fn repr_c_method_fields_use_packed_byte_offsets() {
    let result = run_source_text(
        "repr-c-method-fields",
        r#"
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
        "#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "compiler/runtime failed: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "7\n7\n2\n5\n");
}

#[test]
fn receiver_temporaries_in_conditions_run_before_the_branch() {
    // A copyable receiver such as `V::ZERO` is stored in a temporary. That temporary used to
    // be emitted into the first statement of the `when` branch or the loop body, so the
    // condition read an uninitialized slot.
    let result = run_source_text(
        "receiver-temporaries",
        r#"struct V { x: i32 }

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
"#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        result.status.success(),
        "exit {:?}, stderr: {stderr}",
        result.status.code()
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "else-when\nok\n");
}

#[test]
fn raw_pointers_compare_and_literal_range_starts_follow_the_end_type() {
    let result = run_source_text(
        "pointer-equality",
        r#"struct Counter { items: [i32; 4] }

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
"#,
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    if cfg!(windows) {
        assert!(
            result.status.success(),
            "exit {:?}, stderr: {stderr}",
            result.status.code()
        );
        assert_eq!(String::from_utf8_lossy(&result.stdout), "ok\n");
    }
}

#[test]
fn standalone_files_import_std_and_sibling_modules() {
    let directory = temp_dir("file-imports");
    fs::write(
        directory.join("shapes.ryn"),
        "pub fun sides() -> i32 => 4\n",
    )
    .unwrap();
    let main = directory.join("main.ryn");
    fs::write(
        &main,
        "use shapes\nuse std::math\n\nfun main() {\n    echo shapes::sides()\n    echo math::sqrt(16.0)\n}\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&main)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(&directory);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "stderr: {stderr}");
    assert_eq!(String::from_utf8_lossy(&result.stdout), "4\n4\n");
}
