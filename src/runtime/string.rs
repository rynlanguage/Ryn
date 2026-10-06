use std::{io::Write, slice, str};

pub struct RynString(String);

#[cfg(ryn_runtime_debug)]
static LIVE_STRINGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(ryn_runtime_debug)]
impl Drop for RynString {
    fn drop(&mut self) {
        LIVE_STRINGS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(ryn_runtime_debug)]
pub fn live_strings() -> usize {
    LIVE_STRINGS.load(std::sync::atomic::Ordering::Relaxed)
}

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}

pub(crate) fn text<'a>(pointer: *const u8, length: u64) -> &'a str {
    if length == 0 {
        return "";
    }
    let length = usize::try_from(length)
        .ok()
        .filter(|length| *length <= isize::MAX as usize)
        .unwrap_or_else(|| fail("string length exceeds the host address space"));
    if pointer.is_null() {
        fail("null string data");
    }
    // SAFETY: The compiler passes valid str views whose data remains live for the call.
    let bytes = unsafe { slice::from_raw_parts(pointer, length) };
    str::from_utf8(bytes).unwrap_or_else(|_| fail("invalid UTF-8 string"))
}

fn owned<'a>(pointer: *const RynString) -> &'a String {
    if pointer.is_null() {
        fail("use of a moved String");
    }
    // SAFETY: Ryn Guard keeps owned handles live while an operation borrows them.
    &unsafe { &*pointer }.0
}

pub(crate) fn copy_owned(pointer: *const RynString) -> String {
    owned(pointer).clone()
}

pub(crate) fn compare_owned(left: *const RynString, right: *const RynString) -> std::cmp::Ordering {
    owned(left).cmp(owned(right))
}

pub(crate) fn map_hash(pointer: *const RynString) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in owned(pointer).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn map_equals(left: *const RynString, right: *const RynString) -> bool {
    owned(left) == owned(right)
}

fn owned_mut<'a>(pointer: *mut RynString) -> &'a mut String {
    if pointer.is_null() {
        fail("use of a moved String");
    }
    // SAFETY: Mutable String methods require a mutable root and cannot retain a borrow.
    &mut unsafe { &mut *pointer }.0
}

fn allocate(value: String) -> *mut RynString {
    #[cfg(ryn_runtime_debug)]
    LIVE_STRINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Box::into_raw(Box::new(RynString(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_new(pointer: *const u8, length: u64) -> *mut RynString {
    allocate(text(pointer, length).to_owned())
}

fn display_number(value: impl std::fmt::Display) -> *mut RynString {
    allocate(value.to_string())
}

fn parse_number<T: std::str::FromStr>(pointer: *const RynString, label: &str) -> T {
    owned(pointer)
        .parse()
        .unwrap_or_else(|_| fail(&format!("String is not a valid {label}")))
}

macro_rules! numeric_string_conversions {
    ($from:ident, $to:ident, $ty:ty, $label:literal) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $from(value: $ty) -> *mut RynString {
            display_number(value)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $to(pointer: *const RynString) -> $ty {
            parse_number(pointer, $label)
        }
    };
}

numeric_string_conversions!(ryn_string_from_i8, ryn_string_to_i8, i8, "i8 value");
numeric_string_conversions!(ryn_string_from_i16, ryn_string_to_i16, i16, "i16 value");
numeric_string_conversions!(ryn_string_from_i32, ryn_string_to_i32, i32, "i32 value");
numeric_string_conversions!(ryn_string_from_i64, ryn_string_to_i64, i64, "i64 value");
numeric_string_conversions!(ryn_string_from_u8, ryn_string_to_u8, u8, "u8 value");
numeric_string_conversions!(ryn_string_from_u16, ryn_string_to_u16, u16, "u16 value");
numeric_string_conversions!(ryn_string_from_u32, ryn_string_to_u32, u32, "u32 value");
numeric_string_conversions!(ryn_string_from_u64, ryn_string_to_u64, u64, "u64 value");
numeric_string_conversions!(ryn_string_from_f32, ryn_string_to_f32, f32, "f32 value");
numeric_string_conversions!(ryn_string_from_f64, ryn_string_to_f64, f64, "f64 value");

fn option_scalar(value: Option<u64>) -> *mut super::enums::RynEnum {
    match value {
        Some(value) => {
            let payload = value.to_ne_bytes();
            super::enums::ryn_enum_new(0, payload.as_ptr(), 8, std::ptr::null(), 0)
        }
        None => super::enums::ryn_enum_new(1, std::ptr::null(), 0, std::ptr::null(), 0),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_try_parse(
    pointer: *const RynString,
    type_tag: u32,
) -> *mut super::enums::RynEnum {
    let source = owned(pointer);
    let value = match type_tag {
        0 => source.parse::<i8>().ok().map(|value| value as i64 as u64),
        1 => source.parse::<i16>().ok().map(|value| value as i64 as u64),
        2 => source.parse::<i32>().ok().map(|value| value as i64 as u64),
        3 => source.parse::<i64>().ok().map(|value| value as u64),
        4 => source.parse::<u8>().ok().map(u64::from),
        5 => source.parse::<u16>().ok().map(u64::from),
        6 => source.parse::<u32>().ok().map(u64::from),
        7 => source.parse::<u64>().ok(),
        8 => source
            .parse::<f32>()
            .ok()
            .map(|value| u64::from(value.to_bits())),
        9 => source.parse::<f64>().ok().map(f64::to_bits),
        _ => fail("unknown String numeric parse type"),
    };
    option_scalar(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_clone(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).clone())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_drop(pointer: *mut RynString) {
    if !pointer.is_null() {
        // SAFETY: The compiler clears a handle after a move/drop and destroys each owner once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_print(pointer: *const RynString) {
    let _ = std::io::stdout().write_all(owned(pointer).as_bytes());
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_equals(left: *const RynString, right: *const RynString) -> i8 {
    i8::from(owned(left) == owned(right))
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_len(pointer: *const RynString) -> u64 {
    owned(pointer).len() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_data(pointer: *const RynString) -> *const u8 {
    owned(pointer).as_ptr()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_char_count(pointer: *const RynString) -> u64 {
    owned(pointer).chars().count() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_str_char_count(pointer: *const u8, length: u64) -> u64 {
    text(pointer, length).chars().count() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_byte_at(pointer: *const RynString, index: u64) -> u8 {
    usize::try_from(index)
        .ok()
        .and_then(|index| owned(pointer).as_bytes().get(index))
        .copied()
        .unwrap_or_else(|| fail("String byte index is out of bounds"))
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_char_at(pointer: *const RynString, index: u64) -> u32 {
    usize::try_from(index)
        .ok()
        .and_then(|index| owned(pointer).chars().nth(index))
        .unwrap_or_else(|| fail("String character index is out of bounds")) as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_str_char_at(pointer: *const u8, length: u64, index: u64) -> u32 {
    usize::try_from(index)
        .ok()
        .and_then(|index| text(pointer, length).chars().nth(index))
        .unwrap_or_else(|| fail("str character index is out of bounds")) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_char_from_u32(value: u32) -> u32 {
    char::from_u32(value).unwrap_or_else(|| fail("integer is not a valid Unicode scalar value"))
        as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_slice(
    pointer: *const RynString,
    start: u64,
    end: u64,
) -> *mut RynString {
    let slice = usize::try_from(start)
        .ok()
        .zip(usize::try_from(end).ok())
        .and_then(|(start, end)| owned(pointer).get(start..end))
        .unwrap_or_else(|| fail("String slice must use ordered in-bounds UTF-8 byte boundaries"));
    allocate(slice.to_owned())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_slice_chars(
    pointer: *const RynString,
    start: u64,
    end: u64,
) -> *mut RynString {
    let value = owned(pointer);
    let start = usize::try_from(start).unwrap_or_else(|_| fail("String character index overflows"));
    let end = usize::try_from(end).unwrap_or_else(|_| fail("String character index overflows"));
    let char_count = value.chars().count();
    if start > end || end > char_count {
        fail("String character slice must use ordered in-bounds Unicode scalar indices");
    }
    let byte_offset = |index: usize| {
        if index == char_count {
            Some(value.len())
        } else {
            value.char_indices().nth(index).map(|(offset, _)| offset)
        }
    };
    let begin =
        byte_offset(start).unwrap_or_else(|| fail("String character slice start is out of bounds"));
    let finish =
        byte_offset(end).unwrap_or_else(|| fail("String character slice end is out of bounds"));
    allocate(value[begin..finish].to_owned())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_trim(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).trim().to_owned())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_is_empty(pointer: *const RynString) -> bool {
    owned(pointer).is_empty()
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_trim_start(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).trim_start().to_owned())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_trim_end(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).trim_end().to_owned())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_to_lower(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).to_lowercase())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_to_upper(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).to_uppercase())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_replace_str(
    pointer: *const RynString,
    from: *const u8,
    from_len: u64,
    to: *const u8,
    to_len: u64,
) -> *mut RynString {
    allocate(owned(pointer).replace(text(from, from_len), text(to, to_len)))
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_lines(pointer: *const RynString) -> *mut super::vectors::RynVec {
    let lines = owned(pointer)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let result = super::vectors::ryn_vec_new(
        1,
        Some(ryn_vec_elem_string_drop),
        Some(ryn_vec_elem_string_clone),
    );
    for line in lines {
        let item = allocate(line);
        unsafe { super::vectors::ryn_vec_push(result, (&item as *const *mut RynString).cast()) };
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_chars(pointer: *const RynString) -> *mut super::vectors::RynVec {
    let result = super::vectors::ryn_vec_new(1, None, None);
    for character in owned(pointer).chars() {
        let word = u64::from(character as u32);
        unsafe { super::vectors::ryn_vec_push(result, (&word as *const u64).cast()) };
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_bytes(pointer: *const RynString) -> *mut super::vectors::RynVec {
    let result = super::vectors::ryn_vec_new(1, None, None);
    for byte in owned(pointer).bytes() {
        let word = u64::from(byte);
        unsafe { super::vectors::ryn_vec_push(result, (&word as *const u64).cast()) };
    }
    result
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_repeat(pointer: *const RynString, count: u64) -> *mut RynString {
    let value = owned(pointer);
    let count = usize::try_from(count)
        .unwrap_or_else(|_| fail("String repeat count exceeds the host address space"));
    let capacity = value
        .len()
        .checked_mul(count)
        .unwrap_or_else(|| fail("repeated String is too large"));
    let mut repeated = String::new();
    repeated
        .try_reserve_exact(capacity)
        .unwrap_or_else(|_| fail("cannot allocate repeated String"));
    for _ in 0..count {
        repeated.push_str(value);
    }
    allocate(repeated)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_reverse(pointer: *const RynString) -> *mut RynString {
    allocate(owned(pointer).chars().rev().collect())
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_clear(pointer: *mut RynString) {
    owned_mut(pointer).clear();
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_push(pointer: *mut RynString, character: u32) {
    let character =
        char::from_u32(character).unwrap_or_else(|| fail("invalid Unicode scalar value"));
    owned_mut(pointer).push(character);
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_append_str(pointer: *mut RynString, suffix: *const u8, length: u64) {
    owned_mut(pointer).push_str(text(suffix, length));
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_append_string(pointer: *mut RynString, suffix: *const RynString) {
    if std::ptr::eq(pointer, suffix) {
        // Snapshot before mutating: `value.append(value)` may reallocate its own buffer.
        let suffix = owned(suffix).clone();
        owned_mut(pointer).push_str(&suffix);
    } else {
        owned_mut(pointer).push_str(owned(suffix));
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_concat_str(
    pointer: *const RynString,
    suffix: *const u8,
    length: u64,
) -> *mut RynString {
    let mut value = owned(pointer).clone();
    value.push_str(text(suffix, length));
    allocate(value)
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_concat_string(
    pointer: *const RynString,
    suffix: *const RynString,
) -> *mut RynString {
    let mut value = owned(pointer).clone();
    value.push_str(owned(suffix));
    allocate(value)
}
macro_rules! predicate {
    ($str_name:ident, $string_name:ident, $method:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $str_name(
            pointer: *const RynString,
            pattern: *const u8,
            length: u64,
        ) -> i8 {
            i8::from(owned(pointer).$method(text(pattern, length)))
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn $string_name(pointer: *const RynString, pattern: *const RynString) -> i8 {
            i8::from(owned(pointer).$method(owned(pattern).as_str()))
        }
    };
}
predicate!(
    ryn_string_starts_with_str,
    ryn_string_starts_with_string,
    starts_with
);
predicate!(
    ryn_string_ends_with_str,
    ryn_string_ends_with_string,
    ends_with
);
predicate!(
    ryn_string_contains_str,
    ryn_string_contains_string,
    contains
);

fn find_character_index(value: &str, needle: &str) -> i64 {
    value
        .find(needle)
        .and_then(|byte_index| i64::try_from(value[..byte_index].chars().count()).ok())
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_find_str(
    pointer: *const RynString,
    pattern: *const u8,
    length: u64,
) -> i64 {
    find_character_index(owned(pointer), text(pattern, length))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_find_string(
    pointer: *const RynString,
    pattern: *const RynString,
) -> i64 {
    find_character_index(owned(pointer), owned(pattern))
}

fn split_to_vec(value: &str, separator: &str) -> *mut super::vectors::RynVec {
    let result = super::vectors::ryn_vec_new(
        1,
        Some(ryn_vec_elem_string_drop),
        Some(ryn_vec_elem_string_clone),
    );
    for part in value.split(separator) {
        let item = allocate(part.to_owned());
        // SAFETY: Vec<String> stores one owning String handle per 8-byte element;
        // pushing transfers this newly allocated handle to the vector.
        unsafe {
            super::vectors::ryn_vec_push(result, (&item as *const *mut RynString).cast());
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_split_str(
    pointer: *const RynString,
    pattern: *const u8,
    length: u64,
) -> *mut super::vectors::RynVec {
    split_to_vec(owned(pointer), text(pattern, length))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_string_split_string(
    pointer: *const RynString,
    pattern: *const RynString,
) -> *mut super::vectors::RynVec {
    split_to_vec(owned(pointer), owned(pattern))
}

pub(crate) unsafe extern "C" fn ryn_vec_elem_string_drop(elem: *mut u8) {
    let pointer = unsafe { std::ptr::read(elem as *const *mut RynString) };
    ryn_string_drop(pointer);
}

pub(crate) unsafe extern "C" fn ryn_vec_elem_string_clone(source: *const u8, elem: *mut u8) {
    let value = unsafe { std::ptr::read(source as *const *mut RynString) };
    unsafe { std::ptr::write(elem as *mut *mut RynString, ryn_string_clone(value)) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_string_element(drop_out: *mut usize, clone_out: *mut usize) {
    unsafe {
        *drop_out = ryn_vec_elem_string_drop as usize;
        *clone_out = ryn_vec_elem_string_clone as usize;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_print_char(character: u32) {
    let character =
        char::from_u32(character).unwrap_or_else(|| fail("invalid Unicode scalar value"));
    let _ = write!(std::io::stdout().lock(), "{character}");
}
