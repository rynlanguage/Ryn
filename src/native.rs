//! The native compiler `rync`: the Ryn-written compiler (`selfhost/src/rync.ryn`) that `ryn build` and `ryn run` use
//! by default on Linux x86-64.
//!
//! `rync` lexes, parses, specializes, evaluates compile-time code, checks ownership, generates x86-64 machine code and
//! links the executable itself; the programs it writes contain no Rust code. This module finds the `rync` executable
//! (or builds it from the sources embedded in this compiler on first use), runs it, and reports its diagnostics.
//!
//! Building `rync` uses the bootstrap compiler once: the Rust frontend lowers `rync.ryn` to typed IR, the Ryn code
//! generator (`selfhost/src/native.ryn`, compiled with Cranelift) turns that IR into a native seed, and the seed compiles
//! `rync.ryn` again. That second executable, built by Ryn code alone, is the one installed. `ryn bootstrap` also checks
//! that it reproduces itself byte for byte.

use std::{
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};

use crate::{frontend::Frontend, ir_codec, manifest::Optimize};

include!(concat!(env!("OUT_DIR"), "/native_sources.rs"));

static NEXT_WORK_ID: AtomicU64 = AtomicU64::new(0);

/// Which compiler turns a checked program into an executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The self-hosted native compiler `rync` (Linux x86-64).
    Native,
    /// The bootstrap compiler's Cranelift code generator, kept for compatibility and for other targets.
    Cranelift,
}

impl Backend {
    /// Resolves `--backend <name>`, then `RYN_BACKEND`; the default is `native` where it can target the host.
    pub fn select(name: Option<&str>) -> Result<Self, String> {
        let from_environment = env::var("RYN_BACKEND")
            .ok()
            .filter(|value| !value.is_empty());
        match name.or(from_environment.as_deref()) {
            None => Ok(Self::host_default()),
            Some("native" | "rync") => {
                if native_supported() {
                    Ok(Self::Native)
                } else {
                    Err("error[R0907]: the native backend targets Linux x86-64 only; use `--backend cranelift`".into())
                }
            }
            Some("cranelift") => Ok(Self::Cranelift),
            Some(other) => Err(format!(
                "error[R0907]: unknown backend `{other}`; use `native` or `cranelift`"
            )),
        }
    }

    /// The backend used when none is chosen.
    pub fn host_default() -> Self {
        if native_supported() {
            Self::Native
        } else {
            Self::Cranelift
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Cranelift => "cranelift",
        }
    }
}

/// Whether the native compiler can produce executables for the host.
pub fn native_supported() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// An identifier of the embedded `rync` and standard-library sources and the compiler version. The built `rync` is a
/// function of these sources alone (it reproduces itself), so the identifier names its cache directory.
pub fn fingerprint() -> String {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    for (path, text) in NATIVE_COMPILER_SOURCES
        .iter()
        .chain(NATIVE_STDLIB_SOURCES.iter())
    {
        path.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// The per-user directory of the native compiler built from these sources.
pub fn cache_directory() -> PathBuf {
    let base = env::var_os("RYN_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_CACHE_HOME").map(|path| PathBuf::from(path).join("ryn")))
        .or_else(|| env::var_os("HOME").map(|path| PathBuf::from(path).join(".cache/ryn")))
        .unwrap_or_else(|| env::temp_dir().join("ryn-cache"));
    base.join("native").join(fingerprint())
}

fn write_tree(directory: &Path, files: &[(&str, &str)]) -> Result<(), String> {
    for (relative, text) in files {
        let path = directory.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("error[R0906]: cannot create {}: {error}", parent.display())
            })?;
        }
        fs::write(&path, text)
            .map_err(|error| format!("error[R0906]: cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// The standard library directory `rync` reads `std::x` from: `RYN_STDLIB`, or the embedded standard library written
/// once into the cache.
pub fn stdlib_directory() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os("RYN_STDLIB") {
        let explicit = PathBuf::from(explicit);
        if explicit.is_dir() {
            return Ok(explicit);
        }
        return Err(format!(
            "error[R0906]: RYN_STDLIB does not name a directory: {}",
            explicit.display()
        ));
    }
    let directory = cache_directory().join("stdlib/std/src");
    let marker = directory.join(".complete");
    if marker.is_file() {
        return Ok(directory);
    }
    // Each process writes its own copy and moves it into place, so concurrent compilers never see half of it.
    let staging = cache_directory().join(format!(
        "stdlib-{}-{}",
        std::process::id(),
        NEXT_WORK_ID.fetch_add(1, Ordering::Relaxed)
    ));
    write_tree(&staging, NATIVE_STDLIB_SOURCES)?;
    fs::write(staging.join(".complete"), b"")
        .map_err(|error| format!("error[R0906]: cannot write the standard library: {error}"))?;
    if let Some(parent) = directory.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("error[R0906]: cannot create {}: {error}", parent.display())
        })?;
    }
    if fs::rename(&staging, &directory).is_err() {
        let _ = fs::remove_dir_all(&staging);
        if !marker.is_file() {
            return Err(format!(
                "error[R0906]: cannot install the standard library at {}",
                directory.display()
            ));
        }
    }
    Ok(directory)
}

/// Finds `rync`: `RYN_NATIVE_COMPILER`, a `rync` next to this compiler, or the one built from the embedded sources
/// (building it the first time, which takes about a minute).
// A lock whose holder process has ended was left by an interrupted build, so it is stale at once. Without a holder
// that can be checked, a lock counts as stale after half an hour.
fn lock_is_stale(lock: &Path) -> bool {
    let holder = fs::read_to_string(lock)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    if let (Some(pid), true) = (holder, cfg!(target_os = "linux")) {
        return !Path::new(&format!("/proc/{pid}")).exists();
    }
    fs::metadata(lock)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(1800))
}

pub fn locate_or_build() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os("RYN_NATIVE_COMPILER") {
        let explicit = PathBuf::from(explicit);
        if explicit.is_file() {
            return Ok(explicit);
        }
        return Err(format!(
            "error[R0906]: RYN_NATIVE_COMPILER does not name a file: {}",
            explicit.display()
        ));
    }
    if let Some(sibling) = env::current_exe()
        .ok()
        .and_then(|compiler| compiler.parent().map(|directory| directory.join("rync")))
        && sibling.is_file()
    {
        return Ok(sibling);
    }
    let directory = cache_directory();
    let installed = directory.join("rync");
    if installed.is_file() {
        return Ok(installed);
    }
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "error[R0906]: cannot create {}: {error}",
            directory.display()
        )
    })?;
    // One process builds; the others wait for its result.
    let lock = directory.join("build.lock");
    let started = Instant::now();
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(mut file) => {
                // The lock names its holder, so that a build that was interrupted does not block the others.
                let _ =
                    std::io::Write::write_all(&mut file, std::process::id().to_string().as_bytes());
                break;
            }
            Err(_) => {
                if installed.is_file() {
                    return Ok(installed);
                }
                if lock_is_stale(&lock) {
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                if started.elapsed() > Duration::from_secs(3600) {
                    return Err(format!(
                        "error[R0906]: timed out waiting for another process to build the native compiler ({})",
                        lock.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    let result = (|| {
        if installed.is_file() {
            return Ok(());
        }
        eprintln!("note: building the native Ryn compiler `rync` (first use)...");
        let work = directory.join(format!("build-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        let outcome = bootstrap(&work, false, &mut |_| {});
        let outcome = outcome.and_then(|report| {
            fs::rename(&report.stage2, &installed).map_err(|error| {
                format!(
                    "error[R0906]: cannot install the native compiler at {}: {error}",
                    installed.display()
                )
            })
        });
        let _ = fs::remove_dir_all(&work);
        outcome
    })();
    let _ = fs::remove_file(&lock);
    result.map(|()| installed)
}

/// What a bootstrap built and measured.
pub struct BootstrapReport {
    /// `rync` built by the seed: compiled by Ryn code only.
    pub stage2: PathBuf,
    /// `rync` built by stage 2, when the fixpoint was checked.
    pub stage3: Option<PathBuf>,
    pub seed_seconds: f64,
    pub stage2_seconds: f64,
    pub stage3_seconds: Option<f64>,
}

/// Builds `rync` in `work`: the seed from the bootstrap compiler's IR, stage 2 with the seed, and with `verify` stage 3
/// with stage 2, which must be byte-identical to stage 2.
pub fn bootstrap(
    work: &Path,
    verify: bool,
    progress: &mut dyn FnMut(&str),
) -> Result<BootstrapReport, String> {
    let sources = work.join("selfhost/src");
    write_tree(&sources, NATIVE_COMPILER_SOURCES)?;
    let stdlib = work.join("stdlib/std/src");
    write_tree(&stdlib, NATIVE_STDLIB_SOURCES)?;
    let started = Instant::now();
    // The seed: the bootstrap frontend and analyzer lower `rync.ryn`, and the Ryn code generator writes the executable.
    let ir = crate::check_file_with_imports(sources.join("rync.ryn"), &Frontend::Bootstrap)
        .map_err(|error| {
            format!("error[R0906]: the bootstrap compiler rejects rync.ryn:\n{error}")
        })?;
    let ir_path = work.join("rync.ir");
    fs::write(&ir_path, ir_codec::encode_ir(&ir))
        .map_err(|error| format!("error[R0906]: cannot write {}: {error}", ir_path.display()))?;
    let driver = work.join("native-driver");
    crate::compile_file_with_imports(
        sources.join("native.ryn"),
        &driver,
        Optimize::Speed,
        &Frontend::Bootstrap,
    )
    .map_err(|error| {
        format!("error[R0906]: cannot build the native code generator driver:\n{error}")
    })?;
    let seed = work.join("rync-seed");
    let seed_file = fs::File::create(&seed)
        .map_err(|error| format!("error[R0906]: cannot create {}: {error}", seed.display()))?;
    let generated = Command::new(&driver)
        .arg(&ir_path)
        .arg("exe")
        .stdout(seed_file)
        .output()
        .map_err(|error| format!("error[R0906]: cannot run {}: {error}", driver.display()))?;
    if !generated.status.success() {
        return Err(format!(
            "error[R0906]: the native code generator cannot build the seed ({}):\n{}",
            generated.status,
            String::from_utf8_lossy(&generated.stderr)
        ));
    }
    make_executable(&seed)?;
    let seed_seconds = started.elapsed().as_secs_f64();
    progress(&format!("seed built in {seed_seconds:.1}s"));
    let stage2 = work.join("rync-stage2");
    let stage2_started = Instant::now();
    compile_compiler(&seed, &stdlib, &sources, &stage2)?;
    let stage2_seconds = stage2_started.elapsed().as_secs_f64();
    progress(&format!(
        "stage 2 built by the seed in {stage2_seconds:.1}s"
    ));
    let mut report = BootstrapReport {
        stage2: stage2.clone(),
        stage3: None,
        seed_seconds,
        stage2_seconds,
        stage3_seconds: None,
    };
    if verify {
        let stage3 = work.join("rync-stage3");
        let stage3_started = Instant::now();
        compile_compiler(&stage2, &stdlib, &sources, &stage3)?;
        let stage3_seconds = stage3_started.elapsed().as_secs_f64();
        progress(&format!("stage 3 built by stage 2 in {stage3_seconds:.1}s"));
        let second = fs::read(&stage2)
            .map_err(|error| format!("error[R0906]: cannot read {}: {error}", stage2.display()))?;
        let third = fs::read(&stage3)
            .map_err(|error| format!("error[R0906]: cannot read {}: {error}", stage3.display()))?;
        if second != third {
            return Err(
                "error[R0903]: stage 3 differs from stage 2: the native compiler does not reproduce itself".into(),
            );
        }
        report.stage3 = Some(stage3);
        report.stage3_seconds = Some(stage3_seconds);
    }
    Ok(report)
}

fn compile_compiler(
    compiler: &Path,
    stdlib: &Path,
    sources: &Path,
    output: &Path,
) -> Result<(), String> {
    let result = Command::new(compiler)
        .arg("build")
        .arg("--stdlib")
        .arg(stdlib)
        .arg("-o")
        .arg(output)
        .arg(sources.join("rync.ryn"))
        .output()
        .map_err(|error| format!("error[R0906]: cannot run {}: {error}", compiler.display()))?;
    if !result.status.success() {
        return Err(format!(
            "error[R0906]: {} cannot compile rync.ryn ({}):\n{}",
            compiler.display(),
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| {
        format!(
            "error[R0906]: cannot make {} executable: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// One run of `rync`.
pub struct Invocation<'a> {
    /// Check the program without generating code.
    pub check: bool,
    /// The entry file or the project directory.
    pub input: &'a Path,
    pub output: Option<&'a Path>,
    /// The source directories of the dependency packages, by package name.
    pub packages: Vec<(String, PathBuf)>,
}

/// Runs `rync` and returns its diagnostics as the error. A program the native pipeline cannot compile is an error too:
/// nothing falls back to another backend.
pub fn run(compiler: &Path, invocation: &Invocation<'_>) -> Result<(), String> {
    let stdlib = stdlib_directory()?;
    let mut command = Command::new(compiler);
    command.arg(if invocation.check { "check" } else { "build" });
    command.arg("--stdlib").arg(&stdlib);
    for (name, root) in &invocation.packages {
        let mut pair = std::ffi::OsString::from(name);
        pair.push("=");
        pair.push(root);
        command.arg("--package").arg(pair);
    }
    if let Some(output) = invocation.output {
        command.arg("-o").arg(output);
    }
    command.arg(invocation.input);
    let result = command.output().map_err(|error| {
        format!(
            "error[R0906]: cannot run the native compiler {}: {error}",
            compiler.display()
        )
    })?;
    let stderr = String::from_utf8_lossy(&result.stderr)
        .trim_end()
        .to_owned();
    match result.status.code() {
        Some(0) => Ok(()),
        Some(1 | 3) => Err(stderr),
        _ => Err(format!(
            "error[R0906]: the native compiler {} failed ({}){}{}",
            compiler.display(),
            result.status,
            if stderr.is_empty() { "" } else { ":\n" },
            stderr
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::lock_is_stale;
    use std::{fs, path::PathBuf};

    fn lock_naming(holder: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ryn-native-lock-{}-{}",
            std::process::id(),
            holder.len()
        ));
        fs::write(&path, holder).expect("lock file is written");
        path
    }

    #[test]
    fn a_lock_held_by_a_running_process_is_not_stale() {
        let lock = lock_naming(&std::process::id().to_string());
        let stale = lock_is_stale(&lock);
        let _ = fs::remove_file(&lock);
        assert!(!stale);
    }

    #[test]
    fn a_lock_held_by_a_process_that_has_ended_is_stale_at_once() {
        // No process has this id: Linux allocates pids below 2^22.
        let lock = lock_naming("4294967295");
        let stale = lock_is_stale(&lock);
        let _ = fs::remove_file(&lock);
        assert!(stale);
    }
}
