//! Writes the syntax tree text of the pass fixtures and examples to a directory, for the
//! Ryn-written syntax tree tool. It does nothing unless `RYN_AST_OUTPUT_DIR` is set.

use std::{fs, path::PathBuf};

use ryn::{ast_codec, parser};

#[test]
fn dump_syntax_tree_text_for_the_ryn_tool() {
    let Ok(output) = std::env::var("RYN_AST_OUTPUT_DIR") else {
        return;
    };
    let output = PathBuf::from(output);
    fs::create_dir_all(&output).expect("output directory is created");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for directory in ["examples", "tests/programs/pass"] {
        for entry in fs::read_dir(root.join(directory)).expect("program directory exists") {
            let path = entry.expect("directory entry is readable").path();
            if path.extension().is_none_or(|extension| extension != "ryn") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("program is readable");
            let Ok(program) = parser::parse(&source) else {
                continue;
            };
            let name = format!(
                "{}_{}.ast",
                directory.replace('/', "_"),
                path.file_stem()
                    .expect("program has a name")
                    .to_string_lossy()
            );
            fs::write(output.join(name), ast_codec::encode_program(&program))
                .expect("syntax tree text is written");
        }
    }
}
