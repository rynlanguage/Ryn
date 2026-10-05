use std::{
    io::Write,
    slice,
    sync::OnceLock,
};

static RYN_ARGUMENTS: OnceLock<Vec<String>> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn ryn_args_init() {
    let _ = arguments();
}

fn arguments() -> &'static [String] {
    RYN_ARGUMENTS.get_or_init(|| {
        std::env::args_os()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_args_count() -> u32 {
    u32::try_from(arguments().len()).unwrap_or(u32::MAX)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_arg_ptr(index: u32) -> *const u8 {
    usize::try_from(index)
        .ok()
        .and_then(|index| arguments().get(index))
        .map_or("".as_ptr(), |argument| argument.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_arg_len(index: u32) -> u64 {
    usize::try_from(index)
        .ok()
        .and_then(|index| arguments().get(index))
        .and_then(|argument| u64::try_from(argument.len()).ok())
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print(ptr: *const u8, len: u64) {
    if len == 0 || ptr.is_null() || len > isize::MAX as u64 {
        return;
    }
    let Ok(len) = usize::try_from(len) else {
        return;
    };
    // SAFETY: Non-empty Ryn strings point to immutable object data for `len` bytes.
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    let _ = std::io::stdout().write_all(bytes);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_str_equals(
    left: *const u8,
    left_len: u64,
    right: *const u8,
    right_len: u64,
) -> i8 {
    if left_len != right_len {
        return 0;
    }
    if left_len == 0 {
        return 1;
    }
    if left.is_null() || right.is_null() || left_len > isize::MAX as u64 {
        return 0;
    }
    let Ok(left_len) = usize::try_from(left_len) else {
        return 0;
    };
    // SAFETY: Generated Ryn strings point to immutable object data and equal lengths were checked above.
    let left = unsafe { slice::from_raw_parts(left, left_len) };
    // SAFETY: Generated Ryn strings point to immutable object data and equal lengths were checked above.
    let right = unsafe { slice::from_raw_parts(right, left_len) };
    i8::from(left == right)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_i8(value: i8) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_i16(value: i16) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_i32(value: i32) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_i64(value: i64) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_u8(value: u8) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_u16(value: u16) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_u32(value: u32) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_u64(value: u64) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_f32(value: f32) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_f64(value: f64) {
    let _ = write!(std::io::stdout().lock(), "{value}");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_bool(value: i8) {
    let _ = write!(std::io::stdout().lock(), "{}", value != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_newline() {
    let _ = std::io::stdout().write_all(b"\n");
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_integer_division_by_zero() {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn runtime error: integer division or remainder by zero"
    );
    std::process::exit(1);
}
