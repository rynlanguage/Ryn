use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

pub mod ast;
pub mod ast_codec;
pub mod codegen;
pub mod filesystem_ops;
pub mod frontend;
pub mod generics;
pub mod ir_codec;
mod guard;
pub mod lexer;
pub mod lockfile;
pub mod manifest;
pub mod map_ops;
mod modules;
pub mod parser;
pub mod pe_linker;
pub mod registry;
pub mod sema;
pub mod source;
pub mod string_ops;
pub mod system_ops;
pub mod vector_ops;

/// Checks Ryn source and returns its typed intermediate representation.
///
/// This runs lexing, parsing, and semantic analysis without invoking the CLI.
/// Use [`check_recovering`] to gather recoverable lexer and parser errors.
///
/// # Example
///
/// ```
/// let ir = ryn::check("fun main() { echo 42 }").expect("source is valid");
/// assert_eq!(ir.functions.len(), 1);
/// ```
pub fn check(source: &str) -> Result<sema::RynIr, source::Diagnostic> {
    sema::analyze(parser::parse(source)?)
}

/// Checks Ryn source and gathers recoverable lexer and parser diagnostics.
///
/// Recoverable lexical errors are collected before parsing. When lexing
/// succeeds, independent parser errors are collected between structure
/// fields, function parameters, arguments, structure literal fields, and
/// statements, inside nested blocks, and across top-level declarations.
/// It still checks control-flow bodies after malformed conditions or range
/// headers.
/// Semantic analysis reports duplicate structure and function declarations
/// together in source order.
/// With unique top-level names, it reports errors for all invalid structure
/// fields and function parameter/result types, along with an invalid `main`
/// signature. When signatures are valid, it reports all duplicate parameter
/// names and skips each affected function body. In other bodies, it collects
/// errors across independent statements and nested blocks, including each
/// missing name in an echo interpolation and errors in arguments to known or
/// unknown calls, including known calls with the wrong arity. It also gathers
/// errors from both operands of a binary expression and independent field
/// initializers in structure literals. It checks an assignment's right-hand
/// expression even when its target is invalid, and checks a local initializer
/// even when its declaration is invalid. It also checks return expressions
/// when the function has no declared result type. An invalid `when` or `while`
/// condition does not prevent checking its branches or loop body. If a `for`
/// start bound is invalid, it still checks the end bound and, when that bound
/// is an integer, checks the loop body using its type. A duplicate loop
/// variable name also does not prevent checking the range bounds and body. A
/// failed new local declaration stops later statements in that block to avoid
/// cascading unknown-name errors. Tail return validation can still report
/// independent errors unless recovery stopped at a failed function-level local declaration.
/// Other global semantic errors stop analysis at the first issue.
pub fn check_recovering(source: &str) -> Result<sema::RynIr, Vec<source::Diagnostic>> {
    let program = parser::parse_recovering(source)?;
    sema::analyze_recovering(program)
}

/// Loads and checks a project rooted at a directory containing `src/main.ryn`.
/// Module imports use paths relative to `src/`, such as `use compiler::lexer`.
pub fn check_project(project: impl AsRef<Path>) -> Result<sema::RynIr, String> {
    check_project_with_frontend(project, &frontend::Frontend::Bootstrap)
}

/// Loads and checks a project, parsing its modules with `frontend`.
pub fn check_project_with_frontend(
    project: impl AsRef<Path>,
    frontend: &frontend::Frontend,
) -> Result<sema::RynIr, String> {
    modules::check_project(project.as_ref(), frontend).map(|(ir, _)| ir)
}

/// Checks a source file, parsing it with `frontend`.
///
/// The bootstrap parser gathers independent syntax errors; the self-hosted
/// frontend reports the first one.
pub fn check_source_with_frontend(
    source: &source::SourceFile,
    frontend: &frontend::Frontend,
) -> Result<sema::RynIr, Vec<source::Diagnostic>> {
    let program = frontend.parse_recovering(source.text())?;
    let program = frontend
        .monomorphize(program)
        .map_err(|error| vec![error])?;
    sema::analyze_recovering(program)
}

/// Compiles a project and its imported Ryn modules into a native executable.
pub fn compile_project_with_optimize(
    project: impl AsRef<Path>,
    output: impl AsRef<Path>,
    optimize: manifest::Optimize,
) -> Result<(), String> {
    compile_project_with_frontend(project, output, optimize, &frontend::Frontend::Bootstrap)
}

/// Compiles a project into a native executable, parsing its modules with `frontend`.
pub fn compile_project_with_frontend(
    project: impl AsRef<Path>,
    output: impl AsRef<Path>,
    optimize: manifest::Optimize,
    frontend: &frontend::Frontend,
) -> Result<(), String> {
    let output = output.as_ref();
    let (ir, source_paths) = modules::check_project(project.as_ref(), frontend)?;
    for source in source_paths {
        if paths_refer_to_same_file(&source, output) {
            return Err(format!(
                "error[R0303]: output {} would overwrite module source {}",
                output.display(),
                source.display()
            ));
        }
    }
    codegen::build_native_with_optimize(&ir, output, optimize.codegen_value())
        .map_err(|error| error.to_string())
}

/// Checks a standalone source file together with the modules it imports:
/// `std::...` and `.ryn` files in the same directory.
pub fn check_file_with_imports(
    file: impl AsRef<Path>,
    frontend: &frontend::Frontend,
) -> Result<sema::RynIr, String> {
    modules::check_file(file.as_ref(), frontend).map(|(ir, _)| ir)
}

/// Compiles a standalone source file together with the modules it imports.
pub fn compile_file_with_imports(
    file: impl AsRef<Path>,
    output: impl AsRef<Path>,
    optimize: manifest::Optimize,
    frontend: &frontend::Frontend,
) -> Result<(), String> {
    let output = output.as_ref();
    let (ir, source_paths) = modules::check_file(file.as_ref(), frontend)?;
    for source in source_paths {
        if paths_refer_to_same_file(&source, output) {
            return Err(format!(
                "error[R0303]: output {} would overwrite module source {}",
                output.display(),
                source.display()
            ));
        }
    }
    codegen::build_native_with_optimize(&ir, output, optimize.codegen_value())
        .map_err(|error| error.to_string())
}

/// Checks a [`source::SourceFile`] and returns its typed intermediate representation.
///
/// Keep the source file alongside the result so diagnostics can be rendered with
/// [`source::Diagnostic::render`] using its path and source text.
pub fn check_source(source: &source::SourceFile) -> Result<sema::RynIr, source::Diagnostic> {
    check(source.text())
}

/// Checks a source file and gathers recoverable lexer and parser diagnostics.
///
/// Keep the source file alongside the errors so diagnostics can be rendered
/// using [`source::Diagnostic::render`]. See [`check_recovering`] for details
/// about which stages collect multiple errors.
pub fn check_source_recovering(
    source: &source::SourceFile,
) -> Result<sema::RynIr, Vec<source::Diagnostic>> {
    check_recovering(source.text())
}

/// Errors returned while compiling source code to a native executable.
#[derive(Debug)]
pub enum CompileError {
    Source(source::Diagnostic),
    Native(codegen::BuildError),
    /// The output resolves to the source file, including a filesystem alias.
    OutputWouldOverwriteSource {
        source: PathBuf,
        output: PathBuf,
    },
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::Native(error) => error.fmt(formatter),
            Self::OutputWouldOverwriteSource { source, output } => write!(
                formatter,
                "error[R0303]: output {} would overwrite the source file {}",
                output.display(),
                source.display()
            ),
        }
    }
}

impl Error for CompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Native(error) => Some(error),
            Self::OutputWouldOverwriteSource { .. } => None,
        }
    }
}

impl CompileError {
    /// Renders source errors against their source text and other errors as plain messages.
    pub fn render(&self, source: &source::SourceFile) -> String {
        match self {
            Self::Source(error) => error.render(source),
            Self::Native(_) | Self::OutputWouldOverwriteSource { .. } => self.to_string(),
        }
    }
}

/// Compiles Ryn source through semantic analysis and Cranelift into a native executable.
///
/// Source diagnostics and native build failures remain distinguishable through
/// [`CompileError`]. Use [`check`] when only the typed intermediate representation is needed.
/// If the source came from a file, prefer [`compile_source`] so the output cannot overwrite it.
pub fn compile(source: &str, output: impl AsRef<Path>) -> Result<(), CompileError> {
    let ir = check(source).map_err(CompileError::Source)?;
    codegen::build_native(&ir, output.as_ref()).map_err(CompileError::Native)
}

/// Compiles a [`source::SourceFile`] through semantic analysis and Cranelift
/// into a native executable.
pub fn compile_source(
    source: &source::SourceFile,
    output: impl AsRef<Path>,
) -> Result<(), CompileError> {
    compile_source_with_optimize(source, output, manifest::Optimize::Speed)
}

/// Compiles source using the project's selected Cranelift optimization level.
pub fn compile_source_with_optimize(
    source: &source::SourceFile,
    output: impl AsRef<Path>,
    optimize: manifest::Optimize,
) -> Result<(), CompileError> {
    compile_source_with_frontend(source, output, optimize, &frontend::Frontend::Bootstrap)
}

/// Compiles a source file into a native executable, parsing it with `frontend`.
pub fn compile_source_with_frontend(
    source: &source::SourceFile,
    output: impl AsRef<Path>,
    optimize: manifest::Optimize,
    frontend: &frontend::Frontend,
) -> Result<(), CompileError> {
    let output = output.as_ref();
    if paths_refer_to_same_file(source.path(), output) {
        return Err(CompileError::OutputWouldOverwriteSource {
            source: source.path().to_path_buf(),
            output: output.to_path_buf(),
        });
    }
    let program = frontend
        .parse(source.text())
        .map_err(CompileError::Source)?;
    let program = frontend
        .monomorphize(program)
        .map_err(CompileError::Source)?;
    let ir = sema::analyze(program).map_err(CompileError::Source)?;
    codegen::build_native_with_optimize(&ir, output, optimize.codegen_value())
        .map_err(CompileError::Native)
}

fn paths_refer_to_same_file(input: &Path, output: &Path) -> bool {
    if same_file::is_same_file(input, output).unwrap_or(false) {
        return true;
    }
    let Some(input) = fs::canonicalize(input).ok() else {
        return false;
    };
    let output = if output.exists() {
        fs::canonicalize(output).ok()
    } else {
        let parent = output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = output.file_name();
        match (fs::canonicalize(parent).ok(), name) {
            (Some(parent), Some(name)) => Some(parent.join(name)),
            _ => None,
        }
    };
    output.is_some_and(|output| same_path(&output, &input))
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
