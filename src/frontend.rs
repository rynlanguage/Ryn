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
    ast_codec, parser,
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
            Self::SelfHosted(executable) => run_self_hosted(executable, text, false)
                .and_then(|encoded| ast_codec::decode_result(&encoded)),
        }
    }

    /// Parses source text, gathering independent lexical or syntax errors.
    pub fn parse_recovering(&self, text: &str) -> Result<Program, Vec<Diagnostic>> {
        match self {
            Self::Bootstrap => parser::parse_recovering(text),
            Self::SelfHosted(executable) => run_self_hosted(executable, text, true)
                .map_err(|error| vec![error])
                .and_then(|encoded| ast_codec::decode_recovering_result(&encoded)),
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
                run_self_hosted_mode(executable, &encoded, Some("--monomorphize"))
                    .and_then(|output| ast_codec::decode_result(&output))
            }
        }
    }

    pub fn is_self_hosted(&self) -> bool {
        matches!(self, Self::SelfHosted(_))
    }
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
        let status = command.arg(&input).arg(&output).status().map_err(|error| {
            frontend_failure(format!(
                "cannot start the self-hosted frontend {}: {error}",
                executable.display()
            ))
        })?;
        if !status.success() {
            return Err(frontend_failure(format!(
                "the self-hosted frontend {} failed with {status}",
                executable.display()
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
