//! Run-time checks for `?.` optional field access. `value?.field` reads the field
//! of an `Option` payload and yields `Option` of the field type. Each program runs
//! on both frontends and its exact output is compared.

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
    let directory = temp_dir("optional-field");
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
fn optional_field_chains_through_options_and_option_payloads() {
    // A present payload yields its field, `None` stays `None` at any step of the
    // chain, and a String field is cloned out of the payload. `Option::Some(3)` is
    // typed from its payload when no expected type is given.
    for frontend in [None, Some("rust")] {
        let result = run_fixture("optional_field.ryn", frontend);
        assert!(
            result.status.success(),
            "optional field access should run with frontend {frontend:?}, stderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "4\n-1\n8\nbox\nnone\n3\ntext\n",
            "output with frontend {frontend:?}"
        );
    }
}
