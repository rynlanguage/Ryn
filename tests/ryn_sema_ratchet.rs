//! The Ryn semantic analysis is measured against the Rust analysis on every invalid program of the corpus. The outcome of
//! each program is recorded in `tests/fixtures/ryn_sema_coverage.tsv` (classes: EXACT, WRONG_TEXT, WRONG_CODE, ACCEPT,
//! DECLINE, PARSE_FAIL). Any outcome that differs from the recording fails this test, so both a regression and an
//! improvement must be written into the fixture, by regenerating it with `tools/ryn-diagnostic-coverage.sh`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ryn::frontend;

struct Recorded {
    class: String,
    path: String,
    code: String,
    message: String,
}

fn recorded_outcomes() -> Vec<Recorded> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(root.join("tests/fixtures/ryn_sema_coverage.tsv"))
        .expect("the coverage fixture is readable");
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.splitn(4, '\t');
            let mut next = || fields.next().unwrap_or_default().to_owned();
            Recorded {
                class: next(),
                path: next(),
                code: next(),
                message: next(),
            }
        })
        .collect()
}

/// The class of a diagnostic the Ryn analysis reports, against the Rust diagnostic recorded for the program.
fn compare(code: &str, message: &str, recorded: &Recorded) -> String {
    if code == recorded.code && message.starts_with(&recorded.message) {
        "EXACT".to_owned()
    } else if code == recorded.code {
        "WRONG_TEXT".to_owned()
    } else {
        "WRONG_CODE".to_owned()
    }
}

/// A diagnostic the parser wrote into the syntax tree: ` Error <len>:CODE <len>:MESSAGE <start> <end> + ...`, where each
/// string is prefixed by its length in bytes.
fn parser_error(tree: &str) -> Option<(String, String)> {
    let rest = tree.strip_prefix(" Error ")?;
    let (count, rest) = rest.split_once(':')?;
    let code_length: usize = count.parse().ok()?;
    let code = rest.get(..code_length)?.to_owned();
    let rest = rest.get(code_length..)?.trim_start_matches(' ');
    let (count, rest) = rest.split_once(':')?;
    let message_length: usize = count.parse().ok()?;
    let message = rest.get(..message_length)?.to_owned();
    Some((code, message))
}

/// The outcome of a diagnostic the Ryn tools write as `diagnostic CODE START END LINE:message help`.
fn diagnostic_outcome(text: &str, recorded: &Recorded) -> String {
    let Some(diagnostic) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("diagnostic "))
    else {
        return "DECLINE".to_owned();
    };
    // The message follows the start and end offsets and the line number.
    let mut parts = diagnostic.splitn(4, ' ');
    let code = parts.next().unwrap_or_default();
    let _start = parts.next();
    let _end = parts.next();
    let message = parts
        .next()
        .and_then(|located| located.split_once(':'))
        .map(|(_, message)| message)
        .unwrap_or_default();
    compare(code, message, recorded)
}

/// Runs one Ryn tool of the frontend, writing its output to `output`, and returns that output's first line.
fn tool_output(frontend: &Path, tool: Option<&str>, tree: &Path, output: &Path) -> (bool, String) {
    let _ = fs::remove_file(output);
    let mut command = Command::new(frontend);
    if let Some(tool) = tool {
        command.arg(tool);
    }
    let status = command
        .arg(tree)
        .arg(output)
        .output()
        .expect("the frontend starts")
        .status;
    let text = fs::read_to_string(output).unwrap_or_default();
    (
        status.success(),
        text.lines().next().unwrap_or_default().to_owned(),
    )
}

/// The outcome of one program in the Ryn analysis, in the compiler's order: the syntax tree is parsed, the constant checks
/// (`--comptime`) run, and then the analysis (`--sema`).
fn outcome(frontend: &Path, source: &Path, scratch: &Path, recorded: &Recorded) -> String {
    let syntax_tree = scratch.join("program.ast");
    let output = scratch.join("program.out");
    let parsed = Command::new(frontend)
        .arg(source)
        .arg(&syntax_tree)
        .output()
        .expect("the frontend starts");
    if !parsed.status.success() {
        return "PARSE_FAIL".to_owned();
    }
    let tree = fs::read_to_string(&syntax_tree).unwrap_or_default();
    if let Some((code, message)) = parser_error(&tree) {
        return compare(&code, &message, recorded);
    }
    let (_, constants) = tool_output(frontend, Some("--comptime"), &syntax_tree, &output);
    if constants.starts_with("diagnostic ") {
        let text = fs::read_to_string(&output).unwrap_or_default();
        return diagnostic_outcome(&text, recorded);
    }
    let (analyzed, first) = tool_output(frontend, Some("--sema"), &syntax_tree, &output);
    if first.starts_with("diagnostic ") {
        let text = fs::read_to_string(&output).unwrap_or_default();
        return diagnostic_outcome(&text, recorded);
    }
    if analyzed {
        "ACCEPT".to_owned()
    } else {
        "DECLINE".to_owned()
    }
}

#[test]
fn ryn_analysis_outcomes_match_the_recorded_coverage() {
    let frontend =
        frontend::locate_or_build_self_hosted().expect("the self-hosted frontend builds");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::env::temp_dir().join(format!("ryn-coverage-ratchet-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("scratch directory is created");
    let mut changed = Vec::new();
    let records = recorded_outcomes();
    assert!(!records.is_empty(), "the coverage fixture lists the corpus");
    for recorded in &records {
        let class = outcome(&frontend, &root.join(&recorded.path), &scratch, recorded);
        if class != recorded.class {
            changed.push(format!(
                "{}: recorded {}, now {}",
                recorded.path, recorded.class, class
            ));
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    assert!(
        changed.is_empty(),
        "the Ryn analysis outcomes differ from tests/fixtures/ryn_sema_coverage.tsv; regenerate it with \
         tools/ryn-diagnostic-coverage.sh once the change is intended:\n{}",
        changed.join("\n")
    );
}
