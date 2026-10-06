use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

static SELF_HOSTED_PROCESS_LOCK: Mutex<()> = Mutex::new(());

fn run_frontend(source_text: &str, label: &str) -> (String, bool) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "ryn-frontend-{label}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("temporary directory is created");
    let source = directory.join("sample.ryn");
    fs::write(&source, source_text).expect("sample source is written");
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/self_hosted_project");
    let _guard = SELF_HOSTED_PROCESS_LOCK
        .lock()
        .expect("self-hosted project invocations are serialized");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&project)
        .arg("--")
        .arg(&source)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.success(),
    )
}

fn run_type_checker(source_text: &str, label: &str) -> (String, bool) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "ryn-typecheck-{label}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("temporary directory is created");
    let source = directory.join("sample.ryn");
    fs::write(&source, source_text).expect("sample source is written");
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/self_hosted_project");
    let _guard = SELF_HOSTED_PROCESS_LOCK
        .lock()
        .expect("self-hosted project invocations are serialized");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&project)
        .arg("--")
        .arg("--check")
        .arg(&source)
        .output()
        .expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.success(),
    )
}

#[test]
fn self_hosted_frontend_accepts_the_samples_the_compiler_accepts() {
    let samples = [
        "fun main() {\n    echo \"Hello, Ryn!\"\n}\n",
        "fun add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfun main() {\n    echo add(20, 22)\n}\n",
        "fun main() {\n    mut count := 0\n    while count < 3 {\n        count += 1\n    }\n    echo count\n}\n",
        "fun main() {\n    for index in 0..3 {\n        echo index\n    }\n}\n",
        "fun main() {\n    when 1 < 2 {\n        echo \"yes\"\n    } else {\n        echo \"no\"\n    }\n}\n",
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 1, y: 2 }\n    echo point.x\n}\n",
    ];
    for sample in samples {
        let (stdout, success) = run_frontend(sample, "valid");
        assert!(
            success,
            "the self-hosted frontend rejected a valid sample: {sample}\n{stdout}"
        );
        assert_eq!(stdout.trim(), "ok", "valid sample output: {stdout}");
    }
}

#[test]
fn self_hosted_frontend_rejects_the_samples_the_compiler_rejects() {
    let samples = [
        "fun main() {\n    x := \n}\n",
        "fun main() {\n    echo 1\n",
        "fun main() {\n    when {\n        echo 1\n    }\n}\n",
        "fun 5() {}\n",
        "fun main() {\n    return\n    }\n    echo 1\n}\n",
    ];
    for sample in samples {
        let (stdout, success) = run_frontend(sample, "invalid");
        assert!(
            !success,
            "the self-hosted frontend accepted an invalid sample: {stdout}"
        );
        assert!(
            stdout.contains("error"),
            "expected a diagnostic for an invalid sample: {stdout}"
        );
    }
}

#[test]
fn self_hosted_type_checker_accepts_typed_calls_and_reassignments() {
    let source = "fun add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfun announce() {\n    echo true\n}\n\nfun has_items(items: Vec<u64>) -> bool {\n    true\n}\n\nfun main() {\n    mut count := add(1, 2)\n    count = count + 1\n    announce()\n    when count > 0 {\n        echo count\n    }\n}\n";
    let (stdout, success) = run_type_checker(source, "valid");
    assert!(success, "type checker rejected a valid sample: {stdout}");
    assert_eq!(stdout.trim(), "ok (types)");
}

#[test]
fn self_hosted_type_checker_reports_type_errors_without_crashing() {
    let source = "fun main() {\n    mut count := 1\n    count = \"wrong type\"\n}\n";
    let (stdout, success) = run_type_checker(source, "invalid");
    assert!(
        !success,
        "type checker accepted an invalid assignment: {stdout}"
    );
    assert!(
        stdout.contains("type check failed"),
        "expected a type diagnostic: {stdout}"
    );
}

#[test]
fn self_hosted_type_checker_rejects_assignment_to_immutable_locals() {
    let source = "fun main() {\n    count := 1\n    count = 2\n}\n";
    let (stdout, success) = run_type_checker(source, "immutable");
    assert!(
        !success,
        "type checker accepted assignment to an immutable local: {stdout}"
    );
    assert!(
        stdout.contains("type check failed"),
        "expected a type diagnostic: {stdout}"
    );
}
