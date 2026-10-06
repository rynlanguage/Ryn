use std::{fs, path::PathBuf, process::Command};

fn project_check(project: &PathBuf) -> Result<(), String> {
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(project)
        .output()
        .expect("ryn process starts");
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

fn write_project(directory: &std::path::Path, manifest: &str, main: &str, library: &str) {
    fs::create_dir_all(directory.join("src/game")).expect("project src directories are created");
    fs::write(directory.join("ryn.yaml"), manifest).expect("manifest is written");
    fs::write(directory.join("src/main.ryn"), main).expect("entry point is written");
    fs::write(directory.join("src/game/player.ryn"), library).expect("module is written");
}

#[test]
fn private_fields_are_invisible_outside_their_module_but_methods_work() {
    let project = tempfile_directory("object-visibility");
    write_project(
        &project,
        "name: visibility-demo\nversion: 0.1.0\nowner: guest\n",
        // main.ryn — another module: `health` is private, `health_value()` is pub.
        concat!(
            "use game::player\n",
            "\n",
            "fun main() {\n",
            "    mut hero := player::Player { name: String(\"hero\"), health: 50 }\n",
            "    echo hero.health_value()\n",
            "    hero.damage(10)\n",
            "    echo hero.health_value()\n",
            "}",
        ),
        // game/player.ryn
        concat!(
            "pub struct Player {\n",
            "    pub name: String,\n",
            "    health: i32,\n",
            "}\n",
            "\n",
            "extend Player {\n",
            "    pub fun health_value(self) -> i32 => self.health\n",
            "\n",
            "    pub fun damage(mut self, amount: i32) {\n",
            "        self.health -= amount\n",
            "    }\n",
            "}\n",
        ),
    );

    // The pub method works across modules.
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&project)
        .output()
        .expect("ryn process starts");
    assert!(
        output.status.success(),
        "project failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "50\n40\n");

    // Reading the private field directly is rejected.
    let private = main_with_private_field_read();
    fs::write(project.join("src/main.ryn"), private).expect("entry point is rewritten");
    let error = project_check(&project).expect_err("private field access must fail");
    assert!(
        error.contains("R0426") && error.contains("is private"),
        "expected a private-field diagnostic, got: {error}"
    );
    let _ = fs::remove_dir_all(project);
}

fn main_with_private_field_read() -> String {
    concat!(
        "use game::player\n",
        "\n",
        "fun main() {\n",
        "    hero := player::Player { name: String(\"hero\"), health: 50 }\n",
        "    echo hero.health\n",
        "}",
    )
    .into()
}

fn tempfile_directory(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("ryn-{label}-{}-{unique}", std::process::id()));
    fs::create_dir(&path).expect("temporary test directory is created");
    path
}
