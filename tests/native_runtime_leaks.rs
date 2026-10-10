//! Native runtime resources are released by their owners: after warm-up, repeated file, process, thread and socket use
//! leaves the open descriptors (counted in `/proc/self/fd`) unchanged. Linux only.
//!
//! The live-allocation counter does not cover these programs: they link the C library, whose `malloc` is not counted, so
//! the native compiler declines the counter in them (see `the_live_allocation_counter_is_declined_in_a_libc_program`).

#![cfg(target_os = "linux")]

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

/// The program is built natively in a fresh directory, then run from that directory, which is its working directory.
fn build_native(source_text: &str) -> (PathBuf, Output) {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory: PathBuf =
        std::env::temp_dir().join(format!("ryn-runtime-leaks-{}-{id}", std::process::id()));
    fs::create_dir_all(&directory).expect("temporary directory is created");
    let source = directory.join("main.ryn");
    fs::write(&source, source_text).expect("Ryn source is written");
    let build = Command::new(
        std::env::var("RYN_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_ryn").to_string()),
    )
    .arg("build")
    .arg(&source)
    .arg("-o")
    .arg(directory.join("program"))
    .output()
    .expect("Ryn process starts");
    (directory, build)
}

fn assert_no_growth(name: &str, source_text: &str) {
    let (directory, build) = build_native(source_text);
    assert!(
        build.status.success(),
        "{name} builds natively: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let output = Command::new(directory.join("program"))
        .current_dir(&directory)
        .output()
        .expect("the built program starts");
    let _ = fs::remove_dir_all(&directory);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{name} exits with success; stderr: {stderr}"
    );
    assert_eq!(
        stdout, "0\n",
        "{name}: the descriptor count must not grow; stderr: {stderr}"
    );
}

#[test]
fn file_handles_close_or_drop_their_descriptors() {
    assert_no_growth(
        "files",
        r#"use std::fs

fun open_descriptors() -> u64 {
    return fs::read_dir("/proc/self/fd").len() as u64
}

fun round() {
    mut writer := fs::File::create("leak.txt")
    assert(writer.write("line one\nline two\n"))
    assert(writer.close())
    mut reader := fs::File::open("leak.txt")
    reader.read_line()
    assert(reader.close())
    mut dropped := fs::File::create("dropped.txt")
    assert(dropped.write("left to its destructor"))
}

fun main() {
    round()
    descriptors := open_descriptors()
    mut count: u64 = 0
    while count < 200 {
        round()
        count += 1
    }
    echo open_descriptors() - descriptors
}
"#,
    );
}

#[test]
fn processes_release_their_pipes_and_results() {
    assert_no_growth(
        "processes",
        r#"use std::fs
use std::process

fun open_descriptors() -> u64 {
    return fs::read_dir("/proc/self/fd").len() as u64
}

fun round() {
    mut arguments := Vec<String>()
    arguments.push(String("-c"))
    arguments.push(String("echo captured; echo failed 1>&2"))
    result := process::Process::run_capture_args("/bin/sh", arguments)
    assert(result.exit_code == 0)
}

fun main() {
    round()
    descriptors := open_descriptors()
    mut count: u64 = 0
    while count < 100 {
        round()
        count += 1
    }
    echo open_descriptors() - descriptors
}
"#,
    );
}

#[test]
fn threads_release_their_handles_after_join() {
    assert_no_growth(
        "threads",
        r#"use std::fs
use std::thread
type Worker = extern "C" fun()

fun worker() {
}

fun open_descriptors() -> u64 {
    return fs::read_dir("/proc/self/fd").len() as u64
}

fun round() {
    mut background := thread::Thread::spawn(worker as Worker)
    assert(background.join())
}

fun main() {
    round()
    descriptors := open_descriptors()
    mut count: u64 = 0
    while count < 100 {
        round()
        count += 1
    }
    echo open_descriptors() - descriptors
}
"#,
    );
}

#[test]
fn sockets_close_their_descriptors_when_dropped() {
    assert_no_growth(
        "sockets",
        r#"use std::fs
use std::net

fun open_descriptors() -> u64 {
    return fs::read_dir("/proc/self/fd").len() as u64
}

fun round() {
    mut listener := net::TcpListener::bind("127.0.0.1:45690")
    assert(listener.close())
    mut socket := net::UdpSocket::bind("127.0.0.1:45691")
    assert(socket.close())
    mut dropped := net::UdpSocket::bind("127.0.0.1:45692")
}

fun main() {
    round()
    descriptors := open_descriptors()
    mut count: u64 = 0
    while count < 100 {
        round()
        count += 1
    }
    echo open_descriptors() - descriptors
}
"#,
    );
}

#[test]
fn the_live_allocation_counter_is_declined_in_a_libc_program() {
    let (directory, build) = build_native(
        r#"use std::fs
extern "C" fun ryn_native_live_allocations() -> u64;

fun main() {
    echo fs::read_dir("/proc/self/fd").len()
    echo ryn_native_live_allocations()
}
"#,
    );
    let _ = fs::remove_dir_all(directory);
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert_eq!(build.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("error[R0905]"), "{stderr}");
    assert!(
        stderr.contains("the live allocation counter of a program that uses the C library"),
        "{stderr}"
    );
}
