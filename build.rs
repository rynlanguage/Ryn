use std::{env, path::PathBuf, process::Command};

fn main() {
    configure_windows_stack();
    if let Err(error) = build_runtime_shim() {
        panic!("could not build the native runtime shim: {error}");
    }
}

fn configure_windows_stack() {
    let target = env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        // The Windows executable default is 1 MiB. Deep but valid Ryn functions
        // can use more during recursive semantic analysis, so reserve 8 MiB.
        println!("cargo:rustc-link-arg=/STACK:8388608");
    } else if target.contains("windows-gnu") {
        println!("cargo:rustc-link-arg=-Wl,--stack,8388608");
    }
}

fn build_runtime_shim() -> Result<(), String> {
    println!("cargo:rerun-if-changed=src/runtime_shim.rs");
    println!("cargo:rerun-if-changed=src/runtime/string.rs");
    println!("cargo:rerun-if-changed=src/runtime/vector.rs");
    println!("cargo:rerun-if-changed=src/runtime/map.rs");
    println!("cargo:rerun-if-changed=src/runtime/enum.rs");
    println!("cargo:rerun-if-changed=src/runtime/filesystem.rs");
    println!("cargo:rerun-if-changed=src/runtime/system.rs");
    println!("cargo:rerun-if-changed=src/runtime/input.rs");

    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let target = env::var("TARGET").map_err(|error| error.to_string())?;
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let object_path = out_dir.join(if target.contains("windows") {
        "ryn_runtime_shim.obj"
    } else {
        "ryn_runtime_shim.o"
    });

    let mut command = Command::new(&rustc);
    command
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
        .arg(&object_path);
    let status = command
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
