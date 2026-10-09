//! The syntax tree data model written in Ryn (`selfhost/src/middle/ast.ryn`) must decode every
//! successful syntax tree text the Rust encoder writes and encode it back to the same bytes,
//! and must reject texts that were cut short or corrupted.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{ast_codec, parser};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Builds the Ryn-written round-trip tool with the compiler under test and returns its executable.
fn build_tool() -> PathBuf {
    ryn::frontend::locate_or_build_self_hosted().expect("the self-hosted frontend builds")
}

fn scratch_path(extension: &str) -> PathBuf {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "ryn-ast-roundtrip-{}-{id}.{extension}",
        std::process::id()
    ))
}

/// Runs the tool on `text`; returns the re-encoded text when the tool accepts it.
fn round_trip(tool: &Path, text: &str) -> Option<String> {
    let input = scratch_path("ast");
    let output = scratch_path("out");
    fs::write(&input, text).expect("syntax tree text is written");
    let status = Command::new(tool)
        .arg("--ast-roundtrip")
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

fn parsed_program_texts() -> Vec<String> {
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
            if let Ok(program) = parser::parse(&source) {
                texts.push(ast_codec::encode_program(&program));
            }
        }
    }
    texts
}

#[test]
fn ryn_syntax_tree_model_round_trips_every_encoded_program_and_rejects_damaged_text() {
    let tool = build_tool();
    let texts = parsed_program_texts();
    assert!(
        texts.len() >= 90,
        "expected the pass fixtures and examples to parse, got {}",
        texts.len()
    );
    for text in &texts {
        let encoded =
            round_trip(&tool, text).expect("the Ryn model should decode this syntax tree text");
        assert_eq!(
            &encoded, text,
            "re-encoding must reproduce the text exactly"
        );
    }

    let sample = texts
        .iter()
        .find(|text| text.contains(" Template "))
        .expect("some sample prints a template");
    let truncated = &sample[..sample.len() - 6];
    assert!(
        round_trip(&tool, truncated).is_none(),
        "truncated syntax tree text must be rejected"
    );
    let corrupted = sample.replacen(" Template ", " Templte ", 1);
    assert_ne!(&corrupted, sample, "the corruption must change the text");
    assert!(
        round_trip(&tool, &corrupted).is_none(),
        "unknown statement tags must be rejected"
    );
}
