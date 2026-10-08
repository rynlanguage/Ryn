//! The IR data model written in Ryn (`selfhost/ir_roundtrip`) must decode every IR text the
//! Rust encoder writes and encode it back to the same bytes, and must reject texts that were
//! cut short or corrupted.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{ir_codec, parser, sema};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Builds the Ryn-written round-trip tool with the compiler under test and returns its executable.
fn build_tool() -> PathBuf {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("selfhost/ir_roundtrip");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(&project)
        .arg("--release")
        .output()
        .expect("ryn process starts");
    assert!(
        output.status.success(),
        "the Ryn IR round-trip tool should build, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    project
        .join("build")
        .join("release")
        .join(if cfg!(windows) {
            "ir_roundtrip.exe"
        } else {
            "ir_roundtrip"
        })
}

fn scratch_path(extension: &str) -> PathBuf {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "ryn-ir-roundtrip-{}-{id}.{extension}",
        std::process::id()
    ))
}

/// Runs the tool on `text`; returns the re-encoded text when the tool accepts it.
fn round_trip(tool: &Path, text: &str) -> Option<String> {
    let input = scratch_path("ir");
    let output = scratch_path("out");
    fs::write(&input, text).expect("IR text is written");
    let status = Command::new(tool)
        .arg(&input)
        .arg(&output)
        .output()
        .expect("round-trip tool starts");
    let _ = fs::remove_file(&input);
    let encoded = if status.status.success() {
        fs::read_to_string(&output).ok()
    } else {
        None
    };
    let _ = fs::remove_file(&output);
    encoded
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
fn ryn_ir_model_round_trips_every_encoded_program_and_rejects_damaged_text() {
    let tool = build_tool();
    let texts = analysed_ir_texts();
    assert!(
        texts.len() >= 90,
        "expected the pass fixtures and examples to analyse, got {}",
        texts.len()
    );
    for text in &texts {
        let encoded = round_trip(&tool, text).expect("the Ryn model should decode this IR text");
        assert_eq!(&encoded, text, "re-encoding must reproduce the IR text exactly");
    }

    let sample = texts
        .iter()
        .find(|text| text.contains(" Return "))
        .expect("some sample has a return statement");
    let truncated = &sample[..sample.len() - 6];
    assert!(
        round_trip(&tool, truncated).is_none(),
        "truncated IR must be rejected"
    );
    assert!(
        round_trip(&tool, "Ir 0 - 1 0 0 0").is_none(),
        "short IR must be rejected"
    );
    let corrupted = sample.replacen(" Return ", " Retrn ", 1);
    assert_ne!(&corrupted, sample, "the corruption must change the text");
    assert!(
        round_trip(&tool, &corrupted).is_none(),
        "unknown statement tags must be rejected"
    );
}
