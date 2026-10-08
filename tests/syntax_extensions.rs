//! Run-time checks for the `|>` pipe and the `??` fallback. Both are rewritten
//! by the parser into existing calls and `choose` expressions, so these tests
//! compile and execute real programs and compare their exact output.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("ryn-{label}-{}-{id}", std::process::id()));
    fs::create_dir(&directory).expect("unique temporary directory is created");
    directory
}

fn run_program(label: &str, source_text: &str, frontend: Option<&str>) -> std::process::Output {
    let directory = temp_dir(label);
    let source = directory.join("main.ryn");
    fs::write(&source, source_text).expect("Ryn source is written");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryn"));
    command.arg("run").arg(&source);
    if let Some(frontend) = frontend {
        command.arg("--frontend").arg(frontend);
    }
    let result = command.output().expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    result
}

fn pipe_and_coalesce_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/pass/pipe_and_coalesce.ryn");
    fs::read_to_string(path).expect("pipe and coalesce fixture exists")
}

const PIPE_AND_COALESCE_OUTPUT: &str = "36\n100\n28\n41\n0\n5\n";

#[test]
fn pipe_and_coalesce_run_with_the_expected_output() {
    let result = run_program("pipe-run", &pipe_and_coalesce_source(), None);
    assert!(
        result.status.success(),
        "pipe and coalesce program should run, stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        PIPE_AND_COALESCE_OUTPUT
    );
}

#[test]
fn bootstrap_frontend_matches_the_expected_output() {
    let result = run_program("pipe-run-rust", &pipe_and_coalesce_source(), Some("rust"));
    assert!(
        result.status.success(),
        "bootstrap frontend should run the program, stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        PIPE_AND_COALESCE_OUTPUT
    );
}

#[test]
fn coalesce_evaluates_the_fallback_only_for_none() {
    let source = r#"
fun fallback_value(counter: i32) -> i32 {
    echo "fallback"
    return counter
}

fun main() -> i32 {
    present: Option<i32> = Option::Some(3)
    missing: Option<i32> = Option::None
    echo present ?? fallback_value(9)
    echo missing ?? fallback_value(11)
    return 0
}
"#;
    let result = run_program("coalesce-lazy", source, None);
    assert!(
        result.status.success(),
        "lazy coalesce should run, stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "3\nfallback\n11\n"
    );
}

#[test]
fn pipe_is_left_associative_and_binds_loosest() {
    let source = r#"
fun inc(x: i32) -> i32 => x + 1

fun main() -> i32 {
    echo 1 + 2 |> inc
    echo 1 |> inc |> inc
    return 0
}
"#;
    let result = run_program("pipe-precedence", source, None);
    assert!(
        result.status.success(),
        "pipe precedence program should run, stderr:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout), "4\n3\n");
}

#[test]
fn inclusive_ranges_include_their_end_and_never_wrap() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/pass/inclusive_ranges.ryn"),
    )
    .expect("inclusive range fixture exists");
    let expected = "10\n6\n256\n-3\n-2\n-1\n5\n";
    for frontend in [None, Some("rust")] {
        let result = run_program("inclusive-range", &source, frontend);
        assert!(
            result.status.success(),
            "inclusive ranges should run with frontend {frontend:?}, stderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&result.stdout), expected);
    }
}
