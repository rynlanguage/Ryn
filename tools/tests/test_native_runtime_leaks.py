"""Port of tests/native_runtime_leaks.rs: native runtime resources are released by their owners.

After warm-up, repeated file, process, thread and socket use leaves the open descriptors (counted in
/proc/self/fd) unchanged. Linux only.

The live-allocation counter does not cover these programs: they link the C library, whose malloc is not
counted, so the native compiler declines the counter in them (see
test_the_live_allocation_counter_is_declined_in_a_libc_program).
"""

import subprocess
import sys
import unittest

from ryn_harness import Run, TempDir, run_ryn


def build_native(directory, source_text):
    """The program is built natively in a fresh directory."""
    source = directory / "main.ryn"
    source.write_text(source_text, encoding="utf-8")
    return run_ryn("build", source, "-o", directory / "program")


def run_native(program, cwd):
    """Runs a built program from `cwd`, like `Command::new(..).current_dir(..).output()`."""
    proc = subprocess.run([str(program)], cwd=str(cwd), capture_output=True)
    return Run(
        proc.returncode,
        proc.stdout.decode("utf-8", errors="replace"),
        proc.stderr.decode("utf-8", errors="replace"),
    )


@unittest.skipUnless(sys.platform.startswith("linux"), "descriptor counts come from /proc/self/fd")
class NativeRuntimeLeakTests(unittest.TestCase):
    def assert_no_growth(self, name, source_text):
        with TempDir("runtime-leaks") as directory:
            build = build_native(directory, source_text)
            self.assertEqual(build.code, 0, f"{name} builds natively: {build.stderr}")
            output = run_native(directory / "program", directory)

        self.assertEqual(output.code, 0, f"{name} exits with success; stderr: {output.stderr}")
        self.assertEqual(
            output.stdout,
            "0\n",
            f"{name}: the descriptor count must not grow; stderr: {output.stderr}",
        )

    def test_file_handles_close_or_drop_their_descriptors(self):
        self.assert_no_growth(
            "files",
            r"""use std::fs

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
""",
        )

    def test_processes_release_their_pipes_and_results(self):
        self.assert_no_growth(
            "processes",
            r"""use std::fs
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
""",
        )

    def test_threads_release_their_handles_after_join(self):
        self.assert_no_growth(
            "threads",
            r"""use std::fs
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
""",
        )

    def test_sockets_close_their_descriptors_when_dropped(self):
        self.assert_no_growth(
            "sockets",
            r"""use std::fs
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
""",
        )

    def test_the_live_allocation_counter_is_declined_in_a_libc_program(self):
        with TempDir("runtime-leaks") as directory:
            build = build_native(
                directory,
                r"""use std::fs
extern "C" fun ryn_native_live_allocations() -> u64;

fun main() {
    echo fs::read_dir("/proc/self/fd").len()
    echo ryn_native_live_allocations()
}
""",
            )

        # The Rust-era assertion expected exit status 1. The self-hosted native backend reports a
        # construct it lacks with status 3 (docs/self-hosting-plan.md, "status 3"), so this asserts 3.
        self.assertEqual(build.code, 3, build.stderr)
        self.assertIn("error[R0905]", build.stderr)
        self.assertIn(
            "the live allocation counter of a program that uses the C library",
            build.stderr,
        )


if __name__ == "__main__":
    unittest.main()
