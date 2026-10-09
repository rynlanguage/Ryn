//! Frontend selection: the bootstrap parser written in Rust, or the
//! self-hosted frontend written in Ryn (`selfhost/`).
//!
//! The self-hosted frontend is an ordinary Ryn program. It lexes and parses a
//! source file and writes the syntax tree in the [`crate::ast_codec`] format;
//! the compiler decodes it and continues with semantic analysis, Ryn Guard,
//! and native code generation. Its sources are embedded in the compiler, so
//! `ryn` can build the frontend with itself on first use and cache it.

use std::{
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    ast::Program,
    ast_codec, ir_codec, parser,
    sema::{self, RynIr},
    source::{Diagnostic, Span},
};

/// The Ryn sources of the self-hosted frontend, relative to its project root.
pub const SELF_HOSTED_SOURCES: &[(&str, &str)] = &[
    ("ryn.yaml", include_str!("../selfhost/ryn.yaml")),
    ("src/main.ryn", include_str!("../selfhost/src/main.ryn")),
    (
        "src/frontend/lexer.ryn",
        include_str!("../selfhost/src/frontend/lexer.ryn"),
    ),
    (
        "src/frontend/parser.ryn",
        include_str!("../selfhost/src/frontend/parser.ryn"),
    ),
    (
        "src/frontend/generics.ryn",
        include_str!("../selfhost/src/frontend/generics.ryn"),
    ),
    (
        "src/middle/ast.ryn",
        include_str!("../selfhost/src/middle/ast.ryn"),
    ),
    (
        "src/middle/ir.ryn",
        include_str!("../selfhost/src/middle/ir.ryn"),
    ),
    (
        "src/middle/builtins.ryn",
        include_str!("../selfhost/src/middle/builtins.ryn"),
    ),
    (
        "src/middle/methods.ryn",
        include_str!("../selfhost/src/middle/methods.ryn"),
    ),
    (
        "src/middle/lower.ryn",
        include_str!("../selfhost/src/middle/lower.ryn"),
    ),
    (
        "src/middle/patterns.ryn",
        include_str!("../selfhost/src/middle/patterns.ryn"),
    ),
    (
        "src/middle/constants.ryn",
        include_str!("../selfhost/src/middle/constants.ryn"),
    ),
    (
        "src/middle/floatlib.ryn",
        include_str!("../selfhost/src/middle/floatlib.ryn"),
    ),
];

/// Which implementation turns source text into a syntax tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frontend {
    /// The parser built into the compiler (`src/lexer.rs`, `src/parser.rs`).
    Bootstrap,
    /// The Ryn-written frontend executable at this path.
    SelfHosted(PathBuf),
}

static NEXT_EXCHANGE: AtomicU64 = AtomicU64::new(0);

impl Frontend {
    /// Parses source text, reporting the first lexical or syntax error.
    pub fn parse(&self, text: &str) -> Result<Program, Diagnostic> {
        match self {
            Self::Bootstrap => parser::parse(text),
            Self::SelfHosted(executable) => {
                let parsed = run_self_hosted(executable, text, false)
                    .and_then(|encoded| ast_codec::decode_result(&encoded));
                match parsed {
                    Ok(program) => Ok(program),
                    // A syntax error of the Ryn parser must be the bootstrap parser's, as the other Ryn diagnostics
                    // are; a failure of the tool itself is reported as it is.
                    Err(reported) if reported.code == "R0901" || reported.code == "R0902" => {
                        Err(reported)
                    }
                    Err(reported) => {
                        if let Some(directory) = env::var_os("RYN_DUMP_SEMA") {
                            let _ = std::fs::write(
                                std::path::PathBuf::from(directory).join("ryn.ir"),
                                diagnostic_text(&reported),
                            );
                        }
                        let expected = parser::parse(text);
                        if let Err(diagnostic) = &expected
                            && same_diagnostic(diagnostic, &reported)
                        {
                            return Err(reported);
                        }
                        if env::var_os("RYN_SEMA_STRICT").is_some() {
                            return Err(Diagnostic {
                                code: "R0904",
                                message: "the self-hosted parser reported a syntax error the bootstrap parser does not".into(),
                                span: Span::default(),
                                help: Some(diagnostic_text(&reported)),
                            });
                        }
                        expected
                    }
                }
            }
        }
    }

    /// Parses source text, gathering independent lexical or syntax errors.
    pub fn parse_recovering(&self, text: &str) -> Result<Program, Vec<Diagnostic>> {
        match self {
            Self::Bootstrap => parser::parse_recovering(text),
            Self::SelfHosted(executable) => {
                let parsed = run_self_hosted(executable, text, true)
                    .map_err(|error| vec![error])
                    .and_then(|encoded| ast_codec::decode_recovering_result(&encoded));
                match parsed {
                    Ok(program) => Ok(program),
                    Err(reported)
                        if reported.iter().any(|diagnostic| {
                            diagnostic.code == "R0901" || diagnostic.code == "R0902"
                        }) =>
                    {
                        Err(reported)
                    }
                    Err(reported) => {
                        if let Some(directory) = env::var_os("RYN_DUMP_SEMA") {
                            let text = reported
                                .iter()
                                .map(diagnostic_text)
                                .collect::<Vec<_>>()
                                .join("\n");
                            let _ = std::fs::write(
                                std::path::PathBuf::from(directory).join("ryn.ir"),
                                text,
                            );
                        }
                        let expected = parser::parse_recovering(text);
                        if let Err(expected_list) = &expected
                            && expected_list.len() == reported.len()
                            && expected_list
                                .iter()
                                .zip(&reported)
                                .all(|(left, right)| same_diagnostic(left, right))
                        {
                            return Err(reported);
                        }
                        if env::var_os("RYN_SEMA_STRICT").is_some() {
                            return Err(vec![Diagnostic {
                                code: "R0904",
                                message: "the self-hosted parser reported syntax errors the bootstrap parser does not".into(),
                                span: Span::default(),
                                help: reported.first().map(diagnostic_text),
                            }]);
                        }
                        expected
                    }
                }
            }
        }
    }

    /// Specializes generic functions before semantic analysis. The
    /// self-hosted frontend runs its own pass; the bootstrap compiler
    /// specializes during semantic analysis, so the program is unchanged.
    pub fn monomorphize(&self, program: Program) -> Result<Program, Diagnostic> {
        match self {
            Self::Bootstrap => Ok(program),
            Self::SelfHosted(executable) => {
                let encoded = ast_codec::encode_program(&program);
                let specialized =
                    run_self_hosted_mode(executable, &encoded, Some("--monomorphize"))
                        .and_then(|output| ast_codec::decode_result(&output));
                match specialized {
                    Ok(specialized) => Ok(specialized),
                    Err(reported) => {
                        // A diagnostic of the Ryn specializer must be the bootstrap specializer's, as for the
                        // semantic analysis; otherwise the bootstrap's result stands.
                        if let Some(directory) = env::var_os("RYN_DUMP_SEMA") {
                            let _ = std::fs::write(
                                std::path::PathBuf::from(directory).join("ryn.ir"),
                                diagnostic_text(&reported),
                            );
                        }
                        let expected = crate::generics::monomorphize(program);
                        if let Err(diagnostic) = &expected
                            && same_diagnostic(diagnostic, &reported)
                        {
                            return Err(reported);
                        }
                        if env::var_os("RYN_SEMA_STRICT").is_some() {
                            return Err(Diagnostic {
                                code: "R0904",
                                message: "the self-hosted specialization reported a diagnostic the bootstrap specializer does not"
                                    .into(),
                                span: Span::default(),
                                help: Some(diagnostic_text(&reported)),
                            });
                        }
                        expected
                    }
                }
            }
        }
    }

    /// Runs semantic analysis.
    ///
    /// With the self-hosted frontend, the Ryn semantic analysis lowers the program to typed IR
    /// and the bootstrap analyzer checks the same program: invalid programs get the bootstrap's
    /// exact diagnostics, and an IR that differs from the bootstrap's is reported as an internal
    /// error instead of being compiled. A program the Ryn analysis does not handle is analyzed by
    /// the bootstrap alone. `RYN_SEMA=rust` skips the Ryn analysis.
    pub fn analyze(
        &self,
        mut program: Program,
        recovering: bool,
    ) -> Result<RynIr, Vec<Diagnostic>> {
        // Derived `Default` constructors must exist before constants are evaluated, so that a
        // `const` initialized with `Type::default()` can be folded.
        // The Ryn compile-time checks read the program before the bootstrap pass derives and folds it, and a diagnostic
        // they give must be the bootstrap pass's first: the derived defaults, then the constants.
        let comptime_text = match self {
            Self::SelfHosted(executable)
                if !env::var("RYN_SEMA").is_ok_and(|choice| choice == "rust") =>
            {
                run_self_hosted_mode(
                    executable,
                    &ast_codec::encode_program(&program),
                    Some("--comptime"),
                )
                .ok()
            }
            _ => None,
        };
        let evaluated = crate::comptime::derive_defaults(&mut program)
            .and_then(|()| crate::comptime::evaluate(&mut program));
        if let Some(text) = comptime_text.filter(|text| text.starts_with("diagnostic ")) {
            if let Some(directory) = env::var_os("RYN_DUMP_SEMA") {
                let _ = std::fs::write(std::path::PathBuf::from(directory).join("ryn.ir"), &text);
            }
            let agreed = match (&evaluated, parse_ryn_diagnostic(&text)) {
                (Err(reported), Some(ryn)) => diagnostic_agrees(reported, &ryn),
                _ => false,
            };
            if !agreed && env::var_os("RYN_SEMA_STRICT").is_some() {
                return Err(vec![Diagnostic {
                    code: "R0904",
                    message: "the self-hosted compile-time checks reported a diagnostic the bootstrap pass does not"
                        .into(),
                    span: Span::default(),
                    help: Some(text),
                }]);
            }
        }
        evaluated.map_err(|diagnostic| vec![diagnostic])?;
        // The Ryn pattern checks read `choose` arms as they are written, so they run before the bootstrap rewrites
        // them. A diagnostic they give must be the one the bootstrap's pattern pass gives.
        let pattern_text = match self {
            Self::SelfHosted(executable)
                if !env::var("RYN_SEMA").is_ok_and(|choice| choice == "rust") =>
            {
                run_self_hosted_mode(
                    executable,
                    &ast_codec::encode_program(&program),
                    Some("--patterns"),
                )
                .ok()
            }
            _ => None,
        };
        let desugared = crate::patterns::desugar(&mut program);
        if let Some(text) = pattern_text.filter(|text| text.starts_with("diagnostic ")) {
            if let Some(directory) = env::var_os("RYN_DUMP_SEMA") {
                let _ = std::fs::write(std::path::PathBuf::from(directory).join("ryn.ir"), &text);
            }
            let agreed = match (&desugared, parse_ryn_diagnostic(&text)) {
                (Err(reported), Some(ryn)) => diagnostic_agrees(reported, &ryn),
                _ => false,
            };
            if !agreed && env::var_os("RYN_SEMA_STRICT").is_some() {
                return Err(vec![Diagnostic {
                    code: "R0904",
                    message: "the self-hosted pattern checks reported a diagnostic the bootstrap pattern pass does not"
                        .into(),
                    span: Span::default(),
                    help: Some(text),
                }]);
            }
        }
        desugared.map_err(|diagnostic| vec![diagnostic])?;
        let bootstrap = |program: Program| {
            if recovering {
                sema::analyze_recovering(program)
            } else {
                sema::analyze(program).map_err(|error| vec![error])
            }
        };
        let Self::SelfHosted(executable) = self else {
            return bootstrap(program);
        };
        if env::var("RYN_SEMA").is_ok_and(|choice| choice == "rust") {
            return bootstrap(program);
        }
        let encoded = ast_codec::encode_program(&program);
        let outcome = bootstrap(program);
        let result = run_self_hosted_mode(executable, &encoded, Some("--sema"));
        // `RYN_DUMP_SEMA` writes what the Ryn analysis produced, whether or not it agrees with the bootstrap,
        // so that the diagnostics it reports itself can be counted.
        if let Some(directory) = env::var_os("RYN_DUMP_SEMA")
            && let Ok(text) = &result
        {
            let _ = std::fs::write(std::path::PathBuf::from(directory).join("ryn.ir"), text);
        }
        // The Ryn analysis reports some diagnostics itself. It must name one the bootstrap
        // analyzer also reports; a difference is an internal error under `RYN_SEMA_STRICT`, and
        // otherwise the bootstrap's diagnostics stand.
        if let Ok(text) = &result
            && text.starts_with("diagnostic ")
        {
            let agreed = match (&outcome, parse_ryn_diagnostic(text)) {
                (Err(reported), Some(ryn)) => reported
                    .iter()
                    .any(|diagnostic| diagnostic_agrees(diagnostic, &ryn)),
                _ => false,
            };
            if !agreed && env::var_os("RYN_SEMA_STRICT").is_some() {
                if let Some(directory) = env::var_os("RYN_DUMP_SEMA_DIFF") {
                    let _ =
                        std::fs::write(std::path::PathBuf::from(directory).join("actual.ir"), text);
                }
                return Err(vec![Diagnostic {
                    code: "R0904",
                    message: "the self-hosted semantic analysis reported a diagnostic the bootstrap analyzer does not"
                        .into(),
                    span: Span::default(),
                    help: Some(text.clone()),
                }]);
            }
            return outcome;
        }
        let expected = outcome?;
        match result {
            Ok(text) => {
                let bootstrap = ir_codec::encode_ir(&expected);
                if text == bootstrap {
                    ir_codec::decode_ir(&text).map_err(|message| {
                        vec![frontend_failure(format!(
                            "the self-hosted semantic analysis wrote IR that cannot be read: {message}"
                        ))]
                    })
                } else {
                    if let Some(directory) = env::var_os("RYN_DUMP_SEMA_DIFF") {
                        let directory = std::path::PathBuf::from(directory);
                        let _ = std::fs::write(directory.join("expected.ir"), &bootstrap);
                        let _ = std::fs::write(directory.join("actual.ir"), &text);
                    }
                    Err(vec![Diagnostic {
                        code: "R0904",
                        message: "the self-hosted semantic analysis disagrees with the bootstrap analyzer"
                            .into(),
                        span: Span::default(),
                        help: Some("compile with `RYN_SEMA=rust` and report the program".into()),
                    }])
                }
            }
            // Status 3 (outside what the Ryn analysis handles), a crash, or a missing tool: the
            // bootstrap result stands.
            Err(_) => Ok(expected),
        }
    }

    pub fn is_self_hosted(&self) -> bool {
        matches!(self, Self::SelfHosted(_))
    }
}

struct RynDiagnostic {
    code: String,
    start: usize,
    end: usize,
    message: String,
    help: String,
}

/// Whether two diagnostics are the same: code, span, message and help.
fn same_diagnostic(left: &Diagnostic, right: &Diagnostic) -> bool {
    left.code == right.code
        && left.message == right.message
        && left.span == right.span
        && left.help == right.help
}

/// A diagnostic in the text form that the Ryn tools write.
fn diagnostic_text(diagnostic: &Diagnostic) -> String {
    let help = diagnostic.help.clone().unwrap_or_default();
    format!(
        "diagnostic {} {} {} {}:{} {}:{}",
        diagnostic.code,
        diagnostic.span.start,
        diagnostic.span.end,
        diagnostic.message.len(),
        diagnostic.message,
        help.len(),
        help
    )
}

/// Whether the bootstrap's diagnostic is the one the Ryn analysis wrote: code, span, message and help.
fn diagnostic_agrees(diagnostic: &Diagnostic, ryn: &RynDiagnostic) -> bool {
    diagnostic.code == ryn.code
        && diagnostic.message == ryn.message
        && diagnostic.span.start == ryn.start
        && diagnostic.span.end == ryn.end
        && diagnostic.help.clone().unwrap_or_default() == ryn.help
}

/// Reads `diagnostic <code> <start> <end> <len>:<message> <len>:<help>`.
fn parse_ryn_diagnostic(text: &str) -> Option<RynDiagnostic> {
    let mut rest = text.strip_prefix("diagnostic ")?;
    let word = |rest: &mut &str| -> Option<String> {
        let (head, tail) = rest.split_once(' ')?;
        *rest = tail;
        Some(head.to_owned())
    };
    let code = word(&mut rest)?;
    let start = word(&mut rest)?.parse().ok()?;
    let end = word(&mut rest)?.parse().ok()?;
    let sized = |rest: &mut &str, last: bool| -> Option<String> {
        let (length, tail) = rest.split_once(':')?;
        let length: usize = length.parse().ok()?;
        let value = tail.get(..length)?.to_owned();
        *rest = tail.get(length..)?;
        if !last {
            *rest = rest.strip_prefix(' ')?;
        }
        Some(value)
    };
    let message = sized(&mut rest, false)?;
    let help = sized(&mut rest, true)?;
    Some(RynDiagnostic {
        code,
        start,
        end,
        message,
        help,
    })
}

fn frontend_failure(message: String) -> Diagnostic {
    Diagnostic {
        code: "R0902",
        message,
        span: Span::default(),
        help: Some("rebuild the frontend with `ryn bootstrap` or pass `--frontend rust`".into()),
    }
}

/// Runs the frontend executable on source text and returns its encoded output.
fn run_self_hosted(executable: &Path, text: &str, recovering: bool) -> Result<String, Diagnostic> {
    run_self_hosted_mode(executable, text, recovering.then_some("--recover"))
}

fn run_self_hosted_mode(
    executable: &Path,
    text: &str,
    mode: Option<&str>,
) -> Result<String, Diagnostic> {
    let exchange = env::temp_dir().join(format!(
        "ryn-frontend-{}-{}",
        std::process::id(),
        NEXT_EXCHANGE.fetch_add(1, Ordering::Relaxed)
    ));
    let input = exchange.with_extension("ryn");
    let output = exchange.with_extension("ast");
    let result = (|| {
        fs::write(&input, text).map_err(|error| {
            frontend_failure(format!(
                "cannot write frontend input {}: {error}",
                input.display()
            ))
        })?;
        let mut command = Command::new(executable);
        if let Some(mode) = mode {
            command.arg(mode);
        }
        let status = command.arg(&input).arg(&output).output().map_err(|error| {
            frontend_failure(format!(
                "cannot start the self-hosted frontend {}: {error}",
                executable.display()
            ))
        })?;
        if !status.status.success() {
            return Err(frontend_failure(format!(
                "the self-hosted frontend {} failed with {}",
                executable.display(),
                status.status
            )));
        }
        fs::read_to_string(&output).map_err(|error| {
            frontend_failure(format!(
                "cannot read frontend output {}: {error}",
                output.display()
            ))
        })
    })();
    let _ = fs::remove_file(&input);
    let _ = fs::remove_file(&output);
    result
}

/// A stable identifier of the embedded frontend sources and compiler version,
/// used to name the cached frontend executable.
pub fn self_hosted_fingerprint() -> String {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    env!("RYN_COMPILER_BUILD_FINGERPRINT").hash(&mut hasher);
    for (path, text) in SELF_HOSTED_SOURCES {
        path.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Writes the embedded frontend project into `directory`.
pub fn write_self_hosted_sources(directory: &Path) -> Result<(), String> {
    for (relative, text) in SELF_HOSTED_SOURCES {
        let path = directory.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("error[R0902]: cannot create {}: {error}", parent.display())
            })?;
        }
        fs::write(&path, text)
            .map_err(|error| format!("error[R0902]: cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Builds the self-hosted frontend from its embedded sources, parsing them
/// with `frontend`, and writes the executable to `output`.
pub fn build_self_hosted(
    work_directory: &Path,
    output: &Path,
    frontend: &Frontend,
) -> Result<(), String> {
    write_self_hosted_sources(work_directory)?;
    crate::compile_project_with_frontend(
        work_directory,
        output,
        crate::manifest::Optimize::Speed,
        frontend,
    )
}

fn executable_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// The per-user directory where the compiler keeps its built frontend.
pub fn cache_directory() -> PathBuf {
    let base = env::var_os("RYN_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("LOCALAPPDATA").map(|path| PathBuf::from(path).join("ryn")))
        .or_else(|| env::var_os("XDG_CACHE_HOME").map(|path| PathBuf::from(path).join("ryn")))
        .or_else(|| env::var_os("HOME").map(|path| PathBuf::from(path).join(".cache/ryn")))
        .unwrap_or_else(|| env::temp_dir().join("ryn-cache"));
    base.join("frontend").join(self_hosted_fingerprint())
}

/// Finds the self-hosted frontend executable, building and caching it with
/// the bootstrap parser when it does not exist yet.
///
/// `RYN_FRONTEND_EXE` selects an explicit executable; otherwise an
/// executable named `ryn-frontend` next to the compiler is used when present.
pub fn locate_or_build_self_hosted() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os("RYN_FRONTEND_EXE") {
        let explicit = PathBuf::from(explicit);
        if explicit.is_file() {
            return Ok(explicit);
        }
        return Err(format!(
            "error[R0902]: RYN_FRONTEND_EXE does not name a file: {}",
            explicit.display()
        ));
    }
    if let Some(sibling) = env::current_exe().ok().and_then(|compiler| {
        compiler
            .parent()
            .map(|directory| directory.join(executable_name("ryn-frontend")))
    }) && sibling.is_file()
    {
        return Ok(sibling);
    }
    let directory = cache_directory();
    let executable = directory.join(executable_name("ryn-frontend"));
    if executable.is_file() {
        return Ok(executable);
    }
    eprintln!("note: building the self-hosted Ryn frontend (first use)...");
    // Concurrent compilers may build at the same time: each builds in its own
    // directory and moves the finished executable into place.
    let work = directory.join(format!(
        "build-{}-{}",
        std::process::id(),
        NEXT_EXCHANGE.fetch_add(1, Ordering::Relaxed)
    ));
    let built = work.join(executable_name("ryn-frontend"));
    let result = build_self_hosted(&work.join("source"), &built, &Frontend::Bootstrap)
        .and_then(|()| install_executable(&built, &executable));
    let _ = fs::remove_dir_all(&work);
    result.map(|()| executable)
}

/// Moves a built frontend to `destination`, keeping an executable another
/// process installed first.
pub fn install_executable(built: &Path, destination: &Path) -> Result<(), String> {
    match fs::rename(built, destination) {
        Ok(()) => Ok(()),
        Err(_) if destination.is_file() => Ok(()),
        Err(error) => Err(format!(
            "error[R0902]: cannot install the self-hosted frontend at {}: {error}",
            destination.display()
        )),
    }
}

/// Resolves a frontend name from the command line or `RYN_FRONTEND`:
/// `ryn` (self-hosted, the default) or `rust` (bootstrap).
pub fn select(name: Option<&str>) -> Result<Frontend, String> {
    let from_environment = env::var("RYN_FRONTEND").ok();
    match name.or(from_environment.as_deref()).unwrap_or("ryn") {
        "rust" | "bootstrap" => Ok(Frontend::Bootstrap),
        "ryn" | "self-hosted" | "selfhost" => {
            locate_or_build_self_hosted().map(Frontend::SelfHosted)
        }
        other => Err(format!(
            "error[R0902]: unknown frontend `{other}`; use `ryn` or `rust`"
        )),
    }
}
