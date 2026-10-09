//! The IR checker written in Ryn (`selfhost/ir`) must accept every IR text that the
//! Rust encoder writes, and must reject texts that were cut short or corrupted.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{ir_codec, parser, sema};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Builds the Ryn-written checker with the compiler under test and returns its executable.
fn build_checker() -> PathBuf {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("selfhost/ir");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&project)
        .arg("--release")
        .output()
        .expect("ryn process starts");
    assert!(
        output.status.success(),
        "the Ryn IR checker should build, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    project
        .join("build")
        .join("release")
        .join(if cfg!(windows) { "ir.exe" } else { "ir" })
}

/// Runs the checker on `text`; returns whether it accepted the text.
fn accepts(checker: &Path, text: &str) -> bool {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("ryn-ir-check-{}-{id}.ir", std::process::id()));
    fs::write(&path, text).expect("IR text is written");
    let output = Command::new(checker)
        .arg(&path)
        .output()
        .expect("checker starts");
    let _ = fs::remove_file(&path);
    output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "ok"
}

fn analysed_ir_texts() -> Vec<String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut texts = Vec::new();
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
            if let Ok(ir) = sema::analyze(program) {
                texts.push(ir_codec::encode_ir(&ir));
            }
        }
    }
    texts
}

#[test]
fn ryn_ir_checker_accepts_every_encoded_program_and_rejects_damaged_text() {
    let checker = build_checker();
    let texts = analysed_ir_texts();
    assert!(
        texts.len() >= 90,
        "expected the pass fixtures and examples to analyse, got {}",
        texts.len()
    );
    for text in &texts {
        assert!(
            accepts(&checker, text),
            "the checker should accept this IR text"
        );
    }

    let sample = &texts[0];
    let truncated = &sample[..sample.len() - 6];
    assert!(
        !accepts(&checker, truncated),
        "truncated IR must be rejected"
    );
    assert!(
        !accepts(&checker, "Ir 0 - 1 0 0 0"),
        "short IR must be rejected"
    );
    let corrupted = sample.replacen(" Return ", " Retrn ", 1);
    assert_ne!(&corrupted, sample, "the corruption must change the text");
    assert!(
        !accepts(&checker, &corrupted),
        "unknown statement tags must be rejected"
    );
}
