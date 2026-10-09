//! Run-time checks for range values such as `span := 5..20`. A range value is a//! structure with `start`, `end` and `inclusive` fields, and `for` loops over it
//! without a literal header. Each program runs on both frontends and its exact
//! output is compared.

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
    let directory = temp_dir("range-values");
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
fn range_values_hold_bounds_and_iterate_without_overflow() {
    // Bounds and fields are readable, a range value can be looped twice, an empty
    // range runs no iterations, and `..=` reaches the maximum of `u8` without wrapping.
    for frontend in [None, Some("rust")] {
        let result = run_fixture("range_values.ryn", frontend);
        assert!(
            result.status.success(),
            "range values should run with frontend {frontend:?}, stderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "5\n20\n180\n195\n1\n2\n3\n250\n251\n252\n253\n254\n255\n6\n0\n1\n2\n",
            "output with frontend {frontend:?}"
        );
    }
}
