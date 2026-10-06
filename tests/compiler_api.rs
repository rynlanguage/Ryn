use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn library_compiles_and_runs_native_code_without_the_cli_process() {
    let source = ryn::source::SourceFile::new(
        "library_api.ryn",
        "fun add(left: i32, right: i32) -> i32 { left + right } fun main() { echo(add(20, 22)) }",
    );
    let tokens = ryn::lexer::lex(source.text()).expect("source lexes through the library API");
    assert!(!tokens.is_empty());

    let program = ryn::parser::parse(source.text()).expect("source parses through the library API");
    let ir = ryn::sema::analyze(program).expect("source type-checks through the library API");
    assert_eq!(ir.functions.len(), 2);
    let checked_ir = ryn::check_source(&source).expect("high-level source-file check succeeds");
    assert_eq!(checked_ir.functions.len(), 2);

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let mut output =
        std::env::temp_dir().join(format!("ryn-library-api-{}-{unique}", std::process::id()));
    if cfg!(windows) {
        output.set_extension("exe");
    }
    let object =
        ryn::codegen::emit_object(&checked_ir).expect("library API emits a Cranelift object");
    ryn::codegen::link_object(&object, &output)
        .expect("library API links the object into a native executable");

    let execution = Command::new(&output)
        .output()
        .expect("native output from the library API starts");
    let stdout = String::from_utf8_lossy(&execution.stdout).into_owned();
    let _ = fs::remove_file(output);
    assert!(execution.status.success());
    assert_eq!(stdout, "42\n");
}

#[test]
fn recovering_check_api_reports_lexer_and_parser_errors_for_text_and_files() {
    let lexical_errors =
        ryn::check_recovering("@ @").expect_err("both unexpected characters should be returned");
    assert_eq!(
        lexical_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0005", "R0005"]
    );

    let source_text = "fun first() { mut := 1 }\nfun second( { }\nfun main() {}";
    let parser_errors = ryn::check_recovering(source_text)
        .expect_err("both malformed declarations should be reported");
    assert_eq!(
        parser_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0010", "R0010"]
    );

    let source = ryn::source::SourceFile::new("recovering_api.ryn", source_text);
    let file_errors = ryn::check_source_recovering(&source)
        .expect_err("source-file API should preserve parser recovery");
    assert_eq!(file_errors.len(), 2);

    let semantic_source = "fun first() { echo(missing_first) echo(missing_again) }\nfun second() { when true { branch := 1 echo(branch_missing) echo(branch_missing_again) } else { echo(other_branch_missing) } echo(missing_after_branch) }\nfun third() -> i32 { unavailable := missing_initializer unavailable }\nfun fourth() -> i32 { echo(missing_in_body) 1.0 }\nfun fifth() { while true { echo(loop_missing) } break }\nfun main() {}";
    let semantic_errors = ryn::check_recovering(semantic_source)
        .expect_err("independent statement errors should be returned without cascading");
    assert_eq!(
        semantic_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        [
            "R0203", "R0203", "R0203", "R0203", "R0203", "R0203", "R0203", "R0203", "R0205",
            "R0203", "R0016"
        ]
    );
    assert!(
        semantic_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(semantic_source)
            .expect_err("the compatibility check API still returns one error")
            .code,
        "R0203"
    );
    let semantic_source = ryn::source::SourceFile::new("recovering_sema_api.ryn", semantic_source);
    assert_eq!(
        ryn::check_source_recovering(&semantic_source)
            .expect_err("source-file API should preserve semantic recovery")
            .len(),
        11
    );

    let interpolation_source = "fun main() { echo(\"first={missing_first}, second={missing_second}, third={missing_third}\") }";
    let interpolation_errors = ryn::check_recovering(interpolation_source)
        .expect_err("all independent missing interpolation names should be reported");
    assert_eq!(
        interpolation_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203", "R0203"]
    );
    assert!(
        interpolation_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(interpolation_source)
            .expect_err("the compatibility API still returns the first interpolation error")
            .code,
        "R0203"
    );

    let binary_source = "fun main() { echo(missing_first + missing_second + missing_third) }";
    let binary_errors = ryn::check_recovering(binary_source)
        .expect_err("each independent binary operand error should be returned");
    assert_eq!(
        binary_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203", "R0203"]
    );
    assert!(
        binary_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(binary_source)
            .expect_err("the compatibility API still returns the first binary operand error")
            .code,
        "R0203"
    );

    let condition_source = "fun main() { when missing_if_condition { echo(missing_then) } else { echo(missing_else) } while missing_while_condition { echo(missing_loop) } }";
    let condition_errors = ryn::check_recovering(condition_source)
        .expect_err("invalid conditions should not hide independent branch or loop-body errors");
    assert_eq!(
        condition_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203", "R0203", "R0203", "R0203"]
    );
    assert!(
        condition_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(condition_source)
            .expect_err("the compatibility API still returns the invalid if condition first")
            .code,
        "R0203"
    );

    let if_expression_source =
        "fun main() { echo(when missing_condition { missing_then } else { missing_else }) }";
    let if_expression_errors = ryn::check_recovering(if_expression_source)
        .expect_err("an invalid if expression should still check both value branches");
    assert_eq!(
        if_expression_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203", "R0203"]
    );
    assert!(
        if_expression_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(if_expression_source)
            .expect_err("the compatibility API still returns the condition error first")
            .code,
        "R0203"
    );

    let struct_literal_source = "struct Pair { first: i32, second: bool } fun main() { echo(Pair { first: missing_first, second: \"wrong\", absent: missing_unknown, first: missing_duplicate }) }";
    let struct_literal_errors = ryn::check_recovering(struct_literal_source)
        .expect_err("structure literals should report independent field initializer errors");
    assert_eq!(
        struct_literal_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0205", "R0225", "R0203", "R0228", "R0203"]
    );
    assert!(
        struct_literal_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(struct_literal_source)
            .expect_err("the compatibility API still returns the first field value error")
            .code,
        "R0203"
    );

    let call_source = "fun take(first: i32, second: i32, third: i32) {} fun result(first: i32, second: i32) -> i32 { first + second } fun main() { take(missing_first, \"wrong type\", missing_second) echo(result(missing_third, missing_fourth)) }";
    let call_errors = ryn::check_recovering(call_source)
        .expect_err("independent argument errors should be reported for both call forms");
    assert_eq!(
        call_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0212", "R0203", "R0203", "R0203"]
    );
    assert!(
        call_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(call_source)
            .expect_err("the compatibility API still returns the first call argument error")
            .code,
        "R0203"
    );

    let wrong_arity_source =
        "fun take(expected: i32) {} fun main() { take(missing_expected, missing_extra) }";
    let wrong_arity_errors = ryn::check_recovering(wrong_arity_source)
        .expect_err("known calls should report independent argument errors despite wrong arity");
    assert_eq!(
        wrong_arity_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0211", "R0203", "R0203"]
    );
    assert!(
        wrong_arity_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(wrong_arity_source)
            .expect_err("the compatibility API still returns the arity error first")
            .code,
        "R0211"
    );

    let unknown_call_source =
        "fun main() { echo(missing_function(missing_first, missing_second)) }";
    let unknown_call_errors = ryn::check_recovering(unknown_call_source)
        .expect_err("unknown calls should not hide independent argument expression errors");
    assert_eq!(
        unknown_call_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0210", "R0203", "R0203"]
    );
    assert!(
        unknown_call_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(unknown_call_source)
            .expect_err("the compatibility API still returns the unknown function first")
            .code,
        "R0210"
    );

    let assignment_source = "struct Box { value: i32 } fun main() { missing_target = missing_value fixed := 1 fixed = missing_immutable_value missing_compound += missing_compound_value missing_object.value = missing_field_rhs immutable := Box { value: 1 } immutable.value = missing_immutable_field_rhs mut container := Box { value: 0 } container.absent = missing_unknown_field_rhs }";
    let assignment_errors = ryn::check_recovering(assignment_source)
        .expect_err("invalid assignment targets should not hide independent right-hand errors");
    assert_eq!(
        assignment_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        [
            "R0203", "R0203", "R0204", "R0203", "R0203", "R0203", "R0203", "R0203", "R0204",
            "R0203", "R0225", "R0203"
        ]
    );
    assert!(
        assignment_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(assignment_source)
            .expect_err("the compatibility API still returns the first invalid assignment target")
            .code,
        "R0203"
    );

    let let_initializer_source = "fun main() { value := 1 value := missing_duplicate_initializer value: bool = 2 typed: MissingType = missing_typed_initializer echo(hidden_after_failed_let) }";
    let let_initializer_errors = ryn::check_recovering(let_initializer_source)
        .expect_err("failed let declarations should still check their initializers");
    assert_eq!(
        let_initializer_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0202", "R0203", "R0202", "R0205", "R0230", "R0203"]
    );
    assert!(
        let_initializer_errors[2].span.start < let_initializer_errors[3].span.start,
        "the duplicate declaration should be reported before its initializer type error"
    );
    assert!(
        let_initializer_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(let_initializer_source)
            .expect_err("the compatibility API still returns the duplicate declaration first")
            .code,
        "R0202"
    );

    let return_source = "fun explicit_return() { return missing_return_value } fun tail_return() { missing_tail_value } fun typed_tail() -> i32 { missing_tail_left + missing_tail_right } fun main() {}";
    let return_errors = ryn::check_recovering(return_source)
        .expect_err("return expressions should be checked when their functions lack result types");
    assert_eq!(
        return_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0013", "R0203", "R0214", "R0203", "R0203", "R0203"]
    );
    assert!(
        return_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(return_source)
            .expect_err("the compatibility API still returns the missing return type first")
            .code,
        "R0013"
    );

    let duplicate_parameters = "fun first(value: i32, value: i32, value: i32) { echo(hidden) } fun second() { echo(missing) } fun main() {}";
    let parameter_errors = ryn::check_recovering(duplicate_parameters)
        .expect_err("all duplicate parameters and other function errors should be returned");
    assert_eq!(
        parameter_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0202", "R0202", "R0203"]
    );
    assert!(
        parameter_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(duplicate_parameters)
            .expect_err("the compatibility API still stops at its first parameter error")
            .code,
        "R0202"
    );

    let global_error =
        ryn::check_recovering("fun duplicate() {} fun duplicate() {} fun main() { echo(missing) }")
            .expect_err("a global declaration error blocks dependent body analysis");
    assert_eq!(global_error.len(), 1);
    assert_eq!(global_error[0].code, "R0201");

    let duplicate_declarations = r#"
struct First { value: i32 }
fun duplicate() {}
struct First { value: i32 }
fun main() { echo(missing) }
fun duplicate() {}
struct Second { value: i32 }
struct Second { value: i32 }
"#;
    let duplicate_errors = ryn::check_recovering(duplicate_declarations)
        .expect_err("all duplicate global declarations should be reported");
    assert_eq!(
        duplicate_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0220", "R0201", "R0220"]
    );
    assert!(
        duplicate_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(duplicate_declarations)
            .expect_err("the compatibility API still stops at the first global error")
            .code,
        "R0220"
    );

    let invalid_signatures = "fun first(value: Missing, other: MissingToo) -> ResultMissing { 1 } fun second(value: NotFound) {} fun main(value: MainMissing) { echo(nope) }";
    let signature_errors = ryn::check_recovering(invalid_signatures)
        .expect_err("independent function signature errors should be reported together");
    assert_eq!(
        signature_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0230", "R0230", "R0230", "R0230", "R0208", "R0230"]
    );
    assert!(
        signature_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(invalid_signatures)
            .expect_err("the compatibility API still returns its first signature error")
            .code,
        "R0230"
    );

    let invalid_structs = "struct First { value: Missing, other: MissingToo } struct Second { value: i32, value: MissingAgain } struct Empty {} fun main(value: i32) { echo(nope) }";
    let struct_errors = ryn::check_recovering(invalid_structs)
        .expect_err("independent structure declaration errors should be reported together");
    assert_eq!(
        struct_errors
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0230", "R0230", "R0222", "R0230", "R0221"]
    );
    assert!(
        struct_errors
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert_eq!(
        ryn::check(invalid_structs)
            .expect_err("the compatibility API still returns its first structure error")
            .code,
        "R0230"
    );
}

#[test]
fn recovering_check_reports_independent_by_value_structure_cycles() {
    let source = "struct A { b: B } struct B { a: A } struct C { c: C } fun main() {}";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("each independent recursive layout cycle should be reported");

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0232", "R0232"]
    );
    assert!(
        diagnostics
            .windows(2)
            .all(|pair| pair[0].span.start < pair[1].span.start)
    );
    assert!(diagnostics[0].message.contains("`A`"));
    assert!(diagnostics[1].message.contains("`C`"));
    assert_eq!(
        ryn::check(source)
            .expect_err("the compatibility checker should retain fail-fast behavior")
            .code,
        "R0232"
    );
}

#[test]
fn recovering_for_range_checks_end_and_body_after_an_invalid_start() {
    let source = "fun main() { for item in missing_start..2 { echo(missing_body) } }";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("an invalid start should not hide independent loop-body errors");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203"]
    );
    assert!(diagnostics[0].message.contains("missing_start"));
    assert!(diagnostics[1].message.contains("missing_body"));

    let source = "fun main() { for item in true..2 { echo(missing_body) } }";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("a non-integer start should not hide loop-body errors");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0206", "R0203"]
    );
    assert!(diagnostics[0].message.contains("found `bool`"));
    assert!(diagnostics[1].message.contains("missing_body"));

    let source = "fun main() { for item in missing_start..missing_end { echo(missing_body) } }";
    let diagnostics =
        ryn::check_recovering(source).expect_err("both range-bound errors should be reported");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0203", "R0203"]
    );
    assert!(diagnostics[0].message.contains("missing_start"));
    assert!(diagnostics[1].message.contains("missing_end"));
}

#[test]
fn recovering_for_range_checks_bounds_and_body_after_a_duplicate_name() {
    let source = "fun main() { item := true for item in 0..2 { echo(missing_body) } }";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("a duplicate loop variable should not hide loop-body errors");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0202", "R0203"]
    );
    assert!(diagnostics[0].message.contains("already declared"));
    assert!(diagnostics[1].message.contains("missing_body"));

    let source = "fun main() { item := true for item in 0..missing_end { echo(missing_body) } }";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("duplicate names should not hide end-bound or body errors");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0202", "R0203", "R0203"]
    );
    assert!(diagnostics[0].message.contains("already declared"));
    assert!(diagnostics[1].message.contains("missing_end"));
    assert!(diagnostics[2].message.contains("missing_body"));

    let source = "fun main() { item := true for item in missing_start..2 { echo(missing_body) } }";
    let diagnostics = ryn::check_recovering(source)
        .expect_err("duplicate names should not hide independent range errors");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        ["R0202", "R0203", "R0203"]
    );
    assert!(diagnostics[0].message.contains("already declared"));
    assert!(diagnostics[1].message.contains("missing_start"));
    assert!(diagnostics[2].message.contains("missing_body"));
}

#[test]
fn for_ranges_require_matching_integer_bounds_and_keep_the_binding_local() {
    let non_integer = ryn::check("fun main() { for item in true..2 {} }")
        .expect_err("range bounds must be integer values");
    assert_eq!(non_integer.code, "R0206");
    assert!(non_integer.message.contains("bounds must be integers"));

    let mismatched = ryn::check("fun main() { for item in 0..2.0 {} }")
        .expect_err("range bounds must have the same integer type");
    assert_eq!(mismatched.code, "R0206");
    assert!(mismatched.message.contains("same integer type"));

    let immutable = ryn::check("fun main() { for item in 0..2 { item = 1 } }")
        .expect_err("the range loop variable is immutable");
    assert_eq!(immutable.code, "R0204");

    let out_of_scope = ryn::check("fun main() { for item in 0..2 {} echo(item) }")
        .expect_err("the range loop variable does not escape the loop");
    assert_eq!(out_of_scope.code, "R0203");
}

#[test]
fn compile_api_builds_and_runs_native_code_from_source_text() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let mut output =
        std::env::temp_dir().join(format!("ryn-compile-api-{}-{unique}", std::process::id()));
    if cfg!(windows) {
        output.set_extension("exe");
    }

    ryn::compile("fun main() { echo(6 * 7) }", &output)
        .expect("high-level API compiles source to a native executable");
    let execution = Command::new(&output)
        .output()
        .expect("compiled program starts");
    let stdout = String::from_utf8_lossy(&execution.stdout).into_owned();
    let _ = fs::remove_file(output);

    assert!(execution.status.success());
    assert_eq!(stdout, "42\n");
}

#[test]
fn source_file_compile_api_builds_and_runs_native_code() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let mut output = std::env::temp_dir().join(format!(
        "ryn-source-compile-api-{}-{unique}",
        std::process::id()
    ));
    if cfg!(windows) {
        output.set_extension("exe");
    }
    let source = ryn::source::SourceFile::new("source_api.ryn", "fun main() { echo(40 + 2) }");

    ryn::compile_source(&source, &output)
        .expect("high-level source-file API compiles to a native executable");
    let execution = Command::new(&output)
        .output()
        .expect("compiled program starts");
    let stdout = String::from_utf8_lossy(&execution.stdout).into_owned();
    let _ = fs::remove_file(output);

    assert!(execution.status.success());
    assert_eq!(stdout, "42\n");
}

#[test]
fn source_file_compile_api_rejects_overwriting_its_source() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "ryn-source-output-collision-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("temporary directory is created");
    let source_path = directory.join("fixture.ryn");
    let original = "fun main() { echo(42) }";
    fs::write(&source_path, original).expect("source fixture is written");
    let source = ryn::source::SourceFile::load(&source_path).expect("source file loads");

    let error = ryn::compile_source(&source, &source_path)
        .expect_err("compiler must preserve source when output matches its path");
    assert!(matches!(
        error,
        ryn::CompileError::OutputWouldOverwriteSource { .. }
    ));

    let alias = directory.join(".").join("fixture.ryn");
    let alias_error = ryn::compile_source(&source, alias)
        .expect_err("compiler must also reject a path alias of the source");
    assert!(matches!(
        alias_error,
        ryn::CompileError::OutputWouldOverwriteSource { .. }
    ));

    let hard_link = directory.join("fixture-hard-link.ryn");
    fs::hard_link(&source_path, &hard_link).expect("source hard link is created");
    let hard_link_error = ryn::compile_source(&source, &hard_link)
        .expect_err("compiler must reject a hard link to the source");
    assert!(matches!(
        hard_link_error,
        ryn::CompileError::OutputWouldOverwriteSource { .. }
    ));

    #[cfg(unix)]
    {
        let symbolic_link = directory.join("fixture-symbolic-link.ryn");
        std::os::unix::fs::symlink(&source_path, &symbolic_link)
            .expect("source symbolic link is created");
        let symbolic_link_error = ryn::compile_source(&source, &symbolic_link)
            .expect_err("compiler must reject a symbolic link to the source");
        assert!(matches!(
            symbolic_link_error,
            ryn::CompileError::OutputWouldOverwriteSource { .. }
        ));
    }

    assert_eq!(
        fs::read_to_string(&source_path).expect("source remains readable"),
        original
    );
    fs::remove_dir_all(directory).expect("temporary directory is removed");
}

#[test]
fn source_file_check_api_keeps_diagnostic_render_context() {
    let source = ryn::source::SourceFile::new("src/main.ryn", "fun main() { echo(missing) }");
    let error = ryn::check_source(&source).expect_err("unknown variable must fail checking");

    let rendered = error.render(&source);
    assert!(rendered.contains("src/main.ryn:1:19"));
    assert!(rendered.contains("unknown variable `missing`"));
}

#[test]
fn compile_error_renderer_includes_source_context_for_source_errors() {
    let source = ryn::source::SourceFile::new("src/main.ryn", "fun main() {\n    echo(missing)\n}");
    let error = ryn::compile_source(&source, "unused-output")
        .expect_err("unknown variable must fail compilation");

    let rendered = error.render(&source);
    assert!(rendered.contains("src/main.ryn:2:10"));
    assert!(rendered.contains("2 |     echo(missing)"));
    assert!(rendered.contains("|          ^~~~~~~"));
}

#[test]
fn compile_api_preserves_source_diagnostics_as_typed_errors() {
    let error = ryn::compile("fun main() { echo(missing) }", "unused-output")
        .expect_err("invalid source must fail before native code generation");
    match error {
        ryn::CompileError::Source(diagnostic) => {
            assert_eq!(diagnostic.code, "R0203");
            assert!(
                diagnostic
                    .to_string()
                    .contains("unknown variable `missing`")
            );
        }
        ryn::CompileError::Native(error) => {
            panic!("source diagnostics should not be classified as build errors: {error}");
        }
        ryn::CompileError::OutputWouldOverwriteSource { .. } => {
            panic!("text-based compile has no source path to collide with output");
        }
    }
}

#[test]
fn compile_api_classifies_native_setup_failures_separately() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let blocker = std::env::temp_dir().join(format!(
        "ryn-compile-api-blocker-{}-{unique}",
        std::process::id()
    ));
    fs::write(&blocker, "file blocks the output directory")
        .expect("temporary blocking file is written");
    let output = blocker.join("program.exe");

    let error = ryn::compile("fun main() {}", &output)
        .expect_err("native output setup must fail when its parent is a file");
    let _ = fs::remove_file(&blocker);

    assert!(matches!(&error, ryn::CompileError::Native(_)));
    assert!(std::error::Error::source(&error).is_some());
}
