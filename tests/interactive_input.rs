use std::{
    fs,
    io::{BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream, UdpSocket},
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

fn source_file(source_text: &str) -> (PathBuf, PathBuf) {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!("ryn-input-{}-{id}", std::process::id()));
    fs::create_dir_all(&directory).expect("temporary project directory is created");
    let source = directory.join("main.ryn");
    fs::write(&source, source_text).expect("Ryn source is written");
    (directory, source)
}

fn run_with_stdin(source_text: &str, input: &[u8]) -> std::process::Output {
    let (directory, source) = source_file(source_text);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Ryn process starts");
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().expect("Ryn process completes");
    let _ = fs::remove_dir_all(directory);
    output
}

fn run_project_with_stdin(source_text: &str, input: &[u8]) -> std::process::Output {
    let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("ryn-project-input-{}-{id}", std::process::id()));
    fs::create_dir_all(directory.join("src"))
        .expect("temporary project source directory is created");
    fs::write(
        directory.join("ryn.yaml"),
        "name: input-api-test\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .expect("temporary project manifest is written");
    fs::write(directory.join("src/main.ryn"), source_text)
        .expect("temporary project source is written");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Ryn project process starts");
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child
        .wait_with_output()
        .expect("Ryn project process completes");
    let _ = fs::remove_dir_all(directory);
    output
}

#[test]
fn line_input_handles_lf_crlf_utf8_blank_lines_sequential_reads_and_eof() {
    let output = run_with_stdin(
        "fun main() { first := read_line() second := stdin_read_line() third := ask(\"Q: \") echo \"{first}|{second}|{third}\" }",
        "zażółć gęślą jaźń\r\n你好🦀\n\n".as_bytes(),
    );
    assert!(
        output.status.success(),
        "exit: {:?}, stdout: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Q: zażółć gęślą jaźń|你好🦀|\n"
    );

    let output = run_with_stdin(
        "fun main() { value := read_line() echo \"<{value}>\" }",
        b"",
    );
    assert!(
        output.status.success(),
        "EOF should return an empty String: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "<>\n");
}

#[test]
fn ask_flushes_prompt_before_waiting_and_stdin_read_still_reads_to_eof() {
    let (directory, source) =
        source_file("fun main() { name := ask(\"Name: \") echo \"Hello {name}!\" }");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Ryn process starts");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut prompt = [0u8; 6];
    stdout
        .read_exact(&mut prompt)
        .expect("flushed prompt is readable before input");
    assert_eq!(&prompt, b"Name: ");
    stdin.write_all("v3nn7\r\n".as_bytes()).unwrap();
    drop(stdin);
    let mut remainder = String::new();
    stdout.read_to_string(&mut remainder).unwrap();
    let status = child.wait().unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(status.success());
    assert_eq!(remainder, "Hello v3nn7!\n");

    let output = run_with_stdin("fun main() { data := stdin_read() echo data }", b"a\nb\n");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a\nb\n\n");
}

#[test]
fn standard_io_module_exposes_the_console_api() {
    let directory = std::env::temp_dir().join(format!("ryn-std-io-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-io-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::io\nuse std::env\nuse std::system\nfun main() { io::print(\"Name: \") io::stdout_flush() name := io::read_line() io::println(\"Hello!\") io::eprintln(\"done\") assert(!env::current_dir().is_empty()) assert(!system::os().is_empty()) assert(system::cpu_count() > 0) }",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("你好\n".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "exit: {:?}, stdout: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Name: Hello!\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "done\n");
}

#[test]
fn standard_math_module_exposes_f64_operations_and_random_ranges() {
    let directory = std::env::temp_dir().join(format!("ryn-math-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-math-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::math\nfun main() { echo math::sqrt(9.0) echo math::pow(2.0, 3.0) value := math::random_range(2.0, 4.0) assert(value >= 2.0) assert(value < 4.0) }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n8\n");
}

#[test]
fn string_methods_cover_unicode_text_and_line_operations() {
    let output = run_with_stdin(
        "fun main() { value := String(\"  HéLLo\\r\\nRyn  \") echo value.trim_start() echo value.trim_end() echo value.to_lower() echo value.to_upper() echo value.replace(\"Ryn\", \"Lang\") echo value.reverse() echo value.substring(2, 7) echo value.is_empty() lines := value.lines() echo lines.len() chars := value.chars() echo chars.len() bytes := value.bytes() echo bytes.len() echo String(\"ha\").repeat(3) }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "HéLLo\r\nRyn  \n  HéLLo\r\nRyn\n  héllo\r\nryn  \n  HÉLLO\r\nRYN  \n  HéLLo\r\nLang  \n  nyR\n\roLLéH  \nHéLLo\nfalse\n2\n14\n15\nhahaha\n"
    );
}

#[test]
fn string_parse_and_try_parse_accept_explicit_numeric_types() {
    let output = run_with_stdin(
        "fun main() { value := String(\"42\") echo value.parse::<i32>() echo value.try_parse::<u64>().unwrap_or(0) invalid := String(\"bad\").try_parse::<i32>() echo invalid.is_none() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n42\ntrue\n");
}

#[test]
fn string_numeric_conversions_cover_all_signed_unsigned_and_float_widths() {
    let output = run_with_stdin(
        "fun main() { integer := String(\"42\") echo integer.to_i8() echo integer.to_i16() echo integer.to_i32() echo integer.to_i64() echo integer.to_u8() echo integer.to_u16() echo integer.to_u32() echo integer.to_u64() decimal := String(\"3.5\") echo decimal.to_f32() echo decimal.to_f64() echo (42 as i8).to_string() echo (42 as i16).to_string() echo (42 as i32).to_string() echo (42 as i64).to_string() echo (42 as u8).to_string() echo (42 as u16).to_string() echo (42 as u32).to_string() echo (42 as u64).to_string() echo (3.5 as f32).to_string() echo (3.5 as f64).to_string() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "42\n42\n42\n42\n42\n42\n42\n42\n3.5\n3.5\n42\n42\n42\n42\n42\n42\n42\n42\n3.5\n3.5\n"
    );
}

#[test]
fn vector_and_map_expose_is_empty() {
    let output = run_with_stdin(
        "fun main() { mut values := Vec<i32>() echo values.is_empty() values.push(1) echo values.is_empty() mut names := Map<String, i32>() echo names.is_empty() names.insert(String(\"x\"), 1) echo names.is_empty() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "true\nfalse\ntrue\nfalse\n"
    );
}

#[test]
fn set_uses_map_storage_with_set_semantics_and_owned_string_keys() {
    let output = run_with_stdin(
        "fun main() { mut values := Set<String>() assert(values.is_empty()) assert(!values.contains(String(\"ry\"))) assert(!values.insert(String(\"ry\"))) assert(!values.is_empty()) assert(values.contains(String(\"ry\"))) assert(values.insert(String(\"ry\"))) echo values.len() assert(values.remove(String(\"ry\"))) assert(values.is_empty()) values.insert(String(\"owned\")) values.clear() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n");
}

#[test]
fn map_get_returns_owned_option_and_keys_values_clone_without_aliasing() {
    let output = run_with_stdin(
        "fun main() { mut values := Map<String, String>() values.insert(String(\"first\"), String(\"zażółć\")) selected := values.get(String(\"first\")) missing := values.get(String(\"missing\")) assert(selected.is_some()) assert(missing.is_none()) assert(values.remove(String(\"first\"))) echo selected.unwrap_or(String(\"fallback\")) echo missing.unwrap_or(String(\"empty\")) mut other := Map<String, String>() other.insert(String(\"a\"), String(\"one\")) other.insert(String(\"b\"), String(\"two\")) mut keys := other.keys() mut values_list := other.values() other.clear() echo keys.len() echo values_list.len() echo keys.take(0) echo values_list.take(0) echo keys.take(0) echo values_list.take(0) }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "zażółć\nempty\n2\n2\na\none\nb\ntwo\n"
    );
}

#[test]
fn generic_helper_accepts_and_returns_specialized_option_types() {
    let output = run_with_stdin(
        "fun with_fallback<T>(value: Option<T>, fallback: T) -> T { return value.unwrap_or(fallback) } fun main() { some: Option<i32> = Option::Some(7) none: Option<i32> = Option::None echo with_fallback(some, 2 as i32) echo with_fallback(none, 2 as i32) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n2\n");
}

#[test]
fn vec_rejects_sets_until_set_clone_drop_callbacks_are_supported() {
    let (directory, source) = source_file(
        "fun main() { mut set := Set<String>() mut sets := Vec<Set<String>>() sets.push(set) }",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("is not supported yet"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generic_helper_accepts_specialized_result_types_and_unifies_ok_and_error_payloads() {
    let output = run_with_stdin(
        "fun value_or<T, E>(value: Result<T, E>, fallback: T) -> T { return value.unwrap_or(fallback) } fun main() { good: Result<i32, String> = Result::Ok(9) bad: Result<i32, String> = Result::Err(String(\"bad\")) echo value_or(good, 3 as i32) echo value_or(bad, 3 as i32) }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "9\n3\n");
}
#[test]
fn vector_insert_remove_and_reverse_keep_element_order_and_ownership() {
    let output = run_with_stdin(
        "fun main() { mut values := Vec<i32>() values.push(1) values.push(3) values.insert(1, 2) values.reverse() echo values.index(0) echo values.remove(1) echo values.index(1) mut words := Vec<String>() words.insert(0, String(\"first\")) words.insert(1, String(\"second\")) words.reverse() echo words.take(0) words.clear() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n2\n1\nsecond\n");
}

#[test]
fn vector_sort_orders_signed_unsigned_float_and_char_values() {
    let output = run_with_stdin(
        "fun main() { mut values := Vec<i32>() values.push(5) values.push(-2) values.push(9) values.sort() echo values.index(0) echo values.index(1) echo values.index(2) mut unsigned := Vec<u32>() unsigned.push(4000000000) unsigned.push(2) unsigned.sort() echo unsigned.index(0) echo unsigned.index(1) mut floats := Vec<f64>() floats.push(3.5) floats.push(-1.25) floats.sort() echo floats.index(0) echo floats.index(1) mut chars := Vec<char>() chars.push('z') chars.push('a') chars.sort() echo chars.index(0) echo chars.index(1) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "-2\n5\n9\n2\n4000000000\n-1.25\n3.5\na\nz\n"
    );
}

#[test]
fn option_and_result_expose_success_predicates() {
    let output = run_with_stdin(
        "fun main() { valid := String(\"42\").try_to_i32() invalid := String(\"x\").try_to_i32() echo valid.is_some() echo valid.is_none() echo invalid.is_some() echo invalid.is_none() result := read_file_result(\"missing-api-test-file\") echo result.is_ok() echo result.is_err() }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "true\nfalse\nfalse\ntrue\nfalse\ntrue\n"
    );
}

#[test]
fn scalar_option_unwrap_or_preserves_typed_payload_and_eager_default_evaluation() {
    let output = run_with_stdin(
        "fun fallback() -> i32 { echo \"fallback\" return -1 } fun main() { valid := String(\"42\").try_to_i32() invalid := String(\"nope\").try_to_i32() echo valid.unwrap_or(fallback()) echo invalid.unwrap_or(fallback()) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "fallback\n42\nfallback\n-1\n"
    );
}

#[test]
fn option_unwrap_returns_scalar_and_string_payloads_and_errors_on_none() {
    let output = run_with_stdin(
        "fun main() { number := String(\"42\").try_to_i32().unwrap() checked := String(\"43\").try_to_i32().expect(\"number should parse\") echo number echo checked mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.pop() word := selected.unwrap() echo word echo selected.unwrap() }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "42\n43\nzażółć\nzażółć\n"
    );

    let output = run_with_stdin(
        "fun main() { value := String(\"invalid\").try_to_i32().unwrap() echo value }",
        b"",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("called Option.unwrap() on None"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = run_with_stdin(
        "fun main() { value := String(\"invalid\").try_to_i32().expect(\"custom missing value message\") echo value }",
        b"",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("custom missing value message"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn result_unwrap_and_unwrap_err_extract_scalar_and_string_payloads() {
    let output = run_with_stdin(
        "fun main() { success: Result<i32, String> = Result::Ok(42) failure: Result<i32, String> = Result::Err(String(\"bad\")) echo success.unwrap() echo failure.unwrap_err() echo success.unwrap_err() }",
        b"",
    );
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\nbad\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Result extraction"));
}

#[test]
fn result_expect_reports_custom_failure_message() {
    let output = run_with_stdin(
        "fun main() { result: Result<i32, String> = Result::Err(String(\"bad\")) result.expect(\"parse failed\") }",
        b"",
    );
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("parse failed"));
}

#[test]
fn result_expect_extracts_the_ok_payload() {
    let output = run_with_stdin(
        "fun main() { result: Result<i32, String> = Result::Ok(7) echo result.expect(\"expected value\") }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n");
}

#[test]
fn process_builder_configures_and_waits_for_a_child_process() {
    let cwd_raw = std::env::temp_dir()
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_owned();
    let cwd = cwd_raw.replace('\\', "\\\\").replace('"', "\\\"");
    let source = if cfg!(windows) {
        let command = format!(
            "if /I \"%CD%\"==\"{cwd_raw}\" if \"%RYN_PROCESS_API_TEST%\"==\"present\" exit 23 else exit 1"
        )
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
        format!(
            "use std::process\nfun main() {{ mut arguments := Vec<String>() arguments.push(String(\"/c\")) arguments.push(String(\"{command}\")) mut child := process::Process::new(\"cmd.exe\") assert(child.args(arguments)) assert(child.env(\"RYN_PROCESS_API_TEST\", \"present\")) assert(child.cwd(\"{cwd}\")) assert(child.spawn()) echo child.wait() }}"
        )
    } else {
        let command = format!(
            "test \"$PWD\" = \"{cwd_raw}\" && test \"$RYN_PROCESS_API_TEST\" = present && test \"$1\" = argument && exit 23"
        )
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
        format!(
            "use std::process\nfun main() {{ mut arguments := Vec<String>() arguments.push(String(\"-c\")) arguments.push(String(\"{command}\")) arguments.push(String(\"ryn\")) arguments.push(String(\"argument\")) mut child := process::Process::new(\"sh\") assert(child.args(arguments)) assert(child.env(\"RYN_PROCESS_API_TEST\", \"present\")) assert(child.cwd(\"{cwd}\")) assert(child.spawn()) echo child.wait() }}"
        )
    };
    let output = run_project_with_stdin(&source, b"");
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "23\n");
}

#[test]
fn process_kill_stops_a_running_child() {
    let source = if cfg!(windows) {
        "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"-n\")) arguments.push(String(\"30\")) arguments.push(String(\"127.0.0.1\")) mut child := process::Process::new(\"ping.exe\") assert(child.args(arguments)) assert(child.spawn()) assert(child.kill()) child.wait() echo \"killed\" }"
    } else {
        "use std::process\nfun main() { mut arguments := Vec<String>() arguments.push(String(\"30\")) mut child := process::Process::new(\"sleep\") assert(child.args(arguments)) assert(child.spawn()) assert(child.kill()) child.wait() echo \"killed\" }"
    };
    let output = run_project_with_stdin(source, b"");
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "killed\n");
}

#[test]
fn process_run_capture_returns_exit_code_and_owned_stdout_stderr() {
    #[cfg(windows)]
    let (program, args, expected_out, expected_err) = (
        "cmd.exe",
        vec!["/c", "echo capture-out & echo capture-err 1>&2 & exit /b 7"],
        "capture-out",
        "capture-err",
    );
    #[cfg(not(windows))]
    let (program, args, expected_out, expected_err) = (
        "sh",
        vec!["-c", "printf capture-out; printf capture-err >&2; exit 7"],
        "capture-out",
        "capture-err",
    );
    let argument_setup = args
        .iter()
        .map(|argument| format!("arguments.push(String(\"{argument}\"))"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!(
        "use std::process\nfun main() {{ mut arguments := Vec<String>() {argument_setup} result := process::Process::run_capture_args(\"{program}\", arguments) echo result.exit_code echo result.stdout.contains(\"{expected_out}\") echo result.stderr.contains(\"{expected_err}\") }}"
    );
    let output = run_project_with_stdin(&source, b"");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\ntrue\ntrue\n");
}

#[test]
fn process_run_capture_without_arguments_returns_an_owned_result() {
    let output = run_project_with_stdin(
        "use std::process\nfun main() { result := process::Process::run_capture(\"whoami\") assert(result.exit_code == 0) assert(!result.stdout.is_empty()) assert(result.stderr.is_empty()) echo \"captured\" }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "captured\n");
}

#[test]
fn tcp_client_connects_writes_and_reads_on_loopback() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener binds");
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("client connects");
        let mut request = [0_u8; 7];
        stream
            .read_exact(&mut request)
            .expect("client request arrives");
        assert_eq!(&request, b"request");
        stream
            .write_all(b"response")
            .expect("server response writes");
        stream
            .shutdown(Shutdown::Write)
            .expect("server finishes response");
    });
    let source = format!(
        "use std::net\nfun main() {{ mut stream := net::TcpStream::connect(\"{address}\") assert(stream.write(\"request\")) echo stream.read() }}"
    );
    let output = run_project_with_stdin(&source, b"");
    peer.join().expect("loopback peer completes");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "response\n");
}

#[test]
fn tcp_listener_accepts_and_reads_a_loopback_client_and_dns_resolves_localhost() {
    let reservation = TcpListener::bind("127.0.0.1:0").expect("loopback port reserves");
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let peer = std::thread::spawn(move || {
        let mut connected = None;
        for _ in 0..250 {
            if let Ok(stream) = TcpStream::connect(address) {
                connected = Some(stream);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut stream = connected.expect("Ryn listener accepts client");
        stream.write_all(b"from-peer").expect("peer request writes");
        stream
            .shutdown(Shutdown::Write)
            .expect("peer finishes request");
        let mut response = [0_u8; 3];
        stream
            .read_exact(&mut response)
            .expect("Ryn response arrives");
        assert_eq!(&response, b"ack");
    });
    let source = format!(
        "use std::net\nfun main() {{ mut listener := net::TcpListener::bind(\"{address}\") mut stream := listener.accept() request := stream.read() assert(request.contains(\"from-peer\")) assert(stream.write(\"ack\")) addresses := net::Dns::resolve(\"localhost\") assert(!addresses.is_empty()) echo \"accepted\" }}"
    );
    let output = run_project_with_stdin(&source, b"");
    peer.join().expect("loopback peer completes");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "accepted\n");
}

#[test]
fn udp_socket_sends_and_receives_loopback_datagrams() {
    let server = UdpSocket::bind("127.0.0.1:0").expect("loopback UDP socket binds");
    let address = server.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut request = [0_u8; 32];
        let (length, client) = server.recv_from(&mut request).expect("UDP request arrives");
        assert_eq!(&request[..length], b"udp-request");
        server
            .send_to(b"udp-response", client)
            .expect("UDP response sends");
    });
    let source = format!(
        "use std::net\nfun main() {{ mut socket := net::UdpSocket::bind(\"127.0.0.1:0\") assert(socket.connect(\"{address}\")) assert(socket.write(\"udp-request\")) echo socket.read() assert(socket.close()) }}"
    );
    let output = run_project_with_stdin(&source, b"");
    peer.join().expect("UDP peer completes");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "udp-response\n");
}

#[test]
fn vector_contains_uses_typed_scalar_and_string_equality() {
    let output = run_with_stdin(
        "fun main() { mut numbers := Vec<i32>() numbers.push(-7) numbers.push(42) echo numbers.contains(42) echo numbers.contains(1) mut words := Vec<String>() words.push(String(\"zażółć\")) echo words.contains(String(\"zażółć\")) echo words.contains(String(\"żółć\")) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "true\nfalse\ntrue\nfalse\n"
    );
}

#[test]
fn vector_get_first_and_last_return_owned_options() {
    let output = run_with_stdin(
        "fun main() { mut numbers := Vec<i32>() numbers.push(7) numbers.push(12) echo numbers.get(1).unwrap_or(-1) echo numbers.get(2).unwrap_or(-1) echo numbers.first().unwrap_or(-1) echo numbers.last().unwrap_or(-1) mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.get(0) words.clear() echo selected.is_some() echo selected.unwrap_or(String(\"fallback\")) missing := words.get(0) echo missing.unwrap_or(String(\"empty\")) echo words.last().is_none() }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "12\n-1\n7\n12\ntrue\nzażółć\nempty\ntrue\n"
    );
}

#[test]
fn vector_pop_returns_owned_option_and_transfers_string_elements() {
    let output = run_with_stdin(
        "fun main() { mut values := Vec<i32>() values.push(7) values.push(12) echo values.pop().unwrap_or(-1) echo values.pop().unwrap_or(-1) echo values.pop().unwrap_or(-1) mut words := Vec<String>() words.push(String(\"zażółć\")) selected := words.pop() echo words.is_empty() echo selected.unwrap_or(String(\"fallback\")) empty := words.pop() echo empty.unwrap_or(String(\"empty\")) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "12\n7\n-1\ntrue\nzażółć\nempty\n"
    );
}

#[test]
fn vector_sort_orders_owned_strings_by_utf8_content_without_losing_ownership() {
    let output = run_with_stdin(
        "fun main() { mut words := Vec<String>() words.push(String(\"żaba\")) words.push(String(\"kot\")) words.push(String(\"zażółć\")) words.sort() echo words.take(0) echo words.take(0) echo words.take(0) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "kot\nzażółć\nżaba\n"
    );
}

#[test]
fn sleep_accepts_duration_suffixes() {
    let output = run_with_stdin("fun main() { sleep(1ms) sleep(1ms) sleep(2s) }", b"");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (directory, source) = source_file("fun main() { sleep(3min) sleep(1h) }");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(&source)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn standard_time_module_exposes_wall_clock_and_monotonic_elapsed_time() {
    let directory = std::env::temp_dir().join(format!("ryn-time-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-time-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::time\nfun main() { assert(time::unix() > 0.0) start := time::instant_now() time::sleep(1ms) assert(time::instant_elapsed(start) >= 0.001) assert(time::monotonic() >= start) instant := time::Instant::now() time::sleep(1ms) assert(instant.elapsed() >= 0.001) current := time::Time::now() assert(current.unix_seconds > 0.0) assert(time::Time::unix() > 0.0) }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn standard_filesystem_and_path_modules_operate_on_paths() {
    let directory = std::env::temp_dir().join(format!("ryn-fs-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-fs-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::fs\nuse std::path\nfun main() { assert(fs::create_dir_all(\"build/files\")) assert(fs::write(\"build/files/one.txt\", \"hello\")) assert(fs::File::write(\"build/files/static.txt\", \"static-data\")) assert(fs::File::read_text(\"build/files/static.txt\").contains(\"static-data\")) echo fs::File::read(\"build/files/static.txt\") value := path::new(\"build/files/one.txt\") associated := path::Path::new(\"build/files/one.txt\") assert(associated.exists()) assert(value.exists()) assert(value.is_file()) echo value.filename() echo value.extension() absolute := value.absolute() assert(!absolute.raw.is_empty()) assert(absolute.is_absolute()) canonical := value.canonical() assert(!canonical.raw.is_empty()) assert(fs::copy_file(\"build/files/one.txt\", \"build/files/two.txt\")) assert(fs::rename(\"build/files/two.txt\", \"build/files/three.txt\")) assert(fs::remove_file(\"build/files/one.txt\")) assert(fs::remove_file(\"build/files/three.txt\")) assert(fs::remove_file(\"build/files/static.txt\")) mut writer := fs::File::create(\"build/files/stream.txt\") assert(writer.write(\"first\\r\\n\")) assert(writer.write(\"second\\n\")) assert(writer.flush()) assert(writer.close()) assert(fs::File::append(\"build/files/stream.txt\", \"third\\r\\n\")) mut reader := fs::File::open(\"build/files/stream.txt\") echo reader.read_line() echo reader.read_line() echo reader.read_line() echo reader.read_line() assert(reader.close()) assert(fs::remove_file(\"build/files/stream.txt\")) assert(fs::remove_dir(\"build/files\")) }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "static-data\none.txt\ntxt\nfirst\nsecond\nthird\n\n"
    );
}

#[test]
fn standard_thread_module_spawns_and_joins_a_worker() {
    let directory = std::env::temp_dir().join(format!("ryn-thread-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-thread-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::thread\ntype Worker = extern \"C\" fun()\nfun worker() { echo \"worker done\" }\nfun main() { mut background := thread::Thread::spawn(worker as Worker) id := background.id() assert(id > 0 as u64) assert(background.join()) echo \"main done\" }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("worker done\n"), "{stdout}");
    assert!(stdout.ends_with("main done\n"), "{stdout}");
}

#[test]
fn standard_keyboard_module_exposes_state_queries_and_read_key_type() {
    let directory = std::env::temp_dir().join(format!("ryn-keyboard-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-keyboard-smoke\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::keyboard\nfun read_once() -> Key { return keyboard::read_key() }\nfun main() { moving := keyboard::key_down(Key::W) jump := keyboard::key_pressed(Key::Space) released := keyboard::key_released(Key::W) echo \"keyboard api ready\" }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "keyboard api ready\n"
    );
}

#[test]
fn result_unwrap_or_returns_ok_value_or_evaluates_err_fallback() {
    let output = run_with_stdin(
        "fun fallback() -> i32 { echo \"fallback\" return -1 } fun main() { good: Result<i32, String> = Result::Ok(42) bad: Result<i32, String> = Result::Err(String(\"no\")) echo good.unwrap_or(fallback()) echo bad.unwrap_or(fallback()) }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "fallback\n42\nfallback\n-1\n"
    );
}

#[test]
fn standard_environment_path_and_file_wrappers_match_listed_names() {
    let directory = std::env::temp_dir().join(format!("ryn-std-api-{}", std::process::id()));
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("ryn.yaml"),
        "name: std-api-name-test\nversion: 0.1.0\nowner: guest\ndependencies:\n",
    )
    .unwrap();
    fs::write(
        directory.join("src/main.ryn"),
        "use std::env\nuse std::fs\nuse std::path\nfun main() { assert(set_env(\"RYN_API_TEST\", \"ok\")) echo env::env(\"RYN_API_TEST\") assert(remove_env(\"RYN_API_TEST\")) assert(fs::create_dir_all(\"build/files\")) assert(fs::write(\"build/files/a.txt\", \"contents\")) echo fs::read_file(\"build/files/a.txt\") assert(path::exists(\"build/files/a.txt\")) assert(path::is_file(\"build/files/a.txt\")) p := path::from(\"build/files/a.txt\") assert(p.exists()) assert(p.is_file()) assert(fs::remove_file(\"build/files/a.txt\")) assert(fs::remove_dir(\"build/files\")) }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("run")
        .arg(&directory)
        .current_dir(&directory)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\ncontents\n");
}

#[test]
fn system_assertions_and_panic_stop_the_program_with_failure() {
    for (source, message) in [
        (
            "fun main() { assert(false) echo \"continued\" }",
            "assertion failed",
        ),
        (
            "fun main() { assert_message(false, \"broken invariant\") echo \"continued\" }",
            "broken invariant",
        ),
        (
            "use std::system\nfun main() { system::panic(\"stop now\") echo \"continued\" }",
            "stop now",
        ),
    ] {
        let output = run_with_stdin(source, b"");
        assert!(
            !output.status.success(),
            "source unexpectedly succeeded: {source}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "stderr did not include `{message}`: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("continued"),
            "program continued after failure: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn standard_option_and_result_map_preserve_variants_and_transform_payloads() {
    let output = run_project_with_stdin(
        r#"use std::option
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
}"#,
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "12\ntrue\n16\nkept\n6\ncalled\nowned!\ncalled\nerror!\n"
    );
}

#[test]
fn option_map_moves_nested_owned_values_through_callback_without_aliasing() {
    let output = run_project_with_stdin(
        "use std::option\nfun add_tail(values: Vec<String>) -> Vec<String> { echo values.len() mut result := values result.push(String(\"🦀\")) return result }\nfun main() { callback: fun(Vec<String>) -> Vec<String> = add_tail mut source := Vec<String>() source.push(String(\"你好\")) wrapped: Option<Vec<String>> = Option::Some(source) mapped := wrapped.map(callback) values := mapped.unwrap() echo values.len() echo values.get(0).unwrap_or(String(\"missing\")) echo values.get(1).unwrap_or(String(\"missing\")) expected: Result<Vec<String>, String> = Result::Ok(values) extracted := expected.expect(\"has vector\") echo extracted.len() fallback_source: Result<Vec<String>, String> = Result::Err(String(\"no vector\")) fallback := fallback_source.unwrap_or(Vec<String>()) echo fallback.len() error_source: Result<String, Vec<String>> = Result::Err(extracted) error_values := error_source.unwrap_err() echo error_values.len() }",
        b"",
    );
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "1\n2\n你好\n🦀\n2\n0\n2\n"
    );
}

#[test]
fn option_and_result_extract_struct_and_array_payloads_without_aliasing() {
    let output = run_with_stdin(
        "struct Person { name: String, visits: i32 } fun main() { person := Person { name: String(\"Krystian\"), visits: 4 } maybe: Option<Person> = Option::Some(person) mut copy := maybe.unwrap() copy.name.append(\"!\") echo copy.name echo maybe.expect(\"person\").name fallback: Person = Person { name: String(\"fallback\"), visits: 0 } chosen := maybe.unwrap_or(fallback) echo chosen.name result: Result<[String; 2], String> = Result::Ok([String(\"zażółć\"), String(\"你好\")]) array := result.expect(\"array\") echo array[0] echo array[1] error: Result<String, [String; 2]> = Result::Err([String(\"first\"), String(\"🦀\")]) error_array := error.unwrap_err() echo error_array[0] echo error_array[1] }",
        b"",
    );
    assert!(
        output.status.success(),
        "exit: {:?}, stdout: {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Krystian!\nKrystian\nKrystian\nzażółć\n你好\nfirst\n🦀\n"
    );
}
#[test]
fn environment_exports_the_documented_set_env_name_and_paths_join_components() {
    let output = run_project_with_stdin(
        "use std::env\nuse std::path\nuse std::fs\nfun main() { assert(env::set_env(\"RYN_API_NAMED_SET\", \"ok\")) echo env::get(\"RYN_API_NAMED_SET\") assert(env::remove(\"RYN_API_NAMED_SET\")) assert(fs::create_dir_all(\"build/path-api\")) assert(fs::write(\"build/path-api/file.txt\", \"x\")) base := path::new(\"build/path-api\") joined := base.join(\"file.txt\") assert(joined.exists()) assert(joined.is_file()) assert(joined.is_directory() == false) parent := joined.parent() assert(parent.is_dir()) echo joined.filename() echo joined.extension() assert(fs::remove_file(\"build/path-api/file.txt\")) assert(fs::remove_dir(\"build/path-api\")) }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "ok\nfile.txt\ntxt\n"
    );
}
#[test]
fn filesystem_create_dir_is_non_recursive_and_create_dir_all_is_recursive() {
    let output = run_project_with_stdin(
        "use std::fs\nfun main() { assert(fs::create_dir(\"build/one\")) assert(!fs::create_dir(\"build/two/child\")) assert(fs::create_dir_all(\"build/two/child\")) assert(fs::remove_dir(\"build/one\")) assert(fs::remove_dir_all(\"build/two\")) }",
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
