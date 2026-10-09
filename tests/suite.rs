//! The 250-test language suite.
//!
//! Every file under `tests/suite/<category>/` is one test. A header of `//`
//! comment lines at the top of the file states what the compiler must do:
//!
//! ```text
//! // test: run            compile, execute and compare observable behaviour
//! // out: first line      one line of expected stdout (repeat per line)
//! // err: text            stderr must contain this text (repeatable)
//! // exit: 3              expected process exit status (default 0)
//! // test: fail           `ryn check` must reject the program
//! // code: R0203          the diagnostic code that must be reported
//! // msg: text            the diagnostic must contain this text (repeatable)
//! ```
//!
//! Positive tests run the produced native executable; negative tests are
//! counted as passing only when the compiler exits non-zero with the expected
//! diagnostic and without crashing.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

const EXPECTED_TOTAL: usize = 571;

#[derive(Debug)]
struct Case {
    category: String,
    path: PathBuf,
    run: bool,
    out: Vec<String>,
    err: Vec<String>,
    exit: i32,
    code: Option<String>,
    msg: Vec<String>,
}

fn parse_case(category: &str, path: &Path) -> Case {
    // A project directory keeps its expectations in `src/main.ryn`.
    let header = if path.is_dir() {
        path.join("src/main.ryn")
    } else {
        path.to_path_buf()
    };
    let text = fs::read_to_string(&header).expect("suite file is readable");
    let mut case = Case {
        category: category.into(),
        path: path.to_path_buf(),
        run: true,
        out: Vec::new(),
        err: Vec::new(),
        exit: 0,
        code: None,
        msg: Vec::new(),
    };
    let mut kind_seen = false;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("//") else {
            break;
        };
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let Some((key, value)) = rest.split_once(':') else {
            continue;
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match key {
            "test" => {
                kind_seen = true;
                case.run = match value.trim() {
                    "run" => true,
                    "fail" => false,
                    other => panic!("{}: unknown test kind `{other}`", path.display()),
                };
            }
            "out" => case.out.push(value.to_string()),
            "err" => case.err.push(value.to_string()),
            "exit" => case.exit = value.trim().parse().expect("exit status is a number"),
            "code" => case.code = Some(value.trim().to_string()),
            "msg" => case.msg.push(value.to_string()),
            _ => {}
        }
    }
    assert!(kind_seen, "{} has no `// test:` header", path.display());
    case
}

fn collect_cases() -> Vec<Case> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suite");
    let mut cases = Vec::new();
    let mut categories: Vec<_> = fs::read_dir(&root)
        .expect("tests/suite exists")
        .map(|entry| entry.expect("category entry").path())
        .filter(|path| path.is_dir())
        .collect();
    categories.sort();
    for category in categories {
        let name = category
            .file_name()
            .expect("category has a name")
            .to_string_lossy()
            .into_owned();
        let mut files: Vec<_> = fs::read_dir(&category)
            .expect("category is readable")
            .map(|entry| entry.expect("file entry").path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "ryn")
                    || path.join("src/main.ryn").is_file()
            })
            .collect();
        files.sort();
        for file in files {
            cases.push(parse_case(&name, &file));
        }
    }
    cases
}

fn execute(case: &Case, scratch: &Path) -> Result<(), String> {
    let compiler = env!("CARGO_BIN_EXE_ryn");
    let stem = case
        .path
        .file_stem()
        .or_else(|| case.path.file_name())
        .expect("case has a name")
        .to_string_lossy();
    if !case.run {
        let result = Command::new(compiler)
            .arg("check")
            .arg(&case.path)
            .output()
            .map_err(|error| format!("cannot start the compiler: {error}"))?;
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        if result.status.success() {
            return Err("the compiler accepted an invalid program".into());
        }
        if result.status.code().is_none()
            || stderr.contains("panicked")
            || stderr.contains("internal error")
        {
            return Err(format!("the compiler crashed:\n{stderr}"));
        }
        if let Some(code) = &case.code
            && !stderr.contains(&format!("[{code}]"))
        {
            return Err(format!("expected diagnostic {code}, got:\n{stderr}"));
        }
        for message in &case.msg {
            if !stderr.contains(message.as_str()) {
                return Err(format!("diagnostic lacks `{message}`:\n{stderr}"));
            }
        }
        return Ok(());
    }

    let output = scratch.join(format!("{}_{stem}", case.category));
    let build = Command::new(compiler)
        .arg("build")
        .arg(&case.path)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|error| format!("cannot start the compiler: {error}"))?;
    if !build.status.success() {
        return Err(format!(
            "compilation failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }
    let run = Command::new(&output)
        .output()
        .map_err(|error| format!("cannot start the program: {error}"))?;
    let _ = fs::remove_file(&output);
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    let mut expected = case.out.join("\n");
    if !case.out.is_empty() {
        expected.push('\n');
    }
    if stdout != expected {
        return Err(format!(
            "stdout differs\n--- expected\n{expected}--- actual\n{stdout}"
        ));
    }
    if run.status.code() != Some(case.exit) {
        return Err(format!(
            "exit status {:?}, expected {}\nstderr: {stderr}",
            run.status.code(),
            case.exit
        ));
    }
    for text in &case.err {
        if !stderr.contains(text.as_str()) {
            return Err(format!("stderr lacks `{text}`:\n{stderr}"));
        }
    }
    Ok(())
}

#[test]
fn language_suite() {
    let cases = collect_cases();
    let scratch = std::env::temp_dir().join(format!("ryn-suite-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("scratch directory");

    let queue = Arc::new(Mutex::new((0..cases.len()).collect::<Vec<_>>()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let cases = Arc::new(cases);
    let workers = thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let results = Arc::clone(&results);
            let cases = Arc::clone(&cases);
            let scratch = scratch.clone();
            thread::spawn(move || {
                loop {
                    let Some(index) = queue.lock().expect("queue").pop() else {
                        break;
                    };
                    let outcome = execute(&cases[index], &scratch);
                    results.lock().expect("results").push((index, outcome));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("worker finishes");
    }
    let _ = fs::remove_dir_all(&scratch);

    let mut results = results
        .lock()
        .expect("results")
        .drain(..)
        .collect::<Vec<_>>();
    results.sort_by_key(|(index, _)| *index);
    let mut table: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    for (index, outcome) in &results {
        let entry = table.entry(cases[*index].category.clone()).or_default();
        entry.1 += 1;
        match outcome {
            Ok(()) => entry.0 += 1,
            Err(reason) => failures.push(format!("{}: {reason}", cases[*index].path.display())),
        }
    }
    let mut passed_total = 0;
    for (category, (passed, total)) in &table {
        println!("{category:<28} {passed:>3}/{total:<3}");
        passed_total += passed;
    }
    println!("{:<28} {passed_total:>3}/{}", "TOTAL", cases.len());
    assert!(
        failures.is_empty(),
        "{} suite test(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    if std::env::var_os("RYN_SUITE_PARTIAL").is_none() {
        assert_eq!(
            cases.len(),
            EXPECTED_TOTAL,
            "the suite must contain exactly {EXPECTED_TOTAL} tests"
        );
    }
}
