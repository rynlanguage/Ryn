//! `--backend` selects the code generator, and both generators agree on the same programs.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("ryn-{label}-{}-{id}", std::process::id()));
    fs::create_dir(&directory).expect("unique temporary directory is created");
    directory
}

fn suite_program(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/suite/collections")
        .join(name)
}

/// The build and run of one program with one backend: the compiler's output and the program's output.
fn build_and_run(source: &Path, backend: &str) -> (Output, Option<Output>) {
    let directory = temp_dir("backend");
    let executable = directory.join("program");
    let build = Command::new(
        std::env::var("RYN_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_ryn").to_string()),
    )
    .arg("build")
    .arg("--backend")
    .arg(backend)
    .arg(source)
    .arg("-o")
    .arg(&executable)
    .output()
    .expect("ryn process starts");
    let run = build.status.success().then(|| {
        Command::new(&executable)
            .output()
            .expect("the built program starts")
    });
    let _ = fs::remove_dir_all(directory);
    (build, run)
}

/// The expected stdout lines and exit status, from the program's `// out:` and `// exit:` headers.
fn expectations(source: &Path) -> (String, i32) {
    let text = fs::read_to_string(source).expect("suite program is readable");
    let mut stdout = String::new();
    let mut exit = 0;
    for line in text.lines().take_while(|line| line.starts_with("//")) {
        if let Some(value) = line.strip_prefix("// out: ") {
            stdout.push_str(value);
            stdout.push('\n');
        } else if let Some(value) = line.strip_prefix("// exit: ") {
            exit = value.trim().parse().expect("exit status is a number");
        }
    }
    (stdout, exit)
}

/// Builds the named collections program with each backend and checks its output and exit status against its headers.
fn assert_backends_agree(name: &str) {
    assert_backends_agree_at(&suite_program(name), name);
}

/// The program at `source` built with each backend, checked against its headers.
fn assert_backends_agree_at(source: &Path, name: &str) {
    let (expected_stdout, expected_exit) = expectations(source);
    for backend in ["native", "cranelift"] {
        let (build, run) = build_and_run(source, backend);
        assert!(
            build.status.success(),
            "{name} fails to build with `--backend {backend}`: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = run.expect("a built program was run");
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            expected_stdout,
            "{name} stdout with `--backend {backend}`"
        );
        assert_eq!(
            run.status.code(),
            Some(expected_exit),
            "{name} exit status with `--backend {backend}`"
        );
    }
}

#[test]
fn cranelift_and_native_agree_on_str_view_and_string_byte_access() {
    for name in [
        "str_view_len_and_byte_at.ryn",
        "str_byte_at_out_of_bounds.ryn",
        "string_byte_at_out_of_bounds.ryn",
    ] {
        assert_backends_agree(name);
    }
}

#[test]
fn cranelift_and_native_agree_on_discarded_destructor_values() {
    for name in [
        "discarded_destructor_values_drop_once.ryn",
        "native_nested_destructor_elements.ryn",
    ] {
        assert_backends_agree(name);
    }
}

#[test]
fn cranelift_and_native_agree_on_float_record_shims() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/suite/ffi/float_record_shims_round_trip.ryn");
    assert_backends_agree_at(&source, "float_record_shims_round_trip.ryn");
}

#[test]
fn run_of_a_single_file_leaves_no_executable_beside_it() {
    let directory = temp_dir("run-scratch");
    let source = directory.join("program.ryn");
    fs::write(&source, "fun main() {\n    echo 7\n}\n").expect("source is written");
    let run = Command::new(
        std::env::var("RYN_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_ryn").to_string()),
    )
    .arg("run")
    .arg(&source)
    .output()
    .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "7\n");
    let mut entries: Vec<String> = fs::read_dir(&directory)
        .expect("directory is readable")
        .map(|entry| {
            entry
                .expect("entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["program.ryn".to_string()],
        "run must not leave its executable"
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn native_build_rejects_what_check_rejects() {
    let source = suite_program("immutable_self_string_push_is_r0204.ryn");
    let directory = temp_dir("build-parity");
    let executable = directory.join("program");
    let build = Command::new(
        std::env::var("RYN_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_ryn").to_string()),
    )
    .arg("build")
    .arg(&source)
    .arg("-o")
    .arg(&executable)
    .output()
    .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert_eq!(build.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("error[R0204]"), "{stderr}");
    assert!(
        !executable.exists(),
        "a program that the analysis rejects writes no executable"
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn an_unknown_backend_is_rejected_with_its_diagnostic() {
    let source = suite_program("str_view_len_and_byte_at.ryn");
    let build = Command::new(
        std::env::var("RYN_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_ryn").to_string()),
    )
    .arg("build")
    .arg("--backend")
    .arg("llvm")
    .arg(&source)
    .output()
    .expect("ryn process starts");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert_eq!(build.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("error[R0907]"), "{stderr}");
    assert!(stderr.contains("unknown backend `llvm`"), "{stderr}");
}
