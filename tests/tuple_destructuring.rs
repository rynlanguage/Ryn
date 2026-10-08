//! Run-time checks for `(a, b) := value` tuple destructuring. The declaration is
//! expanded by both parsers into a temporary plus one `:=` per name, so each
//! program runs on both frontends and its exact output is compared.

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

fn run_fixture(name: &str, frontend: Option<&str>) -> std::process::Output {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/programs/pass")
        .join(name);
    let directory = temp_dir("tuple-destructuring");
    let copy = directory.join("main.ryn");
    fs::copy(&source, &copy).expect("fixture is copied");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryn"));
    command.arg("run").arg(&copy);
    if let Some(frontend) = frontend {
        command.arg("--frontend").arg(frontend);
    }
    let result = command.output().expect("ryn process starts");
    let _ = fs::remove_dir_all(directory);
    result
}

#[test]
fn tuple_destructuring_binds_names_and_skips_underscores() {
    // Covers a tuple local, a call result, `mut` bindings, `_` placeholders, and a
    // declaration inside a nested block after an expression statement.
    for frontend in [None, Some("rust")] {
        let result = run_fixture("tuple_destructuring.ryn", frontend);
        assert!(
            result.status.success(),
            "tuple destructuring should run with frontend {frontend:?}, stderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "3\n3\nx\n11\nr\n4\nhalf\n4\n",
            "output with frontend {frontend:?}"
        );
    }
}
