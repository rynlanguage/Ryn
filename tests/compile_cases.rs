use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run_check(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(path)
        .output()
        .expect("ryn process starts")
}

#[test]
fn examples_and_pass_fixtures_pass_semantic_checking() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let directories = [root.join("examples"), root.join("tests/programs/pass")];
    let mut sources = Vec::new();
    for directory in directories {
        sources.extend(
            fs::read_dir(&directory)
                .expect("pass-program directory exists")
                .map(|entry| entry.expect("program directory entry is readable").path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "ryn")),
        );
    }
    sources.sort();
    assert!(
        !sources.is_empty(),
        "the pass-program suite must not be empty"
    );

    for source in sources {
        let result = run_check(&source);
        assert!(
            result.status.success(),
            "{} should pass `ryn check`, stderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn fail_fixtures_are_rejected_with_the_expected_diagnostic_codes() {
    let cases = [
        ("unknown_variable.ryn", "R0203"),
        ("operator_type_mismatch.ryn", "R0206"),
        ("malformed_interpolation.ryn", "R0014"),
        ("mut_ref_immutable.ryn", "R0206"),
        ("write_through_shared_ref.ryn", "R0206"),
        ("return_local_reference.ryn", "R0248"),
        ("return_aliased_local_reference.ryn", "R0248"),
        ("ffi_unsupported_record.ryn", "R0247"),
        ("array_repeat_string.ryn", "R0256"),
        ("array_repeat_length.ryn", "R0240"),
        ("array_repeat_bad_length.ryn", "R0012"),
        ("pipe_bad_target.ryn", "R0265"),
        ("coalesce_not_option.ryn", "R0234"),
        ("defer_return_inside.ryn", "R0267"),
        ("defer_break_outside.ryn", "R0268"),
        ("defer_propagate.ryn", "R0266"),
        ("defer_reference.ryn", "R0269"),
        ("defer_owned_moved.ryn", "R0240"),
    ];
    let failures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/programs/fail");

    for (filename, expected_code) in cases {
        let source = failures.join(filename);
        let result = run_check(&source);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            !result.status.success(),
            "{} should fail `ryn check`",
            source.display()
        );
        assert!(
            stderr.contains(&format!("error[{expected_code}]:")),
            "{} should report {expected_code}, stderr:\n{stderr}",
            source.display()
        );
    }
}
