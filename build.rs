use std::{env, path::PathBuf, process::Command};

fn main() {
    if let Err(error) = build_runtime_shim() {
        panic!("could not build the native runtime shim: {error}");
    }
}

fn build_runtime_shim() -> Result<(), String> {
    println!("cargo:rerun-if-changed=src/runtime_shim.rs");

    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let target = env::var("TARGET").map_err(|error| error.to_string())?;
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let object_path = out_dir.join(if target.contains("windows") {
        "ryn_runtime_shim.obj"
    } else {
        "ryn_runtime_shim.o"
    });

    let status = Command::new(&rustc)
        .args([
            "--edition=2024",
            "--crate-name",
            "ryn_runtime_shim",
            "--crate-type=lib",
            "src/runtime_shim.rs",
            "--emit=obj",
            "-C",
            "debuginfo=0",
            "--target",
            &target,
            "-o",
        ])
        .arg(&object_path)
        .status()
        .map_err(|error| format!("could not compile runtime shim with rustc: {error}"))?;
    if !status.success() {
        return Err(format!("runtime shim compilation exited with {status}"));
    }

    println!(
        "cargo:rustc-env=RYN_RUNTIME_SHIM_OBJECT_PATH={}",
        object_path.display()
    );
    Ok(())
}
