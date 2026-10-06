use std::{
    ffi::{CString, c_char, c_void},
    io::{BufRead, Read, Write},
    net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    process::{Child, Command},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use super::strings::{self, RynString};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn GetProcAddress(library: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(library: *mut c_void) -> i32;
}

#[cfg(unix)]
#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(library: *mut c_void) -> i32;
}

fn owned(value: &str) -> *mut RynString {
    strings::ryn_string_new(value.as_ptr(), value.len() as u64)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('=') && !key.contains('\0')
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_exists(pointer: *const u8, length: u64) -> bool {
    std::env::var_os(strings::text(pointer, length)).is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_or(
    key_ptr: *const u8,
    key_len: u64,
    fallback_ptr: *const u8,
    fallback_len: u64,
) -> *mut RynString {
    let key = strings::text(key_ptr, key_len);
    let value = std::env::var_os(key).map(|value| value.to_string_lossy().into_owned());
    owned(
        value
            .as_deref()
            .unwrap_or_else(|| strings::text(fallback_ptr, fallback_len)),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_set(
    key_ptr: *const u8,
    key_len: u64,
    value_ptr: *const u8,
    value_len: u64,
) -> bool {
    let key = strings::text(key_ptr, key_len);
    let value = strings::text(value_ptr, value_len);
    if !valid_key(key) || value.contains('\0') {
        return false;
    }
    // SAFETY: The Ryn runtime is currently single-threaded and validates OS environment strings.
    unsafe {
        std::env::set_var(key, value);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_remove(pointer: *const u8, length: u64) -> bool {
    let key = strings::text(pointer, length);
    if !valid_key(key) {
        return false;
    }
    // SAFETY: The Ryn runtime is currently single-threaded and validates OS environment strings.
    unsafe {
        std::env::remove_var(key);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdin_read() -> *mut RynString {
    let mut contents = String::new();
    std::io::stdin()
        .read_to_string(&mut contents)
        .unwrap_or_else(|error| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "Ryn runtime error: could not read stdin: {error}"
            );
            std::process::exit(1)
        });
    owned(&contents)
}

fn remove_line_ending(line: &mut String) {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdin_read_line() -> *mut RynString {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .unwrap_or_else(|error| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "Ryn runtime error: could not read stdin line: {error}"
            );
            std::process::exit(1)
        });
    remove_line_ending(&mut line);
    owned(&line)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ask(pointer: *const u8, length: u64) -> *mut RynString {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(strings::text(pointer, length).as_bytes());
    let _ = stdout.flush();
    drop(stdout);
    ryn_stdin_read_line()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdout_write(pointer: *const u8, length: u64) {
    let _ = std::io::stdout().write_all(strings::text(pointer, length).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdout_flush() {
    let _ = std::io::stdout().flush();
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stderr_write(pointer: *const u8, length: u64) {
    let _ = std::io::stderr().write_all(strings::text(pointer, length).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stderr_flush() {
    let _ = std::io::stderr().flush();
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_run(pointer: *const u8, length: u64) -> i32 {
    Command::new(strings::text(pointer, length))
        .status()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_run_args(
    program_ptr: *const u8,
    program_len: u64,
    args: *const super::vectors::RynVec,
) -> i32 {
    let program = strings::text(program_ptr, program_len);
    let arguments = super::vectors::copy_string_items(args);
    Command::new(program)
        .args(arguments)
        .status()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(-1)
}

pub struct RynProcess {
    program: String,
    args: Vec<String>,
    environment: Vec<(String, String)>,
    cwd: Option<std::path::PathBuf>,
    child: Option<Child>,
}

pub struct RynProcessCapture {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_capture(
    program_ptr: *const u8,
    program_len: u64,
    args: *const super::vectors::RynVec,
) -> *mut RynProcessCapture {
    let program = strings::text(program_ptr, program_len);
    let arguments = super::vectors::copy_string_items(args);
    let capture = Command::new(program).args(arguments).output();
    let result = match capture {
        Ok(output) => RynProcessCapture {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(_) => RynProcessCapture {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
        },
    };
    Box::into_raw(Box::new(result))
}

fn capture_ref<'a>(pointer: *const RynProcessCapture) -> Option<&'a RynProcessCapture> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: The Ryn wrapper keeps this opaque capture handle live through its
    // accessors and drops it once after copying all result fields.
    Some(unsafe { &*pointer })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_capture_exit_code(pointer: *const RynProcessCapture) -> i32 {
    capture_ref(pointer)
        .map(|capture| capture.exit_code)
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_capture_stdout(pointer: *const RynProcessCapture) -> *mut RynString {
    let value = capture_ref(pointer)
        .map(|capture| capture.stdout.as_str())
        .unwrap_or("");
    strings::ryn_string_new(value.as_ptr(), value.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_capture_stderr(pointer: *const RynProcessCapture) -> *mut RynString {
    let value = capture_ref(pointer)
        .map(|capture| capture.stderr.as_str())
        .unwrap_or("");
    strings::ryn_string_new(value.as_ptr(), value.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_capture_drop(pointer: *mut RynProcessCapture) {
    if !pointer.is_null() {
        // SAFETY: The std wrapper owns and releases this handle exactly once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

pub struct RynTcpStream {
    socket: Option<TcpStream>,
}

pub struct RynTcpListener {
    socket: Option<TcpListener>,
}

pub struct RynUdpSocket {
    socket: Option<UdpSocket>,
}

pub struct RynThread {
    join: Option<std::thread::JoinHandle<()>>,
    id: u64,
}

static NEXT_RYN_THREAD_ID: AtomicU64 = AtomicU64::new(1);

#[unsafe(no_mangle)]
pub extern "C" fn ryn_thread_spawn(worker: *const c_void) -> *mut RynThread {
    if worker.is_null() {
        return std::ptr::null_mut();
    }
    // Ryn's typed `extern "C" fun()` pointer uses the host C calling convention.
    // SAFETY: The compiler checks that Thread.spawn receives a zero-argument,
    // void-returning C-ABI function pointer, and Ryn Guard keeps code loaded.
    let worker: extern "C" fn() = unsafe { std::mem::transmute(worker) };
    let join = std::thread::spawn(move || worker());
    let id = NEXT_RYN_THREAD_ID.fetch_add(1, Ordering::Relaxed);
    Box::into_raw(Box::new(RynThread {
        join: Some(join),
        id,
    }))
}

fn thread_mut<'a>(pointer: *mut RynThread) -> Option<&'a mut RynThread> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: The owning Ryn Thread handle is unique and remains live for calls.
    Some(unsafe { &mut *pointer })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_thread_join(pointer: *mut RynThread) -> bool {
    thread_mut(pointer)
        .and_then(|thread| thread.join.take())
        .is_some_and(|join| join.join().is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_thread_id(pointer: *const RynThread) -> u64 {
    if pointer.is_null() {
        return 0;
    }
    // SAFETY: The caller holds an owning Thread handle for this read.
    unsafe { (*pointer).id }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_thread_drop(pointer: *mut RynThread) {
    if !pointer.is_null() {
        // SAFETY: The std Thread wrapper owns and drops this handle once.
        let mut thread = unsafe { Box::from_raw(pointer) };
        if let Some(join) = thread.join.take() {
            let _ = join.join();
        }
    }
}

fn resolve_socket_address(address: &str) -> Option<std::net::SocketAddr> {
    address.to_socket_addrs().ok()?.next()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_connect(pointer: *const u8, length: u64) -> *mut RynTcpStream {
    let address = strings::text(pointer, length);
    let Some(address) = resolve_socket_address(address) else {
        return std::ptr::null_mut();
    };
    let socket = TcpStream::connect(address).ok();
    Box::into_raw(Box::new(RynTcpStream { socket }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_read(pointer: *mut RynTcpStream) -> *mut RynString {
    if pointer.is_null() {
        return owned("");
    }
    // SAFETY: Ryn Guard keeps the mutable stream handle live for this call.
    let stream = unsafe { &mut *pointer };
    let mut bytes = Vec::new();
    if stream
        .socket
        .as_mut()
        .is_some_and(|socket| socket.read_to_end(&mut bytes).is_err())
    {
        return owned("");
    }
    owned(&String::from_utf8_lossy(&bytes))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_write(
    pointer: *mut RynTcpStream,
    data_pointer: *const u8,
    data_length: u64,
) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: Ryn Guard keeps the mutable stream handle live for this call.
    let stream = unsafe { &mut *pointer };
    stream.socket.as_mut().is_some_and(|socket| {
        socket
            .write_all(strings::text(data_pointer, data_length).as_bytes())
            .is_ok()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_close(pointer: *mut RynTcpStream) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: Ryn Guard keeps the mutable stream handle live for this call.
    let stream = unsafe { &mut *pointer };
    stream
        .socket
        .take()
        .is_some_and(|socket| socket.shutdown(Shutdown::Both).is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_drop(pointer: *mut RynTcpStream) {
    if !pointer.is_null() {
        // SAFETY: The std TcpStream wrapper owns this box and drops it once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_listener_bind(pointer: *const u8, length: u64) -> *mut RynTcpListener {
    let address = strings::text(pointer, length);
    let socket = address
        .parse::<std::net::SocketAddr>()
        .ok()
        .and_then(|address| TcpListener::bind(address).ok());
    Box::into_raw(Box::new(RynTcpListener { socket }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_accept(pointer: *mut RynTcpListener) -> *mut RynTcpStream {
    if pointer.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Ryn Guard keeps the mutable listener handle live for this call.
    let listener = unsafe { &mut *pointer };
    let socket = listener
        .socket
        .as_ref()
        .and_then(|listener| listener.accept().ok())
        .map(|(socket, _)| socket);
    Box::into_raw(Box::new(RynTcpStream { socket }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_listener_close(pointer: *mut RynTcpListener) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: Ryn Guard keeps the mutable listener handle live for this call.
    unsafe { &mut *pointer }.socket.take().is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_tcp_listener_drop(pointer: *mut RynTcpListener) {
    if !pointer.is_null() {
        // SAFETY: The std TcpListener wrapper owns this box and drops it once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_bind(pointer: *const u8, length: u64) -> *mut RynUdpSocket {
    let address = strings::text(pointer, length);
    let socket = address
        .parse::<std::net::SocketAddr>()
        .ok()
        .and_then(|address| UdpSocket::bind(address).ok());
    Box::into_raw(Box::new(RynUdpSocket { socket }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_connect(
    pointer: *mut RynUdpSocket,
    address_pointer: *const u8,
    address_length: u64,
) -> bool {
    if pointer.is_null() {
        return false;
    }
    let address = strings::text(address_pointer, address_length);
    let Some(address) = resolve_socket_address(address) else {
        return false;
    };
    // SAFETY: Ryn Guard keeps the mutable UDP socket handle live for this call.
    unsafe { &mut *pointer }
        .socket
        .as_ref()
        .is_some_and(|socket| socket.connect(address).is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_read(pointer: *mut RynUdpSocket) -> *mut RynString {
    if pointer.is_null() {
        return owned("");
    }
    let mut bytes = vec![0_u8; u16::MAX as usize];
    // SAFETY: Ryn Guard keeps the mutable UDP socket handle live for this call.
    let result = unsafe { &mut *pointer }
        .socket
        .as_ref()
        .and_then(|socket| socket.recv(&mut bytes).ok());
    let Some(length) = result else {
        return owned("");
    };
    owned(&String::from_utf8_lossy(&bytes[..length]))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_write(
    pointer: *mut RynUdpSocket,
    data_pointer: *const u8,
    data_length: u64,
) -> bool {
    if pointer.is_null() {
        return false;
    }
    let data = strings::text(data_pointer, data_length);
    // SAFETY: Ryn Guard keeps the mutable UDP socket handle live for this call.
    unsafe { &mut *pointer }
        .socket
        .as_ref()
        .is_some_and(|socket| {
            socket
                .send(data.as_bytes())
                .is_ok_and(|written| written == data.len())
        })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_close(pointer: *mut RynUdpSocket) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: Ryn Guard keeps the mutable UDP socket handle live for this call.
    unsafe { &mut *pointer }.socket.take().is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_udp_drop(pointer: *mut RynUdpSocket) {
    if !pointer.is_null() {
        // SAFETY: The std UdpSocket wrapper owns this box and drops it once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_dns_resolve(pointer: *const u8, length: u64) -> *mut super::vectors::RynVec {
    let host = strings::text(pointer, length);
    let result = super::vectors::ryn_vec_new(
        1,
        Some(strings::ryn_vec_elem_string_drop),
        Some(strings::ryn_vec_elem_string_clone),
    );
    if let Ok(addresses) = (host, 0).to_socket_addrs() {
        let mut seen = std::collections::HashSet::new();
        for address in addresses {
            let value = address.ip().to_string();
            if seen.insert(value.clone()) {
                let item = owned(&value);
                // SAFETY: Vec<String> takes ownership of this fresh String handle.
                super::vectors::ryn_vec_push(result, (&item as *const *mut RynString).cast());
            }
        }
    }
    result
}

#[cfg(test)]
mod process_capture_tests {
    use super::*;

    fn push_string(vector: *mut super::super::vectors::RynVec, value: &str) {
        let item = strings::ryn_string_new(value.as_ptr(), value.len() as u64);
        // SAFETY: Vec<String> uses the registered String clone/drop callbacks and
        // receives a pointer to one owning String handle.
        unsafe {
            super::super::vectors::ryn_vec_push(vector, (&item as *const *mut RynString).cast());
        }
    }

    #[test]
    fn capture_keeps_exit_code_and_both_utf8_streams() {
        let arguments = super::super::vectors::ryn_vec_new(
            1,
            Some(strings::ryn_vec_elem_string_drop),
            Some(strings::ryn_vec_elem_string_clone),
        );
        #[cfg(windows)]
        let (program, command) = (
            "cmd.exe",
            "echo capture-out & echo capture-err 1>&2 & exit /b 7",
        );
        #[cfg(not(windows))]
        let (program, command) = ("sh", "printf capture-out; printf capture-err >&2; exit 7");
        #[cfg(windows)]
        {
            push_string(arguments, "/c");
        }
        #[cfg(not(windows))]
        {
            push_string(arguments, "-c");
        }
        push_string(arguments, command);
        let program_ptr = program.as_ptr();
        let capture = ryn_process_capture(program_ptr, program.len() as u64, arguments);
        assert_eq!(ryn_process_capture_exit_code(capture), 7);
        let stdout = ryn_process_capture_stdout(capture);
        let stderr = ryn_process_capture_stderr(capture);
        assert!(strings::copy_owned(stdout).contains("capture-out"));
        assert!(strings::copy_owned(stderr).contains("capture-err"));
        strings::ryn_string_drop(stdout);
        strings::ryn_string_drop(stderr);
        ryn_process_capture_drop(capture);
        super::super::vectors::ryn_vec_drop(arguments);
    }

    #[test]
    fn capture_reports_a_missing_program_without_invalid_handles() {
        let arguments = super::super::vectors::ryn_vec_new(
            1,
            Some(strings::ryn_vec_elem_string_drop),
            Some(strings::ryn_vec_elem_string_clone),
        );
        let program = "ryn-program-that-does-not-exist-for-capture-test";
        let capture = ryn_process_capture(program.as_ptr(), program.len() as u64, arguments);
        assert_eq!(ryn_process_capture_exit_code(capture), -1);
        let stdout = ryn_process_capture_stdout(capture);
        let stderr = ryn_process_capture_stderr(capture);
        assert!(strings::copy_owned(stdout).is_empty());
        assert!(strings::copy_owned(stderr).is_empty());
        strings::ryn_string_drop(stdout);
        strings::ryn_string_drop(stderr);
        ryn_process_capture_drop(capture);
        super::super::vectors::ryn_vec_drop(arguments);
    }

    #[test]
    fn tcp_ffi_reads_and_writes_on_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 3];
            socket.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"hey");
            socket.write_all(b"ok").unwrap();
            socket.shutdown(Shutdown::Write).unwrap();
        });
        let stream = ryn_tcp_connect(address.as_ptr(), address.len() as u64);
        assert!(!stream.is_null());
        assert!(ryn_tcp_write(stream, b"hey".as_ptr(), 3));
        let response = ryn_tcp_read(stream);
        assert_eq!(strings::copy_owned(response), "ok");
        strings::ryn_string_drop(response);
        assert!(ryn_tcp_close(stream));
        ryn_tcp_drop(stream);
        peer.join().unwrap();
    }

    #[test]
    fn udp_ffi_connects_and_transfers_one_loopback_datagram() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap().to_string();
        let peer = std::thread::spawn(move || {
            let mut request = [0_u8; 8];
            let (length, remote) = server.recv_from(&mut request).unwrap();
            assert_eq!(&request[..length], b"datagram");
            server.send_to(b"reply", remote).unwrap();
        });
        let local = "127.0.0.1:0";
        let socket = ryn_udp_bind(local.as_ptr(), local.len() as u64);
        assert!(!socket.is_null());
        assert!(ryn_udp_connect(
            socket,
            address.as_ptr(),
            address.len() as u64
        ));
        assert!(ryn_udp_write(socket, b"datagram".as_ptr(), 8));
        let response = ryn_udp_read(socket);
        assert_eq!(strings::copy_owned(response), "reply");
        strings::ryn_string_drop(response);
        assert!(ryn_udp_close(socket));
        ryn_udp_drop(socket);
        peer.join().unwrap();
    }

    #[test]
    fn dns_runtime_returns_owned_addresses_for_localhost() {
        let host = "localhost";
        let addresses = ryn_dns_resolve(host.as_ptr(), host.len() as u64);
        assert!(!addresses.is_null());
        assert!(super::super::vectors::ryn_vec_len(addresses) > 0);
        super::super::vectors::ryn_vec_drop(addresses);
    }
}

impl Drop for RynProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn process_mut<'a>(pointer: *mut RynProcess) -> Option<&'a mut RynProcess> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: Process handles are opaque owning values; Ryn Guard enforces exclusive
    // access to mutating methods and invokes drop once when the owner leaves scope.
    Some(unsafe { &mut *pointer })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_new(pointer: *const u8, length: u64) -> *mut RynProcess {
    Box::into_raw(Box::new(RynProcess {
        program: strings::text(pointer, length).to_owned(),
        args: Vec::new(),
        environment: Vec::new(),
        cwd: None,
        child: None,
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_arg(
    process: *mut RynProcess,
    pointer: *const u8,
    length: u64,
) -> bool {
    let Some(process) = process_mut(process) else {
        return false;
    };
    if process.child.is_some() {
        return false;
    }
    process.args.push(strings::text(pointer, length).to_owned());
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_args(
    process: *mut RynProcess,
    args: *const super::vectors::RynVec,
) -> bool {
    let Some(process) = process_mut(process) else {
        return false;
    };
    if process.child.is_some() {
        return false;
    }
    process.args.extend(super::vectors::copy_string_items(args));
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_env(
    process: *mut RynProcess,
    key_pointer: *const u8,
    key_length: u64,
    value_pointer: *const u8,
    value_length: u64,
) -> bool {
    let Some(process) = process_mut(process) else {
        return false;
    };
    if process.child.is_some() {
        return false;
    }
    let key = strings::text(key_pointer, key_length).to_owned();
    let value = strings::text(value_pointer, value_length).to_owned();
    if let Some((_, old_value)) = process
        .environment
        .iter_mut()
        .find(|(old_key, _)| *old_key == key)
    {
        *old_value = value;
    } else {
        process.environment.push((key, value));
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_cwd(
    process: *mut RynProcess,
    pointer: *const u8,
    length: u64,
) -> bool {
    let Some(process) = process_mut(process) else {
        return false;
    };
    if process.child.is_some() {
        return false;
    }
    process.cwd = Some(strings::text(pointer, length).into());
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_spawn(pointer: *mut RynProcess) -> bool {
    let Some(process) = process_mut(pointer) else {
        return false;
    };
    if process.child.is_some() {
        return false;
    }
    let mut command = Command::new(&process.program);
    command
        .args(&process.args)
        .envs(process.environment.iter().cloned());
    if let Some(cwd) = &process.cwd {
        command.current_dir(cwd);
    }
    match command.spawn() {
        Ok(child) => {
            process.child = Some(child);
            true
        }
        Err(_) => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_wait(pointer: *mut RynProcess) -> i32 {
    let Some(process) = process_mut(pointer) else {
        return -1;
    };
    process
        .child
        .take()
        .and_then(|mut child| child.wait().ok())
        .and_then(|status| status.code())
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_kill(pointer: *mut RynProcess) -> bool {
    let Some(process) = process_mut(pointer) else {
        return false;
    };
    process
        .child
        .as_mut()
        .is_some_and(|child| child.kill().is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_process_drop(pointer: *mut RynProcess) {
    if !pointer.is_null() {
        // SAFETY: Ryn Guard owns and destroys each Process handle exactly once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_panic(pointer: *const u8, length: u64) {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn panic: {}",
        strings::text(pointer, length)
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_exit(code: i32) -> ! {
    std::process::exit(code)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_assert(condition: bool) {
    if !condition {
        let _ = writeln!(std::io::stderr().lock(), "Ryn panic: assertion failed");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_assert_message(condition: bool, pointer: *const u8, length: u64) {
    if !condition {
        let _ = writeln!(
            std::io::stderr().lock(),
            "Ryn panic: {}",
            strings::text(pointer, length)
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_dyn_load_library(pointer: *const u8, length: u64) -> *mut c_void {
    let path = strings::text(pointer, length);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: The path is NUL-terminated UTF-16 owned by this call.
        unsafe { LoadLibraryW(wide.as_ptr()) }
    }
    #[cfg(unix)]
    {
        let Ok(path) = CString::new(path) else {
            return std::ptr::null_mut();
        };
        // SAFETY: The path is a live NUL-terminated string and RTLD_NOW is 2.
        unsafe { dlopen(path.as_ptr(), 2) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        std::ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_dyn_load_symbol(
    library: *mut c_void,
    pointer: *const u8,
    length: u64,
) -> *mut c_void {
    if library.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(name) = CString::new(strings::text(pointer, length)) else {
        return std::ptr::null_mut();
    };
    #[cfg(windows)]
    {
        // SAFETY: The handle is checked and the symbol name is NUL-terminated.
        unsafe { GetProcAddress(library, name.as_ptr()) }
    }
    #[cfg(unix)]
    {
        // SAFETY: The handle is checked and the symbol name is NUL-terminated.
        unsafe { dlsym(library, name.as_ptr()) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = name;
        std::ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_dyn_unload_library(library: *mut c_void) -> bool {
    if library.is_null() {
        return false;
    }
    #[cfg(windows)]
    {
        // SAFETY: The caller owns a handle returned by ryn_dyn_load_library.
        unsafe { FreeLibrary(library) != 0 }
    }
    #[cfg(unix)]
    {
        // SAFETY: The caller owns a handle returned by ryn_dyn_load_library.
        unsafe { dlclose(library) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = library;
        false
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_pointer_is_null(pointer: *mut c_void) -> bool {
    pointer.is_null()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_call_c_i32_one(function: *mut c_void, argument: i32) -> i32 {
    if function.is_null() {
        return i32::MIN;
    }
    // SAFETY: Ryn requires a non-null symbol for `extern "C" fn(i32) -> i32`.
    let function: unsafe extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(function) };
    // SAFETY: The function pointer is invoked with the declared ABI and signature.
    unsafe { function(argument) }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_abs(value: f64) -> f64 {
    value.abs()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_min(left: f64, right: f64) -> f64 {
    left.min(right)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_max(left: f64, right: f64) -> f64 {
    left.max(right)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_clamp(value: f64, min: f64, max: f64) -> f64 {
    if min > max {
        return f64::NAN;
    }
    value.clamp(min, max)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_sqrt(value: f64) -> f64 {
    value.sqrt()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_pow(value: f64, exponent: f64) -> f64 {
    value.powf(exponent)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_floor(value: f64) -> f64 {
    value.floor()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_ceil(value: f64) -> f64 {
    value.ceil()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_round(value: f64) -> f64 {
    value.round()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_sin(value: f64) -> f64 {
    value.sin()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_cos(value: f64) -> f64 {
    value.cos()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_tan(value: f64) -> f64 {
    value.tan()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_log(value: f64) -> f64 {
    value.ln()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_log2(value: f64) -> f64 {
    value.log2()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_math_log10(value: f64) -> f64 {
    value.log10()
}

static RANDOM_STATE: AtomicU64 = AtomicU64::new(0);
fn next_random() -> u64 {
    let mut current = RANDOM_STATE.load(Ordering::Relaxed);
    loop {
        let seed = if current == 0 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        } else {
            current
        };
        let mut next = seed;
        next ^= next << 13;
        next ^= next >> 7;
        next ^= next << 17;
        if next == 0 {
            next = 0x9e37_79b9_7f4a_7c15;
        }
        match RANDOM_STATE.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(observed) => current = observed,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_random() -> f64 {
    (next_random() >> 11) as f64 / ((1u64 << 53) as f64)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_random_range(min: f64, max: f64) -> f64 {
    if min >= max {
        return min;
    }
    min + ryn_random() * (max - min)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_sleep(milliseconds: u64) {
    std::thread::sleep(std::time::Duration::from_millis(milliseconds));
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_yield_thread() {
    std::thread::yield_now();
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_option_unwrap_failed() {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn runtime error: called Option.unwrap() on None"
    );
    std::process::exit(1);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_option_expect_failed(pointer: *const u8, length: u64) {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn runtime error: {}",
        strings::text(pointer, length)
    );
    std::process::exit(1);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_result_unwrap_failed() {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn runtime error: called Result extraction on the wrong variant"
    );
    std::process::exit(1);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_time_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

static RYN_MONOTONIC_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn ryn_time_monotonic() -> f64 {
    RYN_MONOTONIC_ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_time_elapsed(started_at_monotonic_seconds: f64) -> f64 {
    (ryn_time_monotonic() - started_at_monotonic_seconds).max(0.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_get(pointer: *const u8, length: u64) -> *mut RynString {
    let value = std::env::var_os(strings::text(pointer, length))
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    owned(&value)
}

fn owned_path(path: Result<std::path::PathBuf, std::io::Error>) -> *mut RynString {
    let value = path
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    owned(&value)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_current_dir() -> *mut RynString {
    owned_path(std::env::current_dir())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_set_current_dir(pointer: *const u8, length: u64) -> bool {
    std::env::set_current_dir(strings::text(pointer, length)).is_ok()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_home_dir() -> *mut RynString {
    owned_path(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)),
    )
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_temp_dir() -> *mut RynString {
    owned(&std::env::temp_dir().to_string_lossy())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_executable_path() -> *mut RynString {
    owned_path(std::env::current_exe())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_os() -> *mut RynString {
    owned(std::env::consts::OS)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_arch() -> *mut RynString {
    owned(std::env::consts::ARCH)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(u32::MAX as usize) as u32)
        .unwrap_or(1)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_hostname() -> *mut RynString {
    let value = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    owned(&value)
}
