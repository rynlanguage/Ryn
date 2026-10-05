use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use ryn::{check_source_recovering, compile_source, source::SourceFile};

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
    if !matches!(command.as_str(), "check" | "build" | "run") {
        return Err(usage());
    }
    let requested_input = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut output = None;
    let mut program_args = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--") if command == "run" => {
                program_args.extend(args);
                break;
            }
            Some("-o" | "--output") if command != "check" && output.is_none() => {
                let path = args
                    .next()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .ok_or_else(usage)?;
                output = Some(path);
            }
            _ => return Err(usage()),
        }
    }

    let input = source_path(&requested_input)?;
    let source = SourceFile::load(&input)
        .map_err(|e| format!("error[R0001]: cannot read {}: {e}", input.display()))?;

    match command.as_str() {
        "check" => {
            check_source_recovering(&source).map_err(|errors| {
                errors
                    .iter()
                    .map(|error| error.render(&source))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })?;
            println!("{}: ok", input.display());
            Ok(None)
        }
        "build" | "run" => {
            let output = output.unwrap_or_else(|| default_output_path(&requested_input, &input));
            compile_source(&source, &output).map_err(|error| error.render(&source))?;
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
                println!("built {}", output.display());
                Ok(None)
            }
        }
        _ => Err(usage()),
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
        fs::write(
            source_dir.join("main.ryn"),
            "fn main() {\n    print(\"Hello, Ryn!\")\n}\n",
        )
        .map_err(|error| format!("error[R0401]: cannot write project entry point: {error}"))?;
        fs::write(
            staging.join("README.md"),
            format!(
                "# {name}\n\nA native application written in Ryn.\n\nRun it from this directory with:\n\n```powershell\nryn run .\n```\n\nRemove the generated executable with:\n\n```powershell\nryn clean .\n```\n"
            ),
        )
        .map_err(|error| format!("error[R0401]: cannot write project README: {error}"))?;
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

fn default_output_path(requested_input: &Path, source: &Path) -> PathBuf {
    if requested_input.is_dir() {
        let name = fs::canonicalize(requested_input)
            .ok()
            .and_then(|directory| directory.file_name().map(PathBuf::from))
            .or_else(|| source.file_stem().map(PathBuf::from))
            .unwrap_or_default();
        let mut output = requested_input.join("build").join(name);
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
    "usage:\n  ryn new <path>\n  ryn check <file.ryn|project-dir>\n  ryn build <file.ryn|project-dir> [-o|--output <path>]\n  ryn run <file.ryn|project-dir> [-o|--output <path>] [-- <program-args...>]\n  ryn clean <project-dir>".into()
}

fn help_text() -> String {
    format!(
        "Ryn — Reliable. Fast. Native.\n\n{}\n\nCommands:\n  new <path>        Create a project with a native Hello World example\n  check <source>    Check a .ryn file or project directory\n  build <source>    Compile a .ryn file or project directory\n  run <source>      Compile and run a .ryn file or project directory\n  clean <project>   Remove the project's build directory\n\nA project directory uses src/main.ryn as its entry point.\nAn i32 result from main becomes the process exit code for run.\nPass program arguments to run after --.\nclean removes only the direct build directory and refuses symbolic links.\n\nOptions:\n  -o, --output <path>  Set the executable path for build or run\n  -h, --help           Show this help\n  -V, --version        Show compiler version\n",
        usage()
    )
}
