use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn check_command_reports_a_missing_source_file_with_its_path() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-missing-source-{}-{unique}.ryn",
        std::process::id()
    ));

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0001]: cannot read"));
    assert!(stderr.contains(&source_path.display().to_string()));
}

#[test]
fn check_command_reports_non_utf8_source_as_a_read_error() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-non-utf8-source-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, [0xff, 0xfe]).expect("invalid UTF-8 source bytes are written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0001]: cannot read"));
    assert!(stderr.contains(&source_path.display().to_string()));
}

#[test]
fn check_command_highlights_a_misplaced_numeric_separator() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-numeric-separator-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { echo(1__2) }").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0008]: numeric separators must appear between digits"),
        "{stderr}"
    );
    assert!(stderr.contains("echo(1__2)"), "{stderr}");
    assert!(stderr.contains("^"), "{stderr}");
}

#[test]
fn check_command_highlights_a_digit_invalid_for_the_literal_base() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-invalid-radix-digit-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { echo(0b2) }").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0009]: digit `2` is not valid in a base-2 integer literal"),
        "{stderr}"
    );
    assert!(stderr.contains("use only `0` and `1`"), "{stderr}");
    assert!(stderr.contains("echo(0b2)"), "{stderr}");
    assert!(stderr.contains("^"), "{stderr}");
}

#[test]
fn check_command_reports_an_unterminated_block_comment() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-unterminated-comment-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { /* unfinished").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0018]: unterminated block comment"),
        "{stderr}"
    );
    assert!(stderr.contains("close the comment with `*/`"), "{stderr}");
    assert!(stderr.contains("/* unfinished"), "{stderr}");
}

#[test]
fn check_command_shows_source_and_caret_for_semantic_errors() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-invalid-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, "fun main() {\n    echo(missing)\n}")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !result.status.success(),
        "invalid source must fail `ryn check`"
    );
    assert!(stderr.contains("error[R0203]: unknown variable `missing`"));
    assert!(stderr.contains(":2:10"));
    assert!(stderr.contains("2 |     echo(missing)"));
    assert!(stderr.contains("|          ^~~~~~~"));
}

#[test]
fn check_command_reports_independent_recursive_structure_layout_cycles() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-layout-cycles-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "struct A { b: B }\nstruct B { a: A }\nstruct C { c: C }\nfun main() {}\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0232]").count(), 2, "{stderr}");
    assert!(stderr.contains("structure `A` contains itself by value"));
    assert!(stderr.contains("structure `C` contains itself by value"));
    assert!(stderr.contains(":2:"));
    assert!(stderr.contains(":3:"));
}

#[test]
fn check_command_renders_diagnostics_for_lone_cr_line_endings() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-cr-lines-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, "fun main() {\r value := missing\r}\r")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0203]: unknown variable `missing`"));
    assert!(stderr.contains(":2:11"));
    assert!(stderr.contains("2 |  value := missing"));
    assert!(stderr.contains(&format!("| {}^~~~~~~", " ".repeat(10))));
}

#[test]
fn check_command_reports_multiple_independent_syntax_errors() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-multiple-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun first() { mut := 1 }\nfun second( { }\nfun main() {}",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 2, "{stderr}");
    assert!(stderr.contains("expected variable name"));
    assert!(stderr.contains("expected parameter name"));
    assert!(stderr.contains(":1:19"));
    assert!(stderr.contains(":2:13"));
}

#[test]
fn check_command_reports_syntax_errors_inside_nested_blocks_together() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-nested-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun main() {\n    mut := 1\n    when true {\n        echo(1 + )\n        mut := 2\n        echo(3)\n    }\n    mut := 4\n}\nfun other() { echo(5 + ) }",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 3, "{stderr}");
    assert_eq!(stderr.matches("error[R0012]").count(), 2, "{stderr}");
    assert_eq!(
        stderr.matches("expected variable name").count(),
        3,
        "{stderr}"
    );
    assert_eq!(stderr.matches("expected expression").count(), 2, "{stderr}");
    assert!(stderr.contains(":2:9"));
    assert!(stderr.contains(":4:18"));
    assert!(stderr.contains(":10:24"));
}

#[test]
fn check_command_reports_body_errors_after_malformed_control_flow_headers() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-control-header-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun main() { when ) { mut := 1 echo(2 + ) } while { mut := 3 } for index in 0.. { mut := 4 echo(5 + ) } for index in 0.. when true { 6 + } else { 7 } { mut := 8 } while when true { 9 + } else { true } { mut := 10 } for index in 0.. when ) { 11 } else { 12 } { mut := 13 } while when ) { true } else { true } { mut := 14 } }",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 7, "{stderr}");
    assert_eq!(stderr.matches("error[R0012]").count(), 9, "{stderr}");
    assert_eq!(
        stderr.matches("expected variable name").count(),
        7,
        "{stderr}"
    );
    assert_eq!(stderr.matches("expected expression").count(), 9, "{stderr}");
}

#[test]
fn check_command_reports_semantic_range_errors_and_body_errors_together() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-range-recovery-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun main() {\n    item := true\n    for item in missing_start..2 {\n        echo(missing_body)\n    }\n}\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0202]").count(), 1, "{stderr}");
    assert_eq!(stderr.matches("error[R0203]").count(), 2, "{stderr}");
    assert!(stderr.contains("`item` is already declared in this scope"));
    assert!(stderr.contains("unknown variable `missing_start`"));
    assert!(stderr.contains("unknown variable `missing_body`"));
    assert!(stderr.contains(":3:9"));
    assert!(stderr.contains(":3:17"));
    assert!(stderr.contains(":4:14"));
}

#[test]
fn check_command_reports_structure_field_syntax_errors_together() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-structure-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "struct Config { good: i32, broken: , next: str, : bool, last: f64 } fun main() {}",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 2, "{stderr}");
    assert!(stderr.contains("expected type name"));
    assert!(stderr.contains("expected field name"));
}

#[test]
fn check_command_reports_function_parameter_syntax_errors_together() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-parameter-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun broken(a i32, : bool, c: ) -> i32 { echo(1 + ) 1 } fun main() {}",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 3, "{stderr}");
    assert_eq!(stderr.matches("error[R0012]").count(), 1, "{stderr}");
    assert!(stderr.contains("expected `:` after parameter name"));
    assert!(stderr.contains("expected parameter name"));
    assert!(stderr.contains("expected type name"));
    assert!(stderr.contains("expected expression"));
}

#[test]
fn check_command_reports_call_and_structure_literal_errors_together() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-expression-list-syntax-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "struct Pair { a: i32, b: i32, c: i32 } fun combine(a: i32, b: i32, c: i32) {} fun main() { echo(combine(1 + , 2 + , 3)) echo(combine(1 2)) pair := Pair { a: 1 + , b: 2 + , c: } other := Pair { a: 1 b: 2 } }",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0010]").count(), 2, "{stderr}");
    assert_eq!(stderr.matches("error[R0012]").count(), 5, "{stderr}");
    assert_eq!(stderr.matches("expected expression").count(), 5, "{stderr}");
    assert_eq!(stderr.matches("expected `,`").count(), 2, "{stderr}");
}

#[test]
fn check_command_reports_multiple_independent_lexical_errors() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-multiple-lexical-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "@ fun main() { 💥 }").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0005]").count(), 2, "{stderr}");
    assert!(stderr.contains(":1:1"));
    assert!(stderr.contains(":1:16"));
}

#[test]
fn check_command_reports_semantic_errors_from_independent_functions() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-multiple-semantic-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun first() {\n    echo(missing_first)\n}\nfun second() {\n    echo(missing_second)\n}\nfun main() {}\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0203]").count(), 2, "{stderr}");
    assert!(stderr.contains("`missing_first`"));
    assert!(stderr.contains("`missing_second`"));
    assert!(stderr.contains(":2:10"));
    assert!(stderr.contains(":5:10"));
}

#[test]
fn check_command_reports_all_duplicate_global_declarations() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-duplicate-declarations-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "struct First { value: i32 }\nfun duplicate() {}\nstruct First { value: i32 }\nfun main() {}\nfun duplicate() {}\nstruct Second { value: i32 }\nstruct Second { value: i32 }\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0220]").count(), 2, "{stderr}");
    assert_eq!(stderr.matches("error[R0201]").count(), 1, "{stderr}");
    let first_structure = stderr
        .find("error[R0220]")
        .expect("first structure diagnostic");
    let function = stderr.find("error[R0201]").expect("function diagnostic");
    let second_structure = stderr
        .rfind("error[R0220]")
        .expect("second structure diagnostic");
    assert!(
        first_structure < function && function < second_structure,
        "{stderr}"
    );
}

#[test]
fn check_command_reports_independent_structure_declaration_errors() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-structure-declaration-errors-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "struct First { value: Missing, other: MissingToo }\nstruct Second { value: i32, value: MissingAgain }\nstruct Empty {}\nfun main() {}\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert_eq!(stderr.matches("error[R0230]").count(), 3, "{stderr}");
    assert_eq!(stderr.matches("error[R0222]").count(), 1, "{stderr}");
    assert_eq!(stderr.matches("error[R0221]").count(), 1, "{stderr}");
    let unknown_type = stderr
        .find("error[R0230]")
        .expect("unknown type diagnostic");
    let duplicate_field = stderr
        .find("error[R0222]")
        .expect("duplicate field diagnostic");
    let empty_structure = stderr
        .find("error[R0221]")
        .expect("empty structure diagnostic");
    assert!(
        unknown_type < duplicate_field && duplicate_field < empty_structure,
        "{stderr}"
    );
}

#[test]
fn check_command_still_accepts_a_valid_program_with_recovery_enabled() {
    let source_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/hello.ryn");
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");

    assert!(
        result.status.success(),
        "valid source failed `ryn check`: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains(&source_path.display().to_string()),
        "{stdout}"
    );
    assert!(stdout.trim_end().ends_with(": ok"), "{stdout}");
}

#[test]
fn check_command_explains_the_required_main_signature() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-main-signature-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main(value: i32) {}").expect("temporary source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains(
        "error[R0208]: `main` must take no parameters and return either no value or `i32`"
    ));
    assert!(stderr.contains("help: use `fun main() { ... }` or `fun main() -> i32 { ... }`"));
}

#[test]
fn check_command_highlights_the_incompatible_operator_operand() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-operator-type-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(
        &source_path,
        "fun main() {\n    left: i32 = 1\n    right: i64 = 2\n    echo(left + right)\n}\n",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains(
        "error[R0206]: arithmetic operands must have matching numeric types, found `i32` and `i64`"
    ));
    assert!(stderr.contains("4 |     echo(left + right)"));
    assert!(stderr.contains(&format!("  | {}^~~~~", " ".repeat(16))));
    assert!(stderr.contains(
        "help: use matching numeric types; Ryn does not implicitly convert numeric values"
    ));
}

#[test]
fn check_command_shows_actionable_help_for_immutable_assignment() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-immutable-{}-{unique}.ryn", std::process::id()));
    fs::write(
        &source_path,
        "fun main() {\n    value := 1\n    value = 2\n}",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0204]: `value` is immutable"));
    assert!(stderr.contains("help: declare `mut value := ...` if this binding must be mutable"));
}

#[test]
fn check_command_suggests_the_required_type_for_a_bad_argument() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-type-{}-{unique}.ryn", std::process::id()));
    fs::write(
        &source_path,
        "fun take(value: i32) {}\nfun main() {\n    take(\"wrong\")\n}",
    )
    .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0212]: argument to `take` has type `str` but `i32` is required")
    );
    assert!(stderr.contains("  |          ^~~~~~~"));
    assert!(stderr.contains("help: pass a `i32` value to `take`"));
}

#[test]
fn check_command_suggests_a_boolean_condition() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-condition-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, "fun main() { when 1 { echo(1) } }")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0207]: `when` condition must have type `bool`"));
    assert!(
        stderr.contains("help: use a boolean expression, such as a comparison, for the condition")
    );
}

#[test]
fn check_command_shows_the_valid_integer_literal_range() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-integer-range-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { value: i8 = 128 }")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0206]: integer literal is outside the `i8` range"));
    assert!(
        stderr.contains("help: use a value from `-128` to `127`, or choose a wider integer type")
    );
}

#[test]
fn check_command_suggests_a_close_variable_name() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-name-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, "fun main() { mut count := 1 echo(cout) }")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0203]: unknown variable `cout`"));
    assert!(stderr.contains("help: did you mean `count`?"));
}

#[test]
fn check_command_suggests_process_argument_builtins() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-builtin-name-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { arg_coun(0) }").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        stderr.contains("error[R0210]: unknown function `arg_coun`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("help: did you mean `arg_count`?"),
        "{stderr}"
    );
}

#[test]
fn check_command_suggests_a_close_variable_name_for_assignment_targets() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-assignment-name-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() { mut count := 1 cout += 2 }")
        .expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0203]: unknown variable `cout`"));
    assert!(stderr.contains("help: did you mean `count`?"));
}

#[test]
fn check_command_suggests_close_structure_field_names() {
    let cases = [
        (
            "struct Point { width: i32 height: i32 }\nfun main() { point := Point { widht: 10, height: 20 } }",
            "structure `Point` has no field `widht`",
            "did you mean `width`?",
        ),
        (
            "struct Point { width: i32 height: i32 }\nfun main() { point := Point { width: 10, height: 20 } echo(point.heigth) }",
            "structure `Point` has no field `heigth`",
            "did you mean `height`?",
        ),
        (
            "struct Point { width: i32 } struct Bounds { min: Point }\nfun main() { mut bounds := Bounds { min: Point { width: 0 } } bounds.min.widht = 1 }",
            "structure `Point` has no field `widht`",
            "did you mean `width`?",
        ),
    ];

    for (index, (source, expected_error, expected_help)) in cases.into_iter().enumerate() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos();
        let source_path = std::env::temp_dir().join(format!(
            "ryn-structure-field-{}-{unique}-{index}.ryn",
            std::process::id()
        ));
        fs::write(&source_path, source).expect("temporary Ryn source is written");

        let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
            .arg("check")
            .arg(&source_path)
            .output()
            .expect("ryn process starts");
        let _ = fs::remove_file(&source_path);

        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(
            stderr.contains(&format!("error[R0225]: {expected_error}")),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("help: {expected_help}")),
            "{stderr}"
        );
        if index == 2 {
            assert!(
                stderr.contains("^~~~"),
                "the diagnostic should highlight only the misspelled field: {stderr}"
            );
        }
    }
}

#[test]
fn check_command_reports_truncated_function_declarations_without_panicking() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path =
        std::env::temp_dir().join(format!("ryn-truncated-{}-{unique}.ryn", std::process::id()));
    fs::write(&source_path, "fun").expect("temporary Ryn source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0010]: expected function name"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn check_command_reports_excessive_nesting_without_panicking() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-excessive-nesting-{}-{unique}.ryn",
        std::process::id()
    ));
    let source = format!(
        "fun main() {{ echo({}true{}) }}",
        "(".repeat(129),
        ")".repeat(129),
    );
    fs::write(&source_path, source).expect("temporary source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0015]: maximum parser nesting depth exceeded in expression"));
    assert!(stderr.contains("help: reduce the amount of nesting in expressions"));
    assert!(!stderr.contains("panicked"));
}

#[test]
fn check_command_rejects_loop_control_outside_a_while_loop() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-loop-control-scope-{}-{unique}.ryn",
        std::process::id()
    ));

    for (keyword, code) in [("break", "R0016"), ("continue", "R0017")] {
        fs::write(&source_path, format!("fun main() {{ {keyword} }}"))
            .expect("temporary source is written");
        let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
            .arg("check")
            .arg(&source_path)
            .output()
            .expect("ryn process starts");
        let stderr = String::from_utf8_lossy(&result.stderr);

        assert!(!result.status.success());
        assert!(stderr.contains(&format!(
            "error[{code}]: `{keyword}` can only be used inside a loop"
        )));
        assert!(stderr.contains(&format!(
            "help: move `{keyword}` into the body of a `while` or `for` loop"
        )));
    }

    let _ = fs::remove_file(source_path);
}

#[test]
fn check_command_suggests_valid_expression_forms() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let source_path = std::env::temp_dir().join(format!(
        "ryn-missing-expression-{}-{unique}.ryn",
        std::process::id()
    ));
    fs::write(&source_path, "fun main() {\n    value :=\n}\n")
        .expect("temporary source is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source_path)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_file(&source_path);

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("error[R0012]: expected expression"));
    assert!(
        stderr.contains(
            "help: start with a literal, variable, function call, unary operator, or `(`."
        )
    );
}

#[test]
fn unknown_cli_command_shows_usage_without_reading_the_input_file() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let missing_path = std::env::temp_dir().join(format!(
        "ryn-unknown-command-{}-{unique}.ryn",
        std::process::id()
    ));

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("wat")
        .arg(&missing_path)
        .output()
        .expect("ryn process starts");

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("usage:\n  ryn new <path>"));
    assert!(stderr.contains("ryn check <file.ryn|project-dir>"));
    assert!(stderr.contains("ryn build <file.ryn|project-dir> [--release] [-o|--output <path>]"));
    assert!(stderr.contains("ryn run <file.ryn|project-dir> [--release] [-o|--output <path>]"));
    assert!(stderr.contains("ryn clean <project-dir>"));
    assert!(!stderr.contains("error[R0001]"));
}

#[test]
fn help_option_lists_commands_and_exits_successfully() {
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("--help")
        .output()
        .expect("ryn process starts");

    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(result.status.success());
    assert!(stdout.contains("new <path>"));
    assert!(stdout.contains("check <source>"));
    assert!(stdout.contains("build <source>"));
    assert!(stdout.contains("run <source>"));
    assert!(stdout.contains("clean <project>"));
    assert!(stdout.contains("src/main.ryn"));
    assert!(stdout.contains("i32 result from main becomes the process exit code for run"));
    assert!(stdout.contains("-o, --output <path>"));
    assert!(stdout.contains("-V, --version"));
    assert!(result.stderr.is_empty());
}

#[test]
fn output_option_is_rejected_for_check_and_when_missing_its_path() {
    let directory = std::env::temp_dir().join(format!(
        "ryn-output-option-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos()
    ));
    fs::create_dir(&directory).expect("temporary directory is created");
    let source = directory.join("fixture.ryn");
    fs::write(&source, "fun main() {}").expect("temporary source is written");

    let check = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source)
        .arg("-o")
        .arg(directory.join("ignored"))
        .output()
        .expect("ryn process starts");
    let missing_path = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&source)
        .arg("--output")
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(&directory);

    for result in [check, missing_path] {
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(stderr.contains("ryn new <path>"), "{stderr}");
        assert!(
            stderr.contains("ryn check <file.ryn|project-dir>"),
            "{stderr}"
        );
        assert!(
            stderr.contains("ryn build <file.ryn|project-dir> [--release] [-o|--output <path>]"),
            "{stderr}"
        );
        assert!(
            stderr.contains("ryn run <file.ryn|project-dir> [--release] [-o|--output <path>]"),
            "{stderr}"
        );
        assert!(stderr.contains("ryn clean <project-dir>"), "{stderr}");
    }
}

#[test]
fn version_option_reports_the_package_version() {
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("--version")
        .output()
        .expect("ryn process starts");

    assert!(result.status.success());
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        format!("ryn {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(result.stderr.is_empty());
}
