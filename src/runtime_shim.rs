use std::{io::Write, slice, sync::OnceLock};

#[path = "runtime/string.rs"]
pub mod strings;

#[path = "runtime/filesystem.rs"]
pub mod filesystem;

#[path = "runtime/system.rs"]
pub mod system;

#[path = "runtime/vector.rs"]
pub mod vectors;

#[path = "runtime/map.rs"]
pub mod maps;

#[path = "runtime/enum.rs"]
pub mod enums;

#[path = "runtime/input.rs"]
pub mod input;

#[repr(C)]
pub struct RynFfiRecordI32 {
    pub value: i32,
}

#[repr(C)]
pub struct RynFfiPairI32 {
    pub first: i32,
    pub second: i32,
}

#[repr(C)]
pub struct RynFfiPairI16 {
    pub first: i16,
    pub second: i16,
}

#[repr(C)]
pub struct RynFfiBytePair {
    pub first: u8,
    pub second: u8,
}

#[repr(C)]
pub struct RynFfiMixedPair {
    pub first: u8,
    pub second: u16,
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[repr(C)]
pub struct RynFfiFloatPair {
    pub first: f32,
    pub second: f32,
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[repr(C)]
pub struct RynFfiDoubleValue {
    pub value: f64,
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_float_pair_sum(pair: RynFfiFloatPair) -> f32 {
    pair.first + pair.second
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_float_pair(first: f32, second: f32) -> RynFfiFloatPair {
    RynFfiFloatPair { first, second }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_double_value(value: RynFfiDoubleValue) -> RynFfiDoubleValue {
    value
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_record_i32(record: RynFfiRecordI32) -> i32 {
    record.value + 1
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_record_i32(value: i32) -> RynFfiRecordI32 {
    RynFfiRecordI32 { value }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_pair_i32(pair: RynFfiPairI32) -> i32 {
    pair.first + pair.second
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_pair_i32(first: i32, second: i32) -> RynFfiPairI32 {
    RynFfiPairI32 { first, second }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_pair_i16(pair: RynFfiPairI16) -> i16 {
    pair.first + pair.second
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_pair_i16(first: i16, second: i16) -> RynFfiPairI16 {
    RynFfiPairI16 { first, second }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_byte_pair(pair: RynFfiBytePair) -> u8 {
    pair.first + pair.second
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_byte_pair(first: u8, second: u8) -> RynFfiBytePair {
    RynFfiBytePair { first, second }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_mixed_pair(pair: RynFfiMixedPair) -> u16 {
    pair.first as u16 + pair.second
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_ffi_make_mixed_pair(first: u8, second: u16) -> RynFfiMixedPair {
    RynFfiMixedPair { first, second }
}

#[cfg(ryn_runtime_debug)]
pub fn debug_live_vectors() -> usize {
    vectors::live_vectors()
}

#[cfg(ryn_runtime_debug)]
pub fn debug_live_strings() -> usize {
    strings::live_strings()
}

static RYN_ARGUMENTS: OnceLock<Vec<String>> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn ryn_args_init() {
    let _ = arguments();
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    stack_guard::install();
}

/// Turns a stack overflow of the main thread (a SIGSEGV just past the end of the stack) into a
/// readable runtime error instead of a bare segmentation fault. The handler runs on its own
/// alternate stack. Any other SIGSEGV keeps its default behaviour after a short message.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod stack_guard {
    use std::{
        ffi::c_void,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[repr(C)]
    struct StackT {
        sp: *mut c_void,
        flags: i32,
        size: usize,
    }

    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: i32,
        restorer: usize,
    }

    #[repr(C)]
    struct RLimit {
        current: u64,
        maximum: u64,
    }

    unsafe extern "C" {
        fn sigaltstack(new: *const StackT, old: *mut StackT) -> i32;
        fn sigaction(signal: i32, new: *const SigAction, old: *mut SigAction) -> i32;
        fn getrlimit(resource: i32, limit: *mut RLimit) -> i32;
        fn write(fd: i32, buffer: *const u8, length: usize) -> isize;
        fn _exit(code: i32) -> !;
    }

    const SIGSEGV: i32 = 11;
    const SIGBUS: i32 = 7;
    const SA_SIGINFO: i32 = 4;
    const SA_ONSTACK: i32 = 0x0800_0000;
    const SA_RESETHAND: i32 = 0x8000_0000_u32 as i32;
    const RLIMIT_STACK: i32 = 3;
    const ALTERNATE_STACK: usize = 64 * 1024;

    static STACK_LOW: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn handler(signal: i32, info: *const u8, _context: *const c_void) {
        // `si_addr` is the faulting address; it sits at offset 16 of `siginfo_t` on 64-bit Linux.
        let address = unsafe { std::ptr::read_unaligned(info.add(16) as *const usize) };
        let low = STACK_LOW.load(Ordering::Relaxed);
        let margin = 1024 * 1024;
        let message: &[u8] = if low != 0 && address + margin >= low && address <= low + margin {
            b"Ryn runtime error: stack overflow (recursion is too deep)\n"
        } else if signal == SIGBUS {
            b"Ryn runtime error: bus error\n"
        } else {
            b"Ryn runtime error: invalid memory access (segmentation fault)\n"
        };
        unsafe {
            write(2, message.as_ptr(), message.len());
            _exit(1)
        }
    }

    pub fn install() {
        unsafe {
            // The stack grows down from just above this frame; its lowest address is
            // approximately `top - limit`.
            let marker = 0u8;
            let top = &marker as *const u8 as usize;
            let mut limit = RLimit {
                current: 0,
                maximum: 0,
            };
            if getrlimit(RLIMIT_STACK, &mut limit) == 0
                && limit.current != u64::MAX
                && limit.current > 0
            {
                STACK_LOW.store(
                    top.saturating_sub(limit.current as usize),
                    Ordering::Relaxed,
                );
            }
            let memory = Box::leak(vec![0u8; ALTERNATE_STACK].into_boxed_slice());
            let alternate = StackT {
                sp: memory.as_mut_ptr() as *mut c_void,
                flags: 0,
                size: ALTERNATE_STACK,
            };
            if sigaltstack(&alternate, std::ptr::null_mut()) != 0 {
                return;
            }
            let action = SigAction {
                handler: handler as usize,
                mask: [0; 16],
                flags: SA_SIGINFO | SA_ONSTACK | SA_RESETHAND,
                restorer: 0,
            };
            sigaction(SIGSEGV, &action, std::ptr::null_mut());
            sigaction(SIGBUS, &action, std::ptr::null_mut());
        }
    }
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

#[unsafe(no_mangle)]
pub extern "C" fn ryn_array_index_out_of_bounds() {
    let _ = writeln!(
        std::io::stderr().lock(),
        "Ryn runtime error: array index is out of bounds"
    );
    std::process::exit(1);
}

/// Process entry point when the runtime is linked as a static library by
/// Ryn's own linker. It replaces the Rust `main` wrapper that rustc used to
/// generate: initialize argument storage, run the program, flush stdout, and
/// exit with the program's status.
#[cfg(ryn_staticlib)]
mod entry {
    unsafe extern "C" {
        fn ryn_main() -> i32;
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn ryn_start() -> ! {
        super::ryn_args_init();
        // SAFETY: the Cranelift object always defines this C ABI entry point.
        let code = unsafe { ryn_main() };
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::process::exit(code)
    }

    /// The C runtime normally defines `main` for `cc`-linked Unix executables.
    #[cfg(unix)]
    #[unsafe(no_mangle)]
    pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
        super::ryn_args_init();
        // SAFETY: as above.
        let code = unsafe { ryn_main() };
        let _ = std::io::Write::flush(&mut std::io::stdout());
        code
    }
}
