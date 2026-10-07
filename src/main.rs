use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{
    check_project_with_frontend, check_source_with_frontend, compile_project_with_frontend,
    compile_source_with_frontend,
    frontend::{self, Frontend},
    lockfile::{validate_project_lock_if_present, write_project_lock},
    manifest::{Dependency, Manifest, Optimize},
    registry,
    source::SourceFile,
};

fn main() {
    match run_cli() {
        Ok(Some(code)) => process::exit(code),
        Ok(None) => {}
        Err(message) => {
            eprintln!("{message}");
            process::exit(1);
        }
    }
}

fn run_cli() -> Result<Option<i32>, String> {
    let mut args = env::args_os().skip(1);
    let command = args
        .next()
        .and_then(|s| s.into_string().ok())
        .ok_or_else(usage)?;
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        if args.next().is_some() {
            return Err(usage());
        }
        print!("{}", help_text());
        return Ok(None);
    }
    if matches!(command.as_str(), "-V" | "--version" | "version") {
        if args.next().is_some() {
            return Err(usage());
        }
        println!("ryn {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    if command == "new" {
        let path = args.next().map(PathBuf::from).ok_or_else(usage)?;
        if args.next().is_some() {
            return Err(usage());
        }
        create_project(&path)?;
        return Ok(None);
    }
    if command == "clean" {
        let path = args.next().map(PathBuf::from).ok_or_else(usage)?;
        if args.next().is_some() {
            return Err(usage());
        }
        clean_project(&path)?;
        return Ok(None);
    }
    if command == "login" {
        let token = args
            .next()
            .and_then(|s| s.into_string().ok())
            .ok_or_else(usage)?;
        if args.next().is_some() {
            return Err(usage());
        }
        let path = registry::login(&token)?;
        println!("saved registry token to {}", path.display());
        return Ok(None);
    }
    if command == "publish" {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        if args.next().is_some() || !path.join("ryn.yaml").is_file() {
            return Err(usage());
        }
        // A package with an entry point must check cleanly before it is uploaded.
        // Library-only packages have no src/main.ryn and are checked by their users.
        if path.join("src").join("main.ryn").is_file() {
            check_project_with_frontend(&path, &frontend::select(None)?)?;
        }
        let url = registry::publish(&path)?;
        println!("published {url}");
        return Ok(None);
    }
    if command == "lock" {
        let path = args.next().map(PathBuf::from).ok_or_else(usage)?;
        if args.next().is_some() || !path.is_dir() {
            return Err(usage());
        }
        let lock = write_project_lock(&path)?;
        println!("locked {}", lock.display());
        return Ok(None);
    }
    if command == "ast" {
        // Prints the bootstrap parser's encoded syntax tree; the self-hosted
        // frontend must produce the same bytes for the same input.
        let mut recovering = false;
        let mut monomorphize = false;
        let mut path = None;
        for argument in args.by_ref() {
            match argument.to_str() {
                Some("--recover") if !recovering => recovering = true,
                Some("--monomorphize") if !monomorphize => monomorphize = true,
                _ if path.is_none() => path = Some(PathBuf::from(argument)),
                _ => return Err(usage()),
            }
        }
        let path = path.ok_or_else(usage)?;
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("error[R0001]: cannot read {}: {e}", path.display()))?;
        if monomorphize {
            print!(
                "{}",
                ryn::ast_codec::encode_result(
                    &ryn::parser::parse(&text).and_then(ryn::generics::monomorphize)
                )
            );
        } else if recovering {
            print!(
                "{}",
                ryn::ast_codec::encode_recovering_result(&ryn::parser::parse_recovering(&text))
            );
        } else {
            print!(
                "{}",
                ryn::ast_codec::encode_result(&ryn::parser::parse(&text))
            );
        }
        return Ok(None);
    }
    if command == "bootstrap" {
        if args.next().is_some() {
            return Err(usage());
        }
        bootstrap()?;
        return Ok(None);
    }
    if !matches!(command.as_str(), "check" | "build" | "run") {
        return Err(usage());
    }
    let mut requested_input = None;
    let mut output = None;
    let mut release = false;
    let mut frontend_name = None;
    let mut program_args = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--") if command == "run" => {
                program_args.extend(args);
                break;
            }
            Some("--frontend") if frontend_name.is_none() => {
                let name = args
                    .next()
                    .and_then(|name| name.into_string().ok())
                    .ok_or_else(usage)?;
                frontend_name = Some(name);
            }
            Some("-o" | "--output") if command != "check" && output.is_none() => {
                let path = args
                    .next()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(usage)?;
                output = Some(path);
            }
            Some("--release") if command != "check" && !release => release = true,
            _ if requested_input.is_none() => requested_input = Some(PathBuf::from(argument)),
            _ => return Err(usage()),
        }
    }
    let requested_input = requested_input.ok_or_else(usage)?;

    let project_manifest = if requested_input.is_dir() {
        Some(Manifest::load_project(&requested_input).map_err(|error| error.to_string())?)
    } else {
        None
    };
    if let Some(manifest) = &project_manifest {
        // The first build of a project with registry dependencies creates ryn.lock.
        let uses_registry = manifest
            .dependencies
            .values()
            .any(|dependency| matches!(dependency, Dependency::Version(_)));
        if uses_registry && !requested_input.join("ryn.lock").is_file() {
            let lock = write_project_lock(&requested_input)?;
            eprintln!("note: created {}", lock.display());
        }
        validate_project_lock_if_present(&requested_input)?;
    }
    let input = source_path(&requested_input)?;
    let source = SourceFile::load(&input)
        .map_err(|e| format!("error[R0001]: cannot read {}: {e}", input.display()))?;
    let frontend = frontend::select(frontend_name.as_deref())?;

    match command.as_str() {
        "check" => {
            if project_manifest.is_some() {
                check_project_with_frontend(&requested_input, &frontend)?;
            } else {
                check_source_with_frontend(&source, &frontend).map_err(|errors| {
                    errors
                        .iter()
                        .map(|error| error.render(&source))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })?;
            }
            println!("{}: ok", input.display());
            Ok(None)
        }
        "build" | "run" => {
            let output =
                output.unwrap_or_else(|| default_output_path(&requested_input, &input, release));
            let optimize = project_manifest
                .as_ref()
                .map(|manifest| manifest.build.optimize_for_profile(release))
                .unwrap_or(if release {
                    Optimize::Speed
                } else {
                    Optimize::None
                });
            let cache = if project_manifest.is_some() {
                Some(project_cache_entry(
                    &requested_input,
                    &output,
                    optimize,
                    &frontend,
                )?)
            } else {
                None
            };
            let cache_hit = cache.as_ref().is_some_and(|(path, key)| {
                output_fingerprint(&output).is_some_and(|artifact| {
                    fs::read_to_string(path).ok().as_deref()
                        == Some(format!("{key}:{artifact}").as_str())
                })
            });
            if !cache_hit {
                if project_manifest.is_some() {
                    compile_project_with_frontend(&requested_input, &output, optimize, &frontend)?;
                } else {
                    compile_source_with_frontend(&source, &output, optimize, &frontend)
                        .map_err(|error| error.render(&source))?;
                }
                if let Some((path, key)) = cache {
                    let artifact = output_fingerprint(&output).ok_or_else(|| {
                        format!(
                            "error[R0401]: cannot fingerprint build output {}",
                            output.display()
                        )
                    })?;
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(|error| {
                            format!("error[R0401]: cannot create build cache: {error}")
                        })?;
                    }
                    fs::write(path, format!("{key}:{artifact}")).map_err(|error| {
                        format!("error[R0401]: cannot write build cache: {error}")
                    })?;
                }
            }
            if command == "run" {
                let executable = fs::canonicalize(&output).map_err(|e| {
                    format!(
                        "error[R0301]: could not resolve executable {}: {e}",
                        output.display()
                    )
                })?;
                let status = process::Command::new(&executable)
                    .args(&program_args)
                    .status()
                    .map_err(|e| {
                        format!("error[R0301]: could not run {}: {e}", output.display())
                    })?;
                let code = status
                    .code()
                    .ok_or_else(|| format!("program terminated with {status}"))?;
                Ok(Some(code))
            } else {
                println!(
                    "{} {}",
                    if cache_hit { "up-to-date" } else { "built" },
                    output.display()
                );
                Ok(None)
            }
        }
        _ => Err(usage()),
    }
}

fn project_cache_entry(
    project: &Path,
    output: &Path,
    optimize: Optimize,
    frontend: &Frontend,
) -> Result<(PathBuf, String), String> {
    let project = fs::canonicalize(project).map_err(|error| {
        format!("error[R0401]: cannot resolve project for build cache: {error}")
    })?;
    let mut files = Vec::new();
    let mut visited_projects = HashSet::new();
    collect_project_inputs(&project, &mut visited_projects, &mut files)?;
    files.sort();

    let mut hash = StableHash::new();
    hash.feed(env!("CARGO_PKG_VERSION").as_bytes());
    hash.feed(format!("{optimize:?}").as_bytes());
    hash.feed(output.to_string_lossy().as_bytes());
    let compiler = env::current_exe().map_err(|error| {
        format!("error[R0401]: cannot locate compiler for build cache: {error}")
    })?;
    hash_file(&mut hash, &compiler)?;
    // Switching frontends rebuilds: a stale executable would hide a
    // self-hosted frontend regression.
    match frontend {
        Frontend::Bootstrap => hash.feed(b"frontend:bootstrap"),
        Frontend::SelfHosted(executable) => {
            hash.feed(b"frontend:self-hosted");
            hash_file(&mut hash, executable)?;
        }
    }
    for file in files {
        hash_file(&mut hash, &file)?;
    }
    let key = format!("{:016x}", hash.finish());
    let cache_dir = project.join("build").join("cache").join(match optimize {
        Optimize::Speed => "debug",
        Optimize::Size => "size",
        Optimize::None => "none",
    });
    let mut output_hash = StableHash::new();
    output_hash.feed(output.to_string_lossy().as_bytes());
    Ok((
        cache_dir.join(format!("{:016x}.fingerprint", output_hash.finish())),
        key,
    ))
}

fn output_fingerprint(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut hash = StableHash::new();
    hash.feed(&bytes);
    Some(format!("{:016x}", hash.finish()))
}

fn collect_project_inputs(
    project: &Path,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let project = fs::canonicalize(project)
        .map_err(|error| format!("error[R0410]: cannot resolve dependency project: {error}"))?;
    if !visited.insert(project.clone()) {
        return Ok(());
    }
    let manifest_path = project.join("ryn.yaml");
    let manifest = Manifest::load_project(&project).map_err(|error| error.to_string())?;
    files.push(manifest_path);
    let lockfile_path = project.join("ryn.lock");
    if lockfile_path.is_file() {
        files.push(lockfile_path);
    }
    let source_root = project.join("src");
    if source_root.is_dir() {
        collect_ryn_files(&source_root, files)?;
    }
    for dependency in manifest.dependencies.values() {
        if let Dependency::Path { path } = dependency {
            let dependency = if path.is_absolute() {
                path.clone()
            } else {
                project.join(path)
            };
            collect_project_inputs(&dependency, visited, files)?;
        }
    }
    Ok(())
}

fn collect_ryn_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("error[R0001]: cannot read source directory: {error}"))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("error[R0001]: cannot inspect source entry: {error}"))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("error[R0001]: cannot inspect source entry: {error}"))?;
        if kind.is_dir() {
            collect_ryn_files(&entry.path(), files)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "ryn") {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn hash_file(hash: &mut StableHash, path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "error[R0001]: cannot read {} for build cache: {error}",
            path.display()
        )
    })?;
    hash.feed(path.to_string_lossy().as_bytes());
    hash.feed(&bytes);
    Ok(())
}

struct StableHash(u64);

impl StableHash {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn feed(&mut self, bytes: &[u8]) {
        for byte in (bytes.len() as u64).to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

fn source_path(input: &Path) -> Result<PathBuf, String> {
    if input.is_dir() {
        let entry = input.join("src").join("main.ryn");
        if !entry.is_file() {
            return Err(format!(
                "error[R0001]: project entry point not found: {}",
                entry.display()
            ));
        }
        Ok(entry)
    } else {
        Ok(input.to_path_buf())
    }
}

fn create_project(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(format!(
            "error[R0400]: project path already exists: {}",
            path.display()
        ));
    }
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(usage)?
        .to_string_lossy();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "error[R0401]: cannot create parent directory {}: {error}",
            parent.display()
        )
    })?;

    let staging = create_staging_directory(parent)?;
    let result = (|| {
        let source_dir = staging.join("src");
        fs::create_dir(&source_dir).map_err(|error| {
            format!("error[R0401]: cannot create project source directory: {error}")
        })?;
        for directory in ["cache", "debug", "release"] {
            fs::create_dir_all(staging.join("build").join(directory)).map_err(|error| {
                format!("error[R0401]: cannot create build/{directory} directory: {error}")
            })?;
        }
        fs::write(
            source_dir.join("main.ryn"),
            "fun main() {\n    echo \"Hello, Ryn!\"\n}\n",
        )
        .map_err(|error| format!("error[R0401]: cannot write project entry point: {error}"))?;
        fs::write(
            staging.join("ryn.yaml"),
            format!(
                "name: {}\nversion: 0.1.0\nowner: guest\n\ndependencies:\n\nbuild:\n  optimize: speed\n",
                yaml_scalar(&name)
            ),
        )
        .map_err(|error| format!("error[R0401]: cannot write project manifest: {error}"))?;
        fs::rename(&staging, path).map_err(|error| {
            format!(
                "error[R0401]: cannot install project at {}: {error}",
                path.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    println!("created {}", path.display());
    Ok(())
}

fn yaml_scalar(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn create_staging_directory(parent: &Path) -> Result<PathBuf, String> {
    static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".ryn-new-{}-{id}", process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "error[R0401]: cannot create staging directory {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err("error[R0401]: cannot allocate a unique project staging directory".into())
}

fn clean_project(path: &Path) -> Result<(), String> {
    let project = fs::canonicalize(path).map_err(|error| {
        format!(
            "error[R0402]: cannot resolve project directory {}: {error}",
            path.display()
        )
    })?;
    if !project.is_dir() {
        return Err(format!(
            "error[R0402]: project path is not a directory: {}",
            path.display()
        ));
    }

    let build = project.join("build");
    let metadata = match fs::symlink_metadata(&build) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("no build artifacts in {}", project.display());
            return Ok(());
        }
        Err(error) => {
            return Err(format!(
                "error[R0402]: cannot inspect build directory {}: {error}",
                build.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "error[R0402]: refusing to clean a non-directory or symbolic link at {}",
            build.display()
        ));
    }

    let canonical_build = fs::canonicalize(&build).map_err(|error| {
        format!(
            "error[R0402]: cannot resolve build directory {}: {error}",
            build.display()
        )
    })?;
    let is_direct_build_directory = canonical_build.parent() == Some(project.as_path())
        && canonical_build
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("build"));
    if !is_direct_build_directory {
        return Err(format!(
            "error[R0402]: refusing to clean a path other than the project's direct build directory: {}",
            project.display()
        ));
    }

    fs::remove_dir_all(&canonical_build).map_err(|error| {
        format!(
            "error[R0402]: cannot remove build directory {}: {error}",
            canonical_build.display()
        )
    })?;
    println!("cleaned {}", canonical_build.display());
    Ok(())
}

fn default_output_path(requested_input: &Path, source: &Path, release: bool) -> PathBuf {
    if requested_input.is_dir() {
        let name = fs::canonicalize(requested_input)
            .ok()
            .and_then(|directory| directory.file_name().map(PathBuf::from))
            .or_else(|| source.file_stem().map(PathBuf::from))
            .unwrap_or_default();
        let profile = if release { "release" } else { "debug" };
        let mut output = requested_input.join("build").join(profile).join(name);
        if cfg!(windows) {
            output.set_extension("exe");
        }
        output
    } else {
        output_path(source)
    }
}

fn output_path(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default();
    let mut out = input.with_file_name(stem);
    if cfg!(windows) {
        out.set_extension("exe");
    } else {
        out.set_extension("");
    }
    out
}

fn usage() -> String {
    "usage:\n  ryn new <path>\n  ryn check <file.ryn|project-dir> [--frontend <ryn|rust>]\n  ryn build <file.ryn|project-dir> [--release] [-o|--output <path>] [--frontend <ryn|rust>]\n  ryn run <file.ryn|project-dir> [--release] [-o|--output <path>] [--frontend <ryn|rust>] [-- <program-args...>]\n  ryn clean <project-dir>\n  ryn lock <project-dir>\n  ryn login <token>\n  ryn publish [project-dir]\n  ryn bootstrap".into()
}

/// Rebuilds the self-hosted frontend with itself until it reproduces itself.
///
/// Stage 1 is built by the bootstrap parser, stage 2 by stage 1, and stage 3
/// by stage 2. Builds are reproducible, so a correct self-hosted frontend
/// makes all three executables identical. The verified stage is installed as
/// the compiler's cached frontend.
fn bootstrap() -> Result<(), String> {
    let cache = frontend::cache_directory();
    let work = cache.join("bootstrap");
    if work.exists() {
        fs::remove_dir_all(&work)
            .map_err(|error| format!("error[R0902]: cannot clear {}: {error}", work.display()))?;
    }
    let executable = |stage: usize| {
        work.join(if cfg!(windows) {
            format!("stage{stage}.exe")
        } else {
            format!("stage{stage}")
        })
    };
    let mut parser = Frontend::Bootstrap;
    let mut previous: Option<Vec<u8>> = None;
    for stage in 1..=3 {
        let started = std::time::Instant::now();
        let output = executable(stage);
        frontend::build_self_hosted(&work.join(format!("stage{stage}-src")), &output, &parser)?;
        let bytes = fs::read(&output)
            .map_err(|error| format!("error[R0902]: cannot read {}: {error}", output.display()))?;
        let parsed_by = match stage {
            1 => "the bootstrap parser (Rust)".to_owned(),
            _ => format!("the stage {} self-hosted frontend", stage - 1),
        };
        println!(
            "stage {stage}: frontend parsed by {parsed_by}, built in {:.1}s ({} bytes)",
            started.elapsed().as_secs_f64(),
            bytes.len()
        );
        if let Some(previous) = &previous
            && previous != &bytes
        {
            return Err(format!(
                "error[R0903]: stage {stage} differs from stage {}: the self-hosted frontend does not reproduce itself",
                stage - 1
            ));
        }
        previous = Some(bytes);
        parser = Frontend::SelfHosted(output);
    }
    for (path, text) in frontend::SELF_HOSTED_SOURCES {
        if !path.ends_with(".ryn") {
            continue;
        }
        let expected = ryn::ast_codec::encode_result(&ryn::parser::parse(text));
        let actual = ryn::ast_codec::encode_result(&parser.parse(text));
        if expected != actual {
            return Err(format!(
                "error[R0903]: the self-hosted frontend parses {path} differently from the bootstrap parser"
            ));
        }
    }
    let installed = cache.join(if cfg!(windows) {
        "ryn-frontend.exe"
    } else {
        "ryn-frontend"
    });
    if installed.is_file() {
        fs::remove_file(&installed).map_err(|error| {
            format!(
                "error[R0902]: cannot replace {}: {error}",
                installed.display()
            )
        })?;
    }
    frontend::install_executable(&executable(3), &installed)?;
    println!("fixpoint reached: stages 1, 2 and 3 are byte-identical");
    println!("installed {}", installed.display());
    Ok(())
}

fn help_text() -> String {
    format!(
        "Ryn — Reliable. Fast. Native.\n\n{}\n\nCommands:\n  new <path>        Create a project with a native Hello World example\n  check <source>    Check a .ryn file or project directory\n  build <source>    Compile a .ryn file or project directory\n  run <source>      Compile and run a .ryn file or project directory\n  lock <project>    Resolve and write ryn.lock for supported dependencies\n  login <token>     Save a pods registry token for publishing\n  publish [project] Check, pack and upload the project to the pods registry\n  clean <project>   Remove the project's build directory\n  bootstrap         Rebuild the self-hosted frontend with itself and verify the fixpoint\n\nA project directory uses src/main.ryn as its entry point.\nProject outputs go under build/debug or build/release; build/cache is reserved for compiler caches.\nAn i32 result from main becomes the process exit code for run.\nPass program arguments to run after --.\nclean removes only the direct build directory and refuses symbolic links.\n\nOptions:\n  --release            Use the project's release output directory\n  -o, --output <path>  Set the executable path for build or run\n  --frontend <name>    Parse with the self-hosted `ryn` frontend (default) or the `rust` bootstrap parser\n  -h, --help           Show this help\n  -V, --version        Show compiler version\n",
        usage()
    )
}
