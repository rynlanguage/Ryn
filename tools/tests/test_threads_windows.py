"""Execute Windows threads, including concurrent ownership cleanup and OS handles."""
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from ryn_harness import IS_WINDOWS, REPO_ROOT, ryn_binary


@unittest.skipUnless(IS_WINDOWS, "executes Windows threads")
class WindowsThreadTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ryn-threads-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.compiler = os.environ.get("RYN_TEST_RYNC") or str(Path(ryn_binary()).with_name("rync.exe"))

    def run_source(self, source, library=None):
        path = self.directory / "main.ryn"
        path.write_text(source, encoding="utf-8")
        executable = self.directory / "main.exe"
        common = [self.compiler, "--stdlib", (library or REPO_ROOT / "stdlib/std/src").as_posix(), "--target", "windows"]
        for args in (["check", path.as_posix()], ["-o", executable.as_posix(), path.as_posix()]):
            result = subprocess.run([*common, *args], capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(result.stderr, b"")
        image = executable.read_bytes()
        self.assertNotIn(b"pthread_create\0", image)
        result = subprocess.run([str(executable)], cwd=self.directory, capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(result.stderr, b"")
        return result.stdout

    def test_parallel_workers_allocate_grow_and_drop_owned_values(self):
        output = self.run_source('''use std::thread
type Worker = extern "C" fun()
fun worker() {
    for i in 0..2000 {
        mut text := String("worker")
        text.append(" data")
        mut values := Vec<String>()
        values.push(text)
        values.push(String("other"))
        assert(values.len() == 2)
        assert(values[0] == "worker data")
    }
    write("worker done\\n")
}
fun main() {
    for round in 0..10 {
        mut a := thread::Thread::spawn(worker as Worker)
        mut b := thread::Thread::spawn(worker as Worker)
        mut c := thread::Thread::spawn(worker as Worker)
        mut d := thread::Thread::spawn(worker as Worker)
        assert(a.id() > 0 as u64)
        assert(a.id() != b.id())
        assert(a.join()) assert(b.join()) assert(c.join()) assert(d.join())
    }
    echo "main done"
}
''')
        self.assertEqual(output, b"worker done\n" * 40 + b"main done\n")

    def test_join_once_and_scope_drop_close_windows_handles(self):
        output = self.run_source('''use std::thread
extern "C" fun GetCurrentProcess() -> *u8;
extern "C" fun GetProcessHandleCount(process: *u8, count: *u32) -> i32;
fun worker() {}
fun round() {
    mut worker := thread::Thread::spawn(worker)
    assert(worker.id() > 0 as u64)
    assert(worker.join())
    assert(!worker.join())
}
fun main() {
    mut before: u32 = 0
    mut after: u32 = 0
    assert(GetProcessHandleCount(GetCurrentProcess(), &raw mut before) != 0)
    for i in 0..100 { round() }
    assert(GetProcessHandleCount(GetCurrentProcess(), &raw mut after) != 0)
    assert(before == after)
    assert(!__thread_join(null))
    assert(__thread_id(null) == 0 as u64)
    __thread_drop(null)
    echo "handles closed"
}
''')
        self.assertEqual(output, b"handles closed\n")

    def test_drop_detaches_without_invalidating_worker_context(self):
        output = self.run_source('''use std::thread
fun worker() {
    sleep(40ms)
    text := String("detached done")
    assert(write_file("done.txt", text))
}
fun launch() { _ := thread::Thread::spawn(worker) }
fun main() {
    launch()
    for i in 0..1000 {
        temporary := String("reuse freed storage")
        when file_exists("done.txt") { break }
        sleep(1ms)
    }
    assert(file_exists("done.txt"))
    echo read_file("done.txt")
}
''')
        self.assertEqual(output, b"detached done\n")

    def test_create_failure_is_a_safe_empty_thread(self):
        library = self.directory / "stdlib"
        shutil.copytree(REPO_ROOT / "stdlib/std/src", library)
        runtime = library / "native_windows.ryn"
        source = runtime.read_text(encoding="utf-8")
        declaration = 'extern "C" fun CreateThread(attributes: *u8, stack_size: u64, routine: *u8, argument: *u8, flags: u32, identifier: *u8) -> *u8;'
        self.assertEqual(source.count(declaration), 1)
        runtime.write_text(source.replace(declaration, declaration.replace('extern "C" ', '').replace(';', ' { return null }')), encoding="utf-8")
        output = self.run_source('''use std::thread
fun worker() { echo "must not run" }
fun main() {
    mut thread := thread::Thread::spawn(worker)
    assert(thread.id() == 0 as u64)
    assert(!thread.join())
    echo "failure handled"
}
''', library)
        self.assertEqual(output, b"failure handled\n")

    def test_mutex_try_lock_and_guard_cleanup_on_return(self):
        output = self.run_source('''use std::sync
fun early(handle: *u8) -> i32 {
    guard := sync::lock_raw(handle)
    assert(guard.is_held())
    return 42
}
fun main() {
    mutex := sync::Mutex::new()
    assert(mutex.is_valid())
    when true {
        held := mutex.lock()
        assert(held.is_held())
        denied := mutex.try_lock()
        assert(!denied.is_held())
    }
    assert(early(mutex.raw()) == 42)
    when true {
        acquired := mutex.try_lock()
        assert(acquired.is_held())
    }
    invalid := sync::Mutex { handle: null }
    assert(!invalid.is_valid())
    invalid_lock := invalid.lock()
    invalid_try := invalid.try_lock()
    invalid_raw := sync::lock_raw(null)
    assert(!invalid_lock.is_held())
    assert(!invalid_try.is_held())
    assert(!invalid_raw.is_held())
    echo "mutex guards ready"
}
''')
        self.assertEqual(output, b"mutex guards ready\n")

    def test_mutex_serializes_a_plain_counter_across_windows_workers(self):
        output = self.run_source('''use std::sync
namespace c;
extern "C" fun malloc(size: u64) -> *u8;
extern "C" fun free(pointer: *u8);
extern "C" fun CreateThread(attributes: *u8, stack_size: u64, routine: *u8, argument: *u8, flags: u32, identifier: *u8) -> *u8;
extern "C" fun WaitForSingleObject(handle: *u8, milliseconds: u32) -> u32;
extern "C" fun CloseHandle(handle: *u8) -> i32;
namespace;
fun join_and_close(handle: *u8) {
    assert(handle != null)
    assert(c::WaitForSingleObject(handle, 4294967295 as u32) == 0)
    assert(c::CloseHandle(handle) != 0)
}
fun increment(record: *u8) -> u32 {
    mutex_word := *(record as *u64)
    mutex := mutex_word as *u8
    counter := ((record as u64) + 8) as *u64
    for i in 0..20000 {
        guard := sync::lock_raw(mutex)
        assert(guard.is_held())
        *counter = *counter + 1
    }
    return 0
}
fun main() {
    mutex := sync::Mutex::new()
    assert(mutex.is_valid())
    record := c::malloc(16)
    first := record as *u64
    *first = mutex.raw() as u64
    counter := ((record as u64) + 8) as *u64
    *counter = 0
    worker: extern "C" fun(*u8) -> u32 = increment
    a := c::CreateThread(null, 0, worker as *u8, record, 0, null)
    b := c::CreateThread(null, 0, worker as *u8, record, 0, null)
    c := c::CreateThread(null, 0, worker as *u8, record, 0, null)
    d := c::CreateThread(null, 0, worker as *u8, record, 0, null)
    join_and_close(a) join_and_close(b) join_and_close(c) join_and_close(d)
    echo *counter
    c::free(record)
}
''')
        self.assertEqual(output, b"80000\n")


if __name__ == "__main__":
    unittest.main()
