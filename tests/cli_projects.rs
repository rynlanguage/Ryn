use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ryn-{label}-{}-{unique}", std::process::id()));
        fs::create_dir(&path).expect("temporary test directory is created");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn new_project_can_be_checked_built_and_run_from_its_directory() {
    let temporary = TestDirectory::new("project-workflow");
    let project = temporary.0.join("hello Ryn");

    let created = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("new")
        .arg(&project)
        .output()
        .expect("ryn process starts");
    assert!(
        created.status.success(),
        "project creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let entry = project.join("src").join("main.ryn");
    assert_eq!(
        fs::read_to_string(&entry).expect("project entry point is written"),
        "fn main() {\n    print(\"Hello, Ryn!\")\n}\n"
    );
    let project_readme =
        fs::read_to_string(project.join("README.md")).expect("project README is written");
    assert!(project_readme.contains("ryn run ."));
    assert!(project_readme.contains("ryn clean ."));

    let checked = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(".")
        .current_dir(&project)
        .output()
        .expect("ryn process starts");
    assert!(
        checked.status.success(),
        "project check failed: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(String::from_utf8_lossy(&checked.stdout).contains("main.ryn: ok"));

    let built = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("build")
        .arg(".")
        .current_dir(&project)
        .output()
        .expect("ryn process starts");
    assert!(
        built.status.success(),
        "project build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mut executable = project.join("build").join("hello Ryn");
    if cfg!(windows) {
        executable.set_extension("exe");
    }
    assert!(executable.is_file());

    let run = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(".")
        .current_dir(&project)
        .output()
        .expect("ryn process starts");
    assert!(
        run.status.success(),
        "project run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "Hello, Ryn!\n");
}

#[test]
fn run_propagates_the_i32_main_exit_code_after_flushing_output() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/exit_code.ryn");
    let temporary = TestDirectory::new("exit-code");
    let output = temporary.0.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(source)
        .arg("--output")
        .arg(output)
        .output()
        .expect("ryn process starts");

    assert_eq!(result.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Ryn exit status: 7\n"
    );
    assert!(result.stderr.is_empty());
}

#[test]
fn new_refuses_to_overwrite_an_existing_project_directory() {
    let temporary = TestDirectory::new("project-existing");
    let project = temporary.0.join("existing");
    fs::create_dir(&project).expect("existing project directory is created");
    let marker = project.join("keep.txt");
    fs::write(&marker, "keep me").expect("existing project file is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("new")
        .arg(&project)
        .output()
        .expect("ryn process starts");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("error[R0400]"));
    assert_eq!(
        fs::read_to_string(marker).expect("existing project file remains readable"),
        "keep me"
    );
}

#[test]
fn project_commands_explain_when_the_entry_point_is_missing() {
    let temporary = TestDirectory::new("project-no-entry");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&temporary.0)
        .output()
        .expect("ryn process starts");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("project entry point not found:"));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("src")
            && String::from_utf8_lossy(&result.stderr).contains("main.ryn")
    );
}

#[test]
fn clean_removes_only_the_project_build_directory() {
    let temporary = TestDirectory::new("project-clean");
    let project = temporary.0.join("app");
    let source_dir = project.join("src");
    let build_dir = project.join("build").join("nested");
    fs::create_dir_all(&source_dir).expect("project source directory is created");
    fs::create_dir_all(&build_dir).expect("project build directory is created");
    fs::write(source_dir.join("main.ryn"), "fn main() {}").expect("project source is written");
    fs::write(build_dir.join("app.exe"), "generated").expect("build artifact is written");
    let custom_output = project.join("custom-output.exe");
    fs::write(&custom_output, "keep").expect("custom output is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("clean")
        .arg(&project)
        .output()
        .expect("ryn process starts");

    assert!(
        result.status.success(),
        "clean failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!project.join("build").exists());
    assert!(source_dir.join("main.ryn").is_file());
    assert_eq!(
        fs::read_to_string(custom_output).expect("custom output remains readable"),
        "keep"
    );
}

#[test]
fn clean_without_build_artifacts_succeeds_without_changing_the_project() {
    let temporary = TestDirectory::new("project-clean-empty");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("clean")
        .arg(&temporary.0)
        .output()
        .expect("ryn process starts");

    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("no build artifacts"));
    assert!(temporary.0.is_dir());
}

#[test]
fn clean_refuses_to_delete_a_file_named_build() {
    let temporary = TestDirectory::new("project-clean-build-file");
    let build_file = temporary.0.join("build");
    fs::write(&build_file, "keep").expect("file named build is written");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("clean")
        .arg(&temporary.0)
        .output()
        .expect("ryn process starts");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("non-directory"));
    assert_eq!(
        fs::read_to_string(build_file).expect("file named build remains readable"),
        "keep"
    );
}

#[cfg(unix)]
#[test]
fn clean_refuses_a_build_symlink_and_preserves_its_target() {
    use std::os::unix::fs::symlink;

    let temporary = TestDirectory::new("project-clean-symlink");
    let project = temporary.0.join("project");
    let outside = temporary.0.join("outside");
    fs::create_dir(&project).expect("project directory is created");
    fs::create_dir(&outside).expect("outside directory is created");
    let marker = outside.join("keep.txt");
    fs::write(&marker, "keep").expect("outside marker is written");
    symlink(&outside, project.join("build")).expect("build symlink is created");

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("clean")
        .arg(&project)
        .output()
        .expect("ryn process starts");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symbolic link"));
    assert_eq!(
        fs::read_to_string(marker).expect("outside marker remains readable"),
        "keep"
    );
}

#[cfg(windows)]
#[test]
fn clean_refuses_a_build_junction_and_preserves_its_target() {
    let temporary = TestDirectory::new("project-clean-junction");
    let project = temporary.0.join("project");
    let outside = temporary.0.join("outside");
    fs::create_dir(&project).expect("project directory is created");
    fs::create_dir(&outside).expect("junction target directory is created");
    let marker = outside.join("keep.txt");
    fs::write(&marker, "keep").expect("outside marker is written");
    let junction = project.join("build");
    let created = Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("Windows command processor starts");
    assert!(
        created.status.success(),
        "junction creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("clean")
        .arg(&project)
        .output()
        .expect("ryn process starts");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symbolic link"));
    assert_eq!(
        fs::read_to_string(marker).expect("outside marker remains readable"),
        "keep"
    );
    fs::remove_dir(junction).expect("junction is removed without touching its target");
}
