//! The subset of semantic analysis written in Ryn (`selfhost/sema_subset`) must produce the
//! same IR text as the bootstrap `sema::analyze` for every program it accepts, and must
//! decline the rest with `unsupported`, never with a wrong program.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{ast_codec, ir_codec, parser, sema};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Exit status of the tool when a program lies outside the subset.
const UNSUPPORTED: i32 = 3;

fn build_tool() -> PathBuf {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("selfhost/sema_subset");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&project)
        .arg("--release")
        .output()
        .expect("ryn process starts");
    assert!(
        output.status.success(),
        "the Ryn semantic subset should build, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    project
        .join("build")
        .join("release")
        .join(if cfg!(windows) {
            "sema_subset.exe"
        } else {
            "sema_subset"
        })
}

fn scratch_path(extension: &str) -> PathBuf {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "ryn-sema-subset-{}-{id}.{extension}",
        std::process::id()
    ))
}

/// Runs the tool on a syntax tree text: `Ok(ir)` when it lowers the program, `Err(status)` otherwise.
fn lower(tool: &Path, syntax_tree: &str) -> Result<String, i32> {
    let input = scratch_path("ast");
    let output = scratch_path("ir");
    fs::write(&input, syntax_tree).expect("syntax tree text is written");
    let status = Command::new(tool)
        .arg(&input)
        .arg(&output)
        .output()
        .expect("subset tool starts");
    let _ = fs::remove_file(&input);
    let result = if status.status.success() {
        Ok(fs::read_to_string(&output).expect("IR text is written"))
    } else {
        Err(status.status.code().unwrap_or(-1))
    };
    let _ = fs::remove_file(&output);
    result
}

#[test]
fn ryn_sema_subset_matches_the_bootstrap_ir_for_every_program_it_accepts() {
    let tool = build_tool();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut accepted = 0;
    for directory in ["tests/programs/pass", "examples"] {
        let mut paths: Vec<PathBuf> = fs::read_dir(root.join(directory))
            .expect("program directory exists")
            .map(|entry| entry.expect("directory entry is readable").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "ryn"))
            .collect();
        paths.sort();
        for path in paths {
            let source = fs::read_to_string(&path).expect("program is readable");
            let Ok(program) = parser::parse(&source) else {
                continue;
            };
            let syntax_tree = ast_codec::encode_program(&program);
            let Ok(expected) = sema::analyze(program).map(|ir| ir_codec::encode_ir(&ir)) else {
                continue;
            };
            match lower(&tool, &syntax_tree) {
                Ok(ir) => {
                    assert_eq!(
                        ir,
                        expected,
                        "the Ryn subset must produce the same IR as sema for {}",
                        path.display()
                    );
                    accepted += 1;
                }
                Err(status) => assert_eq!(
                    status,
                    UNSUPPORTED,
                    "programs outside the subset must be declined with status {UNSUPPORTED}: {}",
                    path.display()
                ),
            }
        }
    }
    assert!(
        accepted >= 22,
        "expected the subset to accept at least twenty-two corpus programs, got {accepted}"
    );
}
