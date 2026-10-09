//! The Ryn semantic analysis (`selfhost/src/middle`) must produce the same IR text as the
//! bootstrap `sema::analyze` for the compiler's own Ryn projects: whole programs of several
//! modules, including the semantic analysis itself.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ryn::{ast_codec, frontend::Frontend, ir_codec, sema};

fn build_tool() -> PathBuf {
    ryn::frontend::locate_or_build_self_hosted().expect("the self-hosted frontend builds")
}

fn analyze_with_ryn(tool: &Path, syntax_tree: &str, name: &str) -> Result<String, String> {
    let input = std::env::temp_dir().join(format!(
        "ryn-sema-project-{}-{name}.ast",
        std::process::id()
    ));
    let output =
        std::env::temp_dir().join(format!("ryn-sema-project-{}-{name}.ir", std::process::id()));
    fs::write(&input, syntax_tree).expect("syntax tree text is written");
    let status = Command::new(tool)
        .arg("--sema")
        .arg(&input)
        .arg(&output)
        .output()
        .expect("semantic analysis tool starts");
    let _ = fs::remove_file(&input);
    let result = if status.status.success() {
        Ok(fs::read_to_string(&output).expect("IR text is written"))
    } else {
        Err(String::from_utf8_lossy(&status.stdout).trim().to_owned())
    };
    let _ = fs::remove_file(&output);
    result
}

#[test]
fn ryn_semantic_analysis_matches_the_bootstrap_for_the_compilers_own_projects() {
    let tool = build_tool();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for project in ["selfhost/ir", "selfhost"] {
        let program = ryn::load_project_program(root.join(project), &Frontend::Bootstrap)
            .unwrap_or_else(|error| panic!("{project} should load: {error}"));
        let syntax_tree = ast_codec::encode_program(&program);
        let expected = ir_codec::encode_ir(
            &sema::analyze(program)
                .unwrap_or_else(|error| panic!("{project} should analyze: {error:?}")),
        );
        let actual = analyze_with_ryn(&tool, &syntax_tree, &project.replace('/', "-"))
            .unwrap_or_else(|reason| panic!("the Ryn analysis declined {project}: {reason}"));
        assert!(
            actual == expected,
            "the Ryn analysis must produce the same IR as the bootstrap for {project}"
        );
    }
}
