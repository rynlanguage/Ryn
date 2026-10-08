//! Run-time checks for `Vec` with enum elements. Each program is compiled and
//! executed, and its exact output is compared, so the element callbacks that
//! clone, drop, and move enum handles are exercised for real.

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
    let directory = temp_dir("vec-enum");
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

fn assert_output(name: &str, expected: &str) {
    for frontend in [None, Some("rust")] {
        let result = run_fixture(name, frontend);
        assert!(
            result.status.success(),
            "{name} should run with frontend {frontend:?}, stderr:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            expected,
            "{name} output with frontend {frontend:?}"
        );
    }
}

#[test]
fn vec_of_enum_clones_sets_and_pops_elements() {
    // Element reads, `set`, `clone`, and `pop` each copy or move the enum handle;
    // the snapshot must stay independent of the original vector.
    assert_output("vec_of_enum.ryn", "3\n4\ntag\n9\n9\ndot\ndot\n2\n");
}

#[test]
fn vec_of_enum_first_last_get_loop_take_and_insert() {
    assert_output("vec_of_enum_ops.ryn", "2\ndot\nnone\n2\ndot\n2\n2\nx\n");
}

#[test]
fn vec_of_enum_survives_repeated_clone_and_drop_cycles() {
    assert_output("vec_of_enum_drops.ryn", "200000\n4000\n");
}

#[test]
fn vec_of_option_holds_optional_values_and_nested_options() {
    assert_output("vec_of_option.ryn", "3\n8\n8\n");
}
