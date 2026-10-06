use std::{
    ffi::{CString, c_char, c_void},
    io::{Read, Write},
    process::Command,
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
pub extern "C" fn ryn_env_or(key_ptr: *const u8, key_len: u64, fallback_ptr: *const u8, fallback_len: u64) -> *mut RynString {
    let key = strings::text(key_ptr, key_len);
    let value = std::env::var_os(key).map(|value| value.to_string_lossy().into_owned());
    owned(value.as_deref().unwrap_or_else(|| strings::text(fallback_ptr, fallback_len)))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_set(key_ptr: *const u8, key_len: u64, value_ptr: *const u8, value_len: u64) -> bool {
    let key = strings::text(key_ptr, key_len);
    let value = strings::text(value_ptr, value_len);
    if !valid_key(key) || value.contains('\0') { return false; }
    // SAFETY: The Ryn runtime is currently single-threaded and validates OS environment strings.
    unsafe { std::env::set_var(key, value); }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_env_remove(pointer: *const u8, length: u64) -> bool {
    let key = strings::text(pointer, length);
    if !valid_key(key) { return false; }
    // SAFETY: The Ryn runtime is currently single-threaded and validates OS environment strings.
    unsafe { std::env::remove_var(key); }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdin_read() -> *mut RynString {
    let mut contents = String::new();
    std::io::stdin().read_to_string(&mut contents).unwrap_or_else(|error| {
        let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: could not read stdin: {error}");
        std::process::exit(1)
    });
    owned(&contents)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stdout_write(pointer: *const u8, length: u64) {
    let _ = std::io::stdout().write_all(strings::text(pointer, length).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_stderr_write(pointer: *const u8, length: u64) {
    let _ = std::io::stderr().write_all(strings::text(pointer, length).as_bytes());
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
pub extern "C" fn ryn_process_run_args(program_ptr: *const u8, program_len: u64, args: *const super::vectors::RynVec) -> i32 {
    let program = strings::text(program_ptr, program_len);
    let arguments = super::vectors::copy_string_items(args);
    Command::new(program)
        .args(arguments)
        .status()
        .ok()
        .and_then(|status| status.code())
        .unwrap_or(-1)
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
        let Ok(path) = CString::new(path) else { return std::ptr::null_mut() };
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
