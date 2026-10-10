"""Port of tests/interactive_input.rs: program input from stdin pipes, the console prompt API,
loopback TCP and UDP sockets, and the standard-library programs the Rust file also drives.

Each test runs the ryn binary named by RYN_TEST_BIN through `ryn run` or `ryn check`, feeds stdin
where the Rust test did, and asserts stdout, stderr and exit codes exactly as the Rust test does.
Socket peers run on background threads, as the Rust tests run theirs with std::thread.
"""

import socket
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

from ryn_harness import IS_WINDOWS, Run, TempDir, ryn_binary

# Safety net: a Ryn program that never finishes fails its test instead of hanging the whole run.
TIMEOUT = 120
# How long a socket peer waits for the Ryn program, as in the Rust peers.
PEER_TIMEOUT = 10
# Invalid UTF-8 first line, then "ok line" and a two-byte character.
BAD_UTF8 = b"\xffab\nok line\n\xc3\xa9\n"


def _run(*args: object, cwd: Path | None = None, stdin: bytes = b"") -> Run:
    """Runs the ryn binary with the given bytes on stdin, like Command::output() with a piped stdin."""
    proc = subprocess.run(
        [ryn_binary(), *(str(arg) for arg in args)],
        cwd=None if cwd is None else str(cwd),
        input=stdin,
        capture_output=True,
        timeout=TIMEOUT,
    )
    return Run(
        proc.returncode,
        proc.stdout.decode("utf-8", errors="replace"),
        proc.stderr.decode("utf-8", errors="replace"),
    )


def _write(path: Path, content: str | bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content.encode("utf-8") if isinstance(content, str) else content)
    return path


def _run_source(source_text: str, stdin: bytes = b"") -> Run:
    """Runs a single-file program with `ryn run main.ryn` (the Rust run_with_stdin helper)."""
    with TempDir("input") as directory:
        source = _write(directory / "main.ryn", source_text)
        return _run("run", source, stdin=stdin)


def _run_project(
    source_text: str,
    name: str = "input-api-test",
    stdin: bytes = b"",
    files: dict[str, bytes] | None = None,
) -> Run:
    """Runs `ryn run <dir>` in a fresh project with its working directory set to the project
    (the Rust run_project_with_stdin helper). `files` maps project-relative paths to fixture bytes."""
    with TempDir("project") as directory:
        _write(
            directory / "ryn.yaml",
            f"name: {name}\nversion: 0.1.0\nowner: guest\ndependencies:\n",
        )
        _write(directory / "src" / "main.ryn", source_text)
        for relative, content in (files or {}).items():
            _write(directory / relative, content)
        return _run("run", directory, cwd=directory, stdin=stdin)


def _run_file_project(source_text: str, name: str = "file-handle-test", extra: dict[str, bytes] | None = None) -> Run:
    """Runs a project whose work directory `files/` holds `files/bad.txt`, which has bytes that are
    not UTF-8, plus any `extra` fixtures (the Rust run_file_handle_project helper)."""
    files = {"files/bad.txt": BAD_UTF8, **(extra or {})}
    return _run_project(source_text, name=name, files=files)


def _recv_exact(stream: socket.socket, length: int) -> bytes:
    """Reads exactly `length` bytes, failing on EOF like Rust's read_exact."""
    data = b""
    while len(data) < length:
        chunk = stream.recv(length - len(data))
        if not chunk:
            raise AssertionError(f"peer reached EOF after {len(data)} of {length} bytes")
        data += chunk
    return data


class _Peer:
    """A socket peer on its own thread. join() re-raises whatever the peer raised, like the Rust
    `peer.join().expect(...)` calls."""

    def __init__(self, body):
        self._body = body
        self._error: BaseException | None = None
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        try:
            self._body()
        except BaseException as error:  # re-raised by join()
            self._error = error

    def start(self) -> "_Peer":
        self._thread.start()
        return self

    def join(self) -> None:
        self._thread.join()
        if self._error is not None:
            raise self._error


class InteractiveInputTests(unittest.TestCase):
    def assertSucceeded(self, run: Run) -> None:
        self.assertEqual(
            run.code,
            0,
            f"status={run.code} stdout={run.stdout!r} stderr={run.stderr}",
        )

    def test_line_input_handles_lf_crlf_utf8_blank_lines_sequential_reads_and_eof(self):
        run = _run_source(
            "fun main() { first := read_line() second := stdin_read_line() third := ask(\"Q: \") echo \"{first}|{second}|{third}\" }",
            "zażółć gęślą jaźń\r\n你好🦀\n\n".encode("utf-8"),
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "Q: zażółć gęślą jaźń|你好🦀|\n")

        run = _run_source(
            "fun main() { value := read_line() echo \"<{value}>\" }",
            b"",
        )
        self.assertSucceeded(run)  # EOF should return an empty String
        self.assertEqual(run.stdout, "<>\n")

    def test_ask_flushes_prompt_before_waiting_and_stdin_read_still_reads_to_eof(self):
        with TempDir("input") as directory:
            source = _write(
                directory / "main.ryn",
                "fun main() { name := ask(\"Name: \") echo \"Hello {name}!\" }",
            )
            proc = subprocess.Popen(
                [ryn_binary(), "run", str(source)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                prompt = proc.stdout.read(6)
                self.assertEqual(prompt, b"Name: ", "flushed prompt is readable before input")
                proc.stdin.write(b"v3nn7\r\n")
                proc.stdin.flush()
                remainder, _ = proc.communicate(timeout=TIMEOUT)  # closes stdin, reads to EOF
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()
            self.assertEqual(proc.returncode, 0)
            self.assertEqual(remainder.decode("utf-8", errors="replace"), "Hello v3nn7!\n")

        run = _run_source("fun main() { data := stdin_read() echo data }", b"a\nb\n")
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "a\nb\n\n")

    def test_standard_io_module_exposes_the_console_api(self):
        run = _run_project(
            "use std::io\nuse std::env\nuse std::system\nfun main() { io::print(\"Name: \") io::stdout_flush() name := io::read_line() io::println(\"Hello!\") io::eprintln(\"done\") assert(!env::current_dir().is_empty()) assert(!system::os().is_empty()) assert(system::cpu_count() > 0) }",
            name="std-io-smoke",
            stdin="你好\n".encode("utf-8"),
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "Name: Hello!\n")
        self.assertEqual(run.stderr, "done\n")

    def test_standard_math_module_exposes_f64_operations_and_random_ranges(self):
        run = _run_project(
            "use std::math\nfun main() { echo math::sqrt(9.0) echo math::pow(2.0, 3.0) value := math::random_range(2.0, 4.0) assert(value >= 2.0) assert(value < 4.0) }",
            name="std-math-smoke",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "3\n8\n")

    def test_string_methods_cover_unicode_text_and_line_operations(self):
        run = _run_source(
            "fun main() { value := String(\"  HéLLo\\r\\nRyn  \") echo value.trim_start() echo value.trim_end() echo value.to_lower() echo value.to_upper() echo value.replace(\"Ryn\", \"Lang\") echo value.reverse() echo value.substring(2, 7) echo value.is_empty() lines := value.lines() echo lines.len() chars := value.chars() echo chars.len() bytes := value.bytes() echo bytes.len() echo String(\"ha\").repeat(3) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(
            run.stdout,
            "HéLLo\r\nRyn  \n  HéLLo\r\nRyn\n  héllo\r\nryn  \n  HÉLLO\r\nRYN  \n  HéLLo\r\nLang  \n  nyR\n\roLLéH  \nHéLLo\nfalse\n2\n14\n15\nhahaha\n",
        )

    def test_string_parse_and_try_parse_accept_explicit_numeric_types(self):
        run = _run_source(
            "fun main() { value := String(\"42\") echo value.parse::<i32>() echo value.try_parse::<u64>().unwrap_or(0) invalid := String(\"bad\").try_parse::<i32>() echo invalid.is_none() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "42\n42\ntrue\n")

    def test_string_numeric_conversions_cover_all_signed_unsigned_and_float_widths(self):
        run = _run_source(
            "fun main() { integer := String(\"42\") echo integer.to_i8() echo integer.to_i16() echo integer.to_i32() echo integer.to_i64() echo integer.to_u8() echo integer.to_u16() echo integer.to_u32() echo integer.to_u64() decimal := String(\"3.5\") echo decimal.to_f32() echo decimal.to_f64() echo (42 as i8).to_string() echo (42 as i16).to_string() echo (42 as i32).to_string() echo (42 as i64).to_string() echo (42 as u8).to_string() echo (42 as u16).to_string() echo (42 as u32).to_string() echo (42 as u64).to_string() echo (3.5 as f32).to_string() echo (3.5 as f64).to_string() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(
            run.stdout,
            "42\n42\n42\n42\n42\n42\n42\n42\n3.5\n3.5\n42\n42\n42\n42\n42\n42\n42\n42\n3.5\n3.5\n",
        )

    def test_vector_and_map_expose_is_empty(self):
        run = _run_source(
            "fun main() { mut values := Vec<i32>() echo values.is_empty() values.push(1) echo values.is_empty() mut names := Map<String, i32>() echo names.is_empty() names.insert(String(\"x\"), 1) echo names.is_empty() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "true\nfalse\ntrue\nfalse\n")

    def test_set_uses_map_storage_with_set_semantics_and_owned_string_keys(self):
        run = _run_source(
            "fun main() { mut values := Set<String>() assert(values.is_empty()) assert(!values.contains(String(\"ry\"))) assert(!values.insert(String(\"ry\"))) assert(!values.is_empty()) assert(values.contains(String(\"ry\"))) assert(values.insert(String(\"ry\"))) echo values.len() assert(values.remove(String(\"ry\"))) assert(values.is_empty()) values.insert(String(\"owned\")) values.clear() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "1\n")

    def test_map_get_returns_owned_option_and_keys_values_clone_without_aliasing(self):
        run = _run_source(
            "fun main() { mut values := Map<String, String>() values.insert(String(\"first\"), String(\"zażółć\")) selected := values.get(String(\"first\")) missing := values.get(String(\"missing\")) assert(selected.is_some()) assert(missing.is_none()) assert(values.remove(String(\"first\"))) echo selected.unwrap_or(String(\"fallback\")) echo missing.unwrap_or(String(\"empty\")) mut other := Map<String, String>() other.insert(String(\"a\"), String(\"one\")) other.insert(String(\"b\"), String(\"two\")) mut keys := other.keys() mut values_list := other.values() other.clear() echo keys.len() echo values_list.len() echo keys.take(0) echo values_list.take(0) echo keys.take(0) echo values_list.take(0) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "zażółć\nempty\n2\n2\na\none\nb\ntwo\n")

    def test_generic_helper_accepts_and_returns_specialized_option_types(self):
        run = _run_source(
            "fun with_fallback<T>(value: Option<T>, fallback: T) -> T { return value.unwrap_or(fallback) } fun main() { some: Option<i32> = Option::Some(7) none: Option<i32> = Option::None echo with_fallback(some, 2 as i32) echo with_fallback(none, 2 as i32) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "7\n2\n")

    def test_vec_rejects_sets_until_set_clone_drop_callbacks_are_supported(self):
        with TempDir("input") as directory:
            source = _write(
                directory / "main.ryn",
                "fun main() { mut set := Set<String>() mut sets := Vec<Set<String>>() sets.push(set) }",
            )
            run = _run("check", source)
        self.assertNotEqual(run.code, 0)
        self.assertIn("is not supported yet", run.stderr)

    def test_generic_helper_accepts_specialized_result_types_and_unifies_ok_and_error_payloads(self):
        run = _run_source(
            "fun value_or<T, E>(value: Result<T, E>, fallback: T) -> T { return value.unwrap_or(fallback) } fun main() { good: Result<i32, String> = Result::Ok(9) bad: Result<i32, String> = Result::Err(String(\"bad\")) echo value_or(good, 3 as i32) echo value_or(bad, 3 as i32) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "9\n3\n")

    def test_vector_insert_remove_and_reverse_keep_element_order_and_ownership(self):
        run = _run_source(
            "fun main() { mut values := Vec<i32>() values.push(1) values.push(3) values.insert(1, 2) values.reverse() echo values.index(0) echo values.remove(1) echo values.index(1) mut words := Vec<String>() words.insert(0, String(\"first\")) words.insert(1, String(\"second\")) words.reverse() echo words.take(0) words.clear() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "3\n2\n1\nsecond\n")

    def test_vector_sort_orders_signed_unsigned_float_and_char_values(self):
        run = _run_source(
            "fun main() { mut values := Vec<i32>() values.push(5) values.push(-2) values.push(9) values.sort() echo values.index(0) echo values.index(1) echo values.index(2) mut unsigned := Vec<u32>() unsigned.push(4000000000) unsigned.push(2) unsigned.sort() echo unsigned.index(0) echo unsigned.index(1) mut floats := Vec<f64>() floats.push(3.5) floats.push(-1.25) floats.sort() echo floats.index(0) echo floats.index(1) mut chars := Vec<char>() chars.push('z') chars.push('a') chars.sort() echo chars.index(0) echo chars.index(1) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "-2\n5\n9\n2\n4000000000\n-1.25\n3.5\na\nz\n")

    def test_option_and_result_expose_success_predicates(self):
        run = _run_source(
            "fun main() { valid := String(\"42\").try_to_i32() invalid := String(\"x\").try_to_i32() echo valid.is_some() echo valid.is_none() echo invalid.is_some() echo invalid.is_none() result := read_file_result(\"missing-api-test-file\") echo result.is_ok() echo result.is_err() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "true\nfalse\nfalse\ntrue\nfalse\ntrue\n")

    def test_scalar_option_unwrap_or_preserves_typed_payload_and_eager_default_evaluation(self):
        run = _run_source(
            "fun fallback() -> i32 { echo \"fallback\" return -1 } fun main() { valid := String(\"42\").try_to_i32() invalid := String(\"nope\").try_to_i32() echo valid.unwrap_or(fallback()) echo invalid.unwrap_or(fallback()) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "fallback\n42\nfallback\n-1\n")

    def test_option_unwrap_returns_scalar_and_string_payloads_and_errors_on_none(self):
        run = _run_source(
            "fun main() { number := String(\"42\").try_to_i32().unwrap() checked := String(\"43\").try_to_i32().expect(\"number should parse\") echo number echo checked mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.pop() word := selected.unwrap() echo word echo selected.unwrap() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "42\n43\nzażółć\nzażółć\n")

        run = _run_source(
            "fun main() { value := String(\"invalid\").try_to_i32().unwrap() echo value }",
        )
        self.assertEqual(run.code, 1)
        self.assertIn("called Option.unwrap() on None", run.stderr)

        run = _run_source(
            "fun main() { value := String(\"invalid\").try_to_i32().expect(\"custom missing value message\") echo value }",
        )
        self.assertEqual(run.code, 1)
        self.assertIn("custom missing value message", run.stderr)

    def test_result_unwrap_and_unwrap_err_extract_scalar_and_string_payloads(self):
        run = _run_source(
            "fun main() { success: Result<i32, String> = Result::Ok(42) failure: Result<i32, String> = Result::Err(String(\"bad\")) echo success.unwrap() echo failure.unwrap_err() echo success.unwrap_err() }",
        )
        self.assertNotEqual(run.code, 0)
        self.assertEqual(run.stdout, "42\nbad\n")
        self.assertIn("Result extraction", run.stderr)

    def test_result_expect_reports_custom_failure_message(self):
        run = _run_source(
            "fun main() { result: Result<i32, String> = Result::Err(String(\"bad\")) result.expect(\"parse failed\") }",
        )
        self.assertNotEqual(run.code, 0)
        self.assertEqual(run.stdout, "")
        self.assertIn("parse failed", run.stderr)

    def test_result_expect_extracts_the_ok_payload(self):
        run = _run_source(
            "fun main() { result: Result<i32, String> = Result::Ok(7) echo result.expect(\"expected value\") }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "7\n")

    def test_process_builder_configures_and_waits_for_a_child_process(self):
        cwd_raw = tempfile.gettempdir().rstrip("\\/")
        cwd = cwd_raw.replace("\\", "\\\\").replace('"', '\\"')
        if IS_WINDOWS:
            command = f'if /I "%CD%"=="{cwd_raw}" if "%RYN_PROCESS_API_TEST%"=="present" exit 23 else exit 1'
            command = command.replace("\\", "\\\\").replace('"', '\\"')
            source = (
                "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"/c\")) "
                f"arguments.push(String(\"{command}\")) mut child := process::Process::new(\"cmd.exe\") "
                "assert(child.args(arguments)) assert(child.env(\"RYN_PROCESS_API_TEST\", \"present\")) "
                f"assert(child.cwd(\"{cwd}\")) assert(child.spawn()) echo child.wait() }}"
            )
        else:
            command = f'test "$PWD" = "{cwd_raw}" && test "$RYN_PROCESS_API_TEST" = present && test "$1" = argument && exit 23'
            command = command.replace("\\", "\\\\").replace('"', '\\"')
            source = (
                "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"-c\")) "
                f"arguments.push(String(\"{command}\")) arguments.push(String(\"ryn\")) "
                "arguments.push(String(\"argument\")) mut child := process::Process::new(\"sh\") "
                "assert(child.args(arguments)) assert(child.env(\"RYN_PROCESS_API_TEST\", \"present\")) "
                f"assert(child.cwd(\"{cwd}\")) assert(child.spawn()) echo child.wait() }}"
            )
        run = _run_project(source)
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "23\n")

    def test_process_kill_stops_a_running_child(self):
        if IS_WINDOWS:
            source = "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"-n\")) arguments.push(String(\"30\")) arguments.push(String(\"127.0.0.1\")) mut child := process::Process::new(\"ping.exe\") assert(child.args(arguments)) assert(child.spawn()) assert(child.kill()) child.wait() echo \"killed\" }"
        else:
            source = "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"30\")) mut child := process::Process::new(\"sleep\") assert(child.args(arguments)) assert(child.spawn()) assert(child.kill()) child.wait() echo \"killed\" }"
        run = _run_project(source)
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "killed\n")

    def test_process_run_capture_returns_exit_code_and_owned_stdout_stderr(self):
        if IS_WINDOWS:
            program, args = "cmd.exe", ["/c", "echo capture-out & echo capture-err 1>&2 & exit /b 7"]
        else:
            program, args = "sh", ["-c", "printf capture-out; printf capture-err >&2; exit 7"]
        expected_out, expected_err = "capture-out", "capture-err"
        argument_setup = " ".join(f'arguments.push(String("{argument}"))' for argument in args)
        source = (
            "use std::process\nfun main() { mut arguments := Vec<String>() "
            f"{argument_setup} result := process::Process::run_capture_args(\"{program}\", arguments) "
            "echo result.exit_code "
            f"echo result.stdout.contains(\"{expected_out}\") "
            f"echo result.stderr.contains(\"{expected_err}\") }}"
        )
        run = _run_project(source)
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "7\ntrue\ntrue\n")

    def test_process_run_capture_without_arguments_returns_an_owned_result(self):
        run = _run_project(
            "use std::process\nfun main() { result := process::Process::run_capture(\"whoami\") assert(result.exit_code == 0) assert(!result.stdout.is_empty()) assert(result.stderr.is_empty()) echo \"captured\" }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "captured\n")

    def test_tcp_client_connects_writes_and_reads_on_loopback(self):
        listener = socket.create_server(("127.0.0.1", 0))

        def peer() -> None:
            # A client that never connects fails this test instead of blocking the run.
            with listener:
                listener.settimeout(PEER_TIMEOUT)
                stream, _ = listener.accept()
                with stream:
                    stream.settimeout(PEER_TIMEOUT)
                    request = _recv_exact(stream, 7)
                    if request != b"request":
                        raise AssertionError(f"unexpected request {request!r}")
                    stream.sendall(b"response")
                    stream.shutdown(socket.SHUT_WR)

        address = listener.getsockname()
        peer_thread = _Peer(peer).start()
        source = (
            f"use std::net\nfun main() {{ mut stream := net::TcpStream::connect(\"{address[0]}:{address[1]}\") "
            "assert(stream.write(\"request\")) echo stream.read() }"
        )
        run = _run_project(source)
        peer_thread.join()
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "response\n")

    def test_tcp_listener_accepts_and_reads_a_loopback_client_and_dns_resolves_localhost(self):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as reservation:
            reservation.bind(("127.0.0.1", 0))
            address = reservation.getsockname()
        # the reservation is released here, so the Ryn listener can bind the same port

        def peer() -> None:
            stream = None
            for _ in range(250):
                candidate = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                try:
                    candidate.connect(address)
                    stream = candidate
                    break
                except OSError:
                    candidate.close()
                    time.sleep(0.02)
            if stream is None:
                raise AssertionError("Ryn listener accepts client")
            # A listener that never answers fails this test instead of blocking the run.
            with stream:
                stream.settimeout(PEER_TIMEOUT)
                stream.sendall(b"from-peer")
                stream.shutdown(socket.SHUT_WR)
                response = _recv_exact(stream, 3)
                if response != b"ack":
                    raise AssertionError(f"unexpected response {response!r}")

        peer_thread = _Peer(peer).start()
        source = (
            f"use std::net\nfun main() {{ mut listener := net::TcpListener::bind(\"{address[0]}:{address[1]}\") "
            "mut stream := listener.accept() request := stream.read() assert(request.contains(\"from-peer\")) "
            "assert(stream.write(\"ack\")) addresses := net::Dns::resolve(\"localhost\") "
            "assert(!addresses.is_empty()) echo \"accepted\" }"
        )
        run = _run_project(source)
        peer_thread.join()
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "accepted\n")

    def test_udp_socket_sends_and_receives_loopback_datagrams(self):
        server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        server.bind(("127.0.0.1", 0))
        address = server.getsockname()

        def peer() -> None:
            # A program that never sends fails this test instead of blocking the run.
            with server:
                server.settimeout(PEER_TIMEOUT)
                request, client = server.recvfrom(32)
                if request != b"udp-request":
                    raise AssertionError(f"unexpected request {request!r}")
                server.sendto(b"udp-response", client)

        peer_thread = _Peer(peer).start()
        source = (
            f"use std::net\nfun main() {{ mut socket := net::UdpSocket::bind(\"127.0.0.1:0\") "
            f"assert(socket.connect(\"{address[0]}:{address[1]}\")) assert(socket.write(\"udp-request\")) "
            "echo socket.read() assert(socket.close()) }"
        )
        run = _run_project(source)
        peer_thread.join()
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "udp-response\n")

    def test_vector_contains_uses_typed_scalar_and_string_equality(self):
        run = _run_source(
            "fun main() { mut numbers := Vec<i32>() numbers.push(-7) numbers.push(42) echo numbers.contains(42) echo numbers.contains(1) mut words := Vec<String>() words.push(String(\"zażółć\")) echo words.contains(String(\"zażółć\")) echo words.contains(String(\"żółć\")) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "true\nfalse\ntrue\nfalse\n")

    def test_vector_get_first_and_last_return_owned_options(self):
        run = _run_source(
            "fun main() { mut numbers := Vec<i32>() numbers.push(7) numbers.push(12) echo numbers.get(1).unwrap_or(-1) echo numbers.get(2).unwrap_or(-1) echo numbers.first().unwrap_or(-1) echo numbers.last().unwrap_or(-1) mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.get(0) words.clear() echo selected.is_some() echo selected.unwrap_or(String(\"fallback\")) missing := words.get(0) echo missing.unwrap_or(String(\"empty\")) echo words.last().is_none() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "12\n-1\n7\n12\ntrue\nzażółć\nempty\ntrue\n")

    def test_vector_pop_returns_owned_option_and_transfers_string_elements(self):
        run = _run_source(
            "fun main() { mut values := Vec<i32>() values.push(7) values.push(12) echo values.pop().unwrap_or(-1) echo values.pop().unwrap_or(-1) echo values.pop().unwrap_or(-1) mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.pop() echo words.is_empty() echo selected.unwrap_or(String(\"fallback\")) empty := words.pop() echo empty.unwrap_or(String(\"empty\")) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "12\n7\n-1\ntrue\nzażółć\nempty\n")

    def test_vector_sort_orders_owned_strings_by_utf8_content_without_losing_ownership(self):
        run = _run_source(
            "fun main() { mut words := Vec<String>() words.push(String(\"żaba\")) words.push(String(\"kot\")) words.push(String(\"zażółć\")) words.sort() echo words.take(0) echo words.take(0) echo words.take(0) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "kot\nzażółć\nżaba\n")

    def test_sleep_accepts_duration_suffixes(self):
        run = _run_source("fun main() { sleep(1ms) sleep(1ms) sleep(2s) }")
        self.assertSucceeded(run)
        with TempDir("input") as directory:
            source = _write(directory / "main.ryn", "fun main() { sleep(3min) sleep(1h) }")
            run = _run("check", source)
        self.assertSucceeded(run)

    def test_standard_time_module_exposes_wall_clock_and_monotonic_elapsed_time(self):
        run = _run_project(
            "use std::time\nfun main() { assert(time::unix() > 0.0) start := time::instant_now() time::sleep(1ms) assert(time::instant_elapsed(start) >= 0.001) assert(time::monotonic() >= start) instant := time::Instant::now() time::sleep(1ms) assert(instant.elapsed() >= 0.001) current := time::Time::now() assert(current.unix_seconds > 0.0) assert(time::Time::unix() > 0.0) }",
            name="std-time-smoke",
        )
        self.assertSucceeded(run)

    def test_standard_filesystem_and_path_modules_operate_on_paths(self):
        run = _run_project(
            "use std::fs\nuse std::path\nfun main() { assert(fs::create_dir_all(\"build/files\")) assert(fs::write(\"build/files/one.txt\", \"hello\")) assert(fs::File::write(\"build/files/static.txt\", \"static-data\")) assert(fs::File::read_text(\"build/files/static.txt\").contains(\"static-data\")) echo fs::File::read(\"build/files/static.txt\") value := path::new(\"build/files/one.txt\") associated := path::Path::new(\"build/files/one.txt\") assert(associated.exists()) assert(value.exists()) assert(value.is_file()) echo value.filename() echo value.extension() absolute := value.absolute() assert(!absolute.raw.is_empty()) assert(absolute.is_absolute()) canonical := value.canonical() assert(!canonical.raw.is_empty()) assert(fs::copy_file(\"build/files/one.txt\", \"build/files/two.txt\")) assert(fs::rename(\"build/files/two.txt\", \"build/files/three.txt\")) assert(fs::remove_file(\"build/files/one.txt\")) assert(fs::remove_file(\"build/files/three.txt\")) assert(fs::remove_file(\"build/files/static.txt\")) mut writer := fs::File::create(\"build/files/stream.txt\") assert(writer.write(\"first\\r\\n\")) assert(writer.write(\"second\\n\")) assert(writer.flush()) assert(writer.close()) assert(fs::File::append(\"build/files/stream.txt\", \"third\\r\\n\")) mut reader := fs::File::open(\"build/files/stream.txt\") echo reader.read_line() echo reader.read_line() echo reader.read_line() echo reader.read_line() assert(reader.close()) assert(fs::remove_file(\"build/files/stream.txt\")) assert(fs::remove_dir(\"build/files\")) }",
            name="std-fs-smoke",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "static-data\none.txt\ntxt\nfirst\nsecond\nthird\n\n")

    def test_standard_file_handles_read_write_and_close_like_buffered_files(self):
        source = (
            "use std::fs\n"
            "fun main() {\n"
            "    assert(fs::create_dir_all(\"files\"))\n"
            "    mut writer := fs::File::create(\"files/a.txt\")\n"
            "    assert(writer.write(\"line one\\r\\nline two\\n\"))\n"
            "    assert(writer.write(\"tail\"))\n"
            "    assert(writer.flush())\n"
            "    assert(writer.close())\n"
            "    echo writer.write(\"after close\")\n"
            "    echo writer.flush()\n"
            "    echo writer.close()\n"
            "    mut reader := fs::File::open(\"files/a.txt\")\n"
            "    echo reader.read_line()\n"
            "    echo reader.read_line()\n"
            "    echo reader.read_line()\n"
            "    echo reader.read_line().len()\n"
            "    echo reader.write(\"x\")\n"
            "    echo reader.read().len()\n"
            "    assert(reader.close())\n"
            "    mut again := fs::File::open(\"files/a.txt\")\n"
            "    echo again.read().len()\n"
            "    echo again.read().len()\n"
            "    assert(again.close())\n"
            "    mut bad := fs::File::open(\"files/bad.txt\")\n"
            "    echo bad.read_line().len()\n"
            "    echo bad.read_line()\n"
            "    echo bad.read_line().len()\n"
            "    echo bad.read_line()\n"
            "    assert(bad.close())\n"
            "    mut bytes := fs::File::open(\"files/bad.txt\")\n"
            "    echo bytes.read().len()\n"
            "    assert(bytes.close())\n"
            "    mut stream := fs::File::create(\"files/line.txt\")\n"
            "    assert(stream.write(\"x\"))\n"
            "    assert(stream.close())\n"
            "    mut single := fs::File::open(\"files/line.txt\")\n"
            "    echo single.read_line().len()\n"
            "    echo single.read_line().len()\n"
            "    assert(single.close())\n"
            "    assert(fs::remove_file(\"files/a.txt\"))\n"
            "    assert(fs::remove_file(\"files/bad.txt\"))\n"
            "    assert(fs::remove_file(\"files/line.txt\"))\n"
            "    assert(fs::remove_dir(\"files\"))\n"
            "    echo \"done\"\n"
            "}\n"
        )
        run = _run_file_project(source, name="file-handle-test")
        self.assertSucceeded(run)
        self.assertEqual(
            run.stdout,
            "false\nfalse\nfalse\nline one\nline two\ntail\n0\nfalse\n0\n23\n0\n0\nok line\n2\n\n0\n1\n0\ndone\n",
        )

    def test_standard_read_file_results_report_text_and_failure_codes(self):
        big = "é" * 5000
        source = (
            "fun main() {\n"
            "    echo read_file_result(\"files/small.txt\").unwrap().len()\n"
            "    echo read_file_result(\"files/empty.txt\").unwrap().len()\n"
            "    echo read_file_result(\"files/big.txt\").unwrap().len()\n"
            "    echo read_file_result(\"files/bad.txt\").unwrap_err()\n"
            "    echo try_read_file(\"files/bad.txt\").is_none()\n"
            "    echo try_read_file(\"files/big.txt\").unwrap().len()\n"
            "}\n"
        )
        run = _run_file_project(
            source,
            name="read-file-results-test",
            extra={
                "files/small.txt": b"ok text\n",
                "files/empty.txt": b"",
                "files/big.txt": big.encode("utf-8"),
            },
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "8\n0\n10000\n3\ntrue\n10000\n")

    def test_standard_file_open_of_a_missing_file_stops_with_the_operating_system_error(self):
        run = _run_file_project(
            "use std::fs\nfun main() {\n    mut missing := fs::File::open(\"missing.txt\")\n    echo \"unreachable\"\n}\n",
            name="file-open-test",
        )
        self.assertEqual(run.code, 1)
        self.assertEqual(run.stdout, "")
        self.assertIn(
            "Ryn runtime error: could not open `missing.txt`: No such file or directory (os error 2)",
            run.stderr,
        )

    def test_standard_thread_module_spawns_and_joins_a_worker(self):
        run = _run_project(
            "use std::thread\ntype Worker = extern \"C\" fun()\nfun worker() { echo \"worker done\" }\nfun main() { mut background := thread::Thread::spawn(worker as Worker) id := background.id() assert(id > 0 as u64) assert(background.join()) echo \"main done\" }",
            name="std-thread-smoke",
        )
        self.assertSucceeded(run)
        self.assertIn("worker done\n", run.stdout)
        self.assertTrue(run.stdout.endswith("main done\n"), run.stdout)

    def test_standard_keyboard_module_exposes_state_queries_and_read_key_type(self):
        run = _run_project(
            "use std::keyboard\nfun read_once() -> Key { return keyboard::read_key() }\nfun main() { moving := keyboard::key_down(Key::W) jump := keyboard::key_pressed(Key::Space) released := keyboard::key_released(Key::W) echo \"keyboard api ready\" }",
            name="std-keyboard-smoke",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "keyboard api ready\n")

    def test_result_unwrap_or_returns_ok_value_or_evaluates_err_fallback(self):
        run = _run_source(
            "fun fallback() -> i32 { echo \"fallback\" return -1 } fun main() { good: Result<i32, String> = Result::Ok(42) bad: Result<i32, String> = Result::Err(String(\"no\")) echo good.unwrap_or(fallback()) echo bad.unwrap_or(fallback()) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "fallback\n42\nfallback\n-1\n")

    def test_standard_environment_path_and_file_wrappers_match_listed_names(self):
        run = _run_project(
            "use std::env\nuse std::fs\nuse std::path\nfun main() { assert(set_env(\"RYN_API_TEST\", \"ok\")) echo env::env(\"RYN_API_TEST\") assert(remove_env(\"RYN_API_TEST\")) assert(fs::create_dir_all(\"build/files\")) assert(fs::write(\"build/files/a.txt\", \"contents\")) echo fs::read_file(\"build/files/a.txt\") assert(path::exists(\"build/files/a.txt\")) assert(path::is_file(\"build/files/a.txt\")) p := path::from(\"build/files/a.txt\") assert(p.exists()) assert(p.is_file()) assert(fs::remove_file(\"build/files/a.txt\")) assert(fs::remove_dir(\"build/files\")) }",
            name="std-api-name-test",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "ok\ncontents\n")

    def test_system_assertions_and_panic_stop_the_program_with_failure(self):
        cases = [
            ("fun main() { assert(false) echo \"continued\" }", "assertion failed"),
            (
                "fun main() { assert_message(false, \"broken invariant\") echo \"continued\" }",
                "broken invariant",
            ),
            (
                "use std::system\nfun main() { system::panic(\"stop now\") echo \"continued\" }",
                "stop now",
            ),
        ]
        for source, message in cases:
            with self.subTest(message=message):
                run = _run_source(source, b"")
                self.assertNotEqual(run.code, 0, f"source unexpectedly succeeded: {source}")
                self.assertIn(message, run.stderr)
                self.assertNotIn("continued", run.stdout, f"program continued after failure: {run.stdout}")

    def test_standard_option_and_result_map_preserve_variants_and_transform_payloads(self):
        run = _run_project(
            r"""use std::option
use std::result
type Transform = fun(i32) -> i32
fun twice(value: i32) -> i32 { return value * 2 }
fun plus_one(value: i32) -> i32 { return value + 1 }
fun suffix(value: String) -> String { echo "called" return value.concat("!") }
fun main() {
    callback: Transform = twice
    callback_plus: fun(i32) -> i32 = plus_one
    text_callback: fun(String) -> String = suffix
    some: Option<i32> = Option::Some(6)
    none: Option<i32> = Option::None
    mapped := some.map(callback)
    empty := none.map(callback)
    echo mapped.unwrap_or(0 as i32)
    echo empty.is_none()
    good: Result<i32, String> = Result::Ok(8)
    bad: Result<i32, String> = Result::Err(String("kept"))
    changed := good.map(callback)
    kept := bad.map(callback)
    echo changed.unwrap_or(0 as i32)
    echo kept.unwrap_err()
    typed: Result<i32, i32> = Result::Err(5)
    mapped_error := typed.map_err(callback_plus)
    echo mapped_error.unwrap_err()
    text: Option<String> = Option::Some(String("owned"))
    text_result := text.map(text_callback)
    echo text_result.unwrap_or(String("empty"))
    text_error: Result<i32, String> = Result::Err(String("error"))
    changed_error := text_error.map_err(text_callback)
    echo changed_error.unwrap_err()
}""",
        )
        self.assertSucceeded(run)
        self.assertEqual(
            run.stdout,
            "12\ntrue\n16\nkept\n6\ncalled\nowned!\ncalled\nerror!\n",
        )

    def test_option_map_moves_nested_owned_values_through_callback_without_aliasing(self):
        run = _run_project(
            "use std::option\nfun add_tail(values: Vec<String>) -> Vec<String> { echo values.len() mut result := values result.push(String(\"🦀\")) return result }\nfun main() { callback: fun(Vec<String>) -> Vec<String> = add_tail mut source := Vec<String>() source.push(String(\"你好\")) wrapped: Option<Vec<String>> = Option::Some(source) mapped := wrapped.map(callback) values := mapped.unwrap() echo values.len() echo values.get(0).unwrap_or(String(\"missing\")) echo values.get(1).unwrap_or(String(\"missing\")) expected: Result<Vec<String>, String> = Result::Ok(values) extracted := expected.expect(\"has vector\") echo extracted.len() fallback_source: Result<Vec<String>, String> = Result::Err(String(\"no vector\")) fallback := fallback_source.unwrap_or(Vec<String>()) echo fallback.len() error_source: Result<String, Vec<String>> = Result::Err(extracted) error_values := error_source.unwrap_err() echo error_values.len() }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "1\n2\n你好\n🦀\n2\n0\n2\n")

    def test_option_and_result_extract_struct_and_array_payloads_without_aliasing(self):
        run = _run_source(
            "struct Person { name: String, visits: i32 } fun main() { person := Person { name: String(\"Krystian\"), visits: 4 } maybe: Option<Person> = Option::Some(person) mut copy := maybe.unwrap() copy.name.append(\"!\") echo copy.name echo maybe.expect(\"person\").name fallback: Person = Person { name: String(\"fallback\"), visits: 0 } chosen := maybe.unwrap_or(fallback) echo chosen.name result: Result<[String; 2], String> = Result::Ok([String(\"zażółć\"), String(\"你好\")]) array := result.expect(\"array\") echo array[0] echo array[1] error: Result<String, [String; 2]> = Result::Err([String(\"first\"), String(\"🦀\")]) error_array := error.unwrap_err() echo error_array[0] echo error_array[1] }",
        )
        self.assertSucceeded(run)
        self.assertEqual(
            run.stdout,
            "Krystian!\nKrystian\nKrystian\nzażółć\n你好\nfirst\n🦀\n",
        )

    def test_environment_exports_the_documented_set_env_name_and_paths_join_components(self):
        run = _run_project(
            "use std::env\nuse std::path\nuse std::fs\nfun main() { assert(env::set_env(\"RYN_API_NAMED_SET\", \"ok\")) echo env::get(\"RYN_API_NAMED_SET\") assert(env::remove(\"RYN_API_NAMED_SET\")) assert(fs::create_dir_all(\"build/path-api\")) assert(fs::write(\"build/path-api/file.txt\", \"x\")) base := path::new(\"build/path-api\") joined := base.join(\"file.txt\") assert(joined.exists()) assert(joined.is_file()) assert(joined.is_directory() == false) parent := joined.parent() assert(parent.is_dir()) echo joined.filename() echo joined.extension() assert(fs::remove_file(\"build/path-api/file.txt\")) assert(fs::remove_dir(\"build/path-api\")) }",
        )
        self.assertSucceeded(run)
        self.assertEqual(run.stdout, "ok\nfile.txt\ntxt\n")

    def test_filesystem_create_dir_is_non_recursive_and_create_dir_all_is_recursive(self):
        run = _run_project(
            "use std::fs\nfun main() { assert(fs::create_dir(\"build/one\")) assert(!fs::create_dir(\"build/two/child\")) assert(fs::create_dir_all(\"build/two/child\")) assert(fs::remove_dir(\"build/one\")) assert(fs::remove_dir_all(\"build/two\")) }",
        )
        self.assertSucceeded(run)


if __name__ == "__main__":
    unittest.main()
