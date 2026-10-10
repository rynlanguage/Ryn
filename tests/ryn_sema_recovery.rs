//! Recovery after an error. Every independent error of a program is reported, and a failed statement changes neither the
//! statements after it nor the functions after its function. Each program runs through `ryn check` with the bootstrap
//! analysis (`RYN_SEMA=rust`) and with the Ryn analysis alone (`RYN_SEMA=ryn-only`), and both must report the same errors.

use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// The `error[...]` lines that `ryn check` writes for `source` with the given analysis.
fn errors(source: &str, sema: &str) -> Vec<String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-recovery-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, source).expect("temporary Ryn source is written");
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .env("RYN_SEMA", sema)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !result.status.success(),
        "{sema}: the program has errors and must be rejected\n{stderr}"
    );
    stderr
        .lines()
        .filter(|line| line.starts_with("error["))
        .map(str::to_owned)
        .collect()
}

/// Asserts that both analyses report exactly `expected` for `source`, in order.
fn assert_reports(source: &str, expected: &[&str]) {
    for sema in ["rust", "ryn-only"] {
        let reported = errors(source, sema);
        assert_eq!(reported, expected, "RYN_SEMA={sema} for:\n{source}");
    }
}

#[test]
fn each_failed_statement_is_reported_and_the_next_one_is_still_analyzed() {
    assert_reports(
        "fun main() {\n    echo(first_missing)\n    echo(second_missing)\n}\n",
        &[
            "error[R0203]: unknown variable `first_missing`",
            "error[R0203]: unknown variable `second_missing`",
        ],
    );
}

#[test]
fn a_failed_let_hides_its_name_from_the_rest_of_the_block_without_a_second_error() {
    assert_reports(
        "fun main() {\n    value := missing\n    echo(value)\n}\n",
        &["error[R0203]: unknown variable `missing`"],
    );
}

#[test]
fn a_failed_block_does_not_leak_its_locals_into_the_next_function() {
    assert_reports(
        "fun first() {\n    when true {\n        value := missing_one\n    }\n}\nfun second() {\n    value := 1\n    echo(value)\n}\nfun main() {}\n",
        &["error[R0203]: unknown variable `missing_one`"],
    );
}

#[test]
fn a_failed_return_is_not_followed_by_a_missing_return_error() {
    assert_reports(
        "fun f() -> i32 {\n    return missing\n}\nfun main() {}\n",
        &["error[R0203]: unknown variable `missing`"],
    );
}

#[test]
fn a_loop_body_is_analyzed_after_its_header_has_failed() {
    assert_reports(
        "fun main() {\n    for i in 0..3 {\n        echo(missing_in_loop)\n    }\n    echo(after_loop_missing)\n}\n",
        &[
            "error[R0203]: unknown variable `missing_in_loop`",
            "error[R0203]: unknown variable `after_loop_missing`",
        ],
    );
}
