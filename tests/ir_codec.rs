//! The textual IR must survive a round trip: analysing a program, encoding its IR,
//! decoding that text and encoding the result again gives the same text.

use std::{fs, path::PathBuf};

use ryn::{ir_codec, parser, sema};

fn program_paths(directory: &str) -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(directory);
    let mut paths: Vec<PathBuf> = fs::read_dir(&root)
        .expect("program directory exists")
        .map(|entry| entry.expect("directory entry is readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ryn"))
        .collect();
    paths.sort();
    paths
}

/// Round-trips one program's IR. Returns false when the program does not analyse.
fn round_trip(path: &PathBuf) -> bool {
    let source = fs::read_to_string(path).expect("program is readable");
    let Ok(program) = parser::parse(&source) else {
        return false;
    };
    let Ok(ir) = sema::analyze(program) else {
        return false;
    };
    let first = ir_codec::encode_ir(&ir);
    let decoded = ir_codec::decode_ir(&first)
        .unwrap_or_else(|error| panic!("{} should decode: {error}", path.display()));
    let second = ir_codec::encode_ir(&decoded);
    assert_eq!(
        first,
        second,
        "{} changes when its IR is decoded and encoded again",
        path.display()
    );
    true
}

#[test]
fn every_pass_fixture_round_trips_its_ir() {
    let paths = program_paths("tests/programs/pass");
    assert!(
        !paths.is_empty(),
        "the pass-program suite must not be empty"
    );
    for path in &paths {
        assert!(
            round_trip(path),
            "{} should analyse so its IR can be checked",
            path.display()
        );
    }
}

#[test]
fn examples_round_trip_their_ir_where_they_analyse() {
    let paths = program_paths("examples");
    let checked = paths.iter().filter(|path| round_trip(path)).count();
    assert!(
        checked * 2 >= paths.len(),
        "only {checked} of {} examples analysed",
        paths.len()
    );
}
