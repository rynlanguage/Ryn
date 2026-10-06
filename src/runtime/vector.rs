use std::{io::Write, ptr};

pub(crate) type DropElement = unsafe extern "C" fn(*mut u8);
pub(crate) type CloneElement = unsafe extern "C" fn(*const u8, *mut u8);

pub struct RynVec {
    data: Vec<u64>,
    stride: usize,
    drop_element: Option<DropElement>,
    clone_element: Option<CloneElement>,
}

pub(crate) fn copy_string_items(pointer: *const RynVec) -> Vec<String> {
    let value = vector(pointer);
    if value.stride != 1 || value.clone_element.is_none() {
        fail("process arguments require Vec<String>");
    }
    value
        .data
        .iter()
        .map(|item| {
            let pointer = *item as usize as *const super::strings::RynString;
            super::strings::copy_owned(pointer)
        })
        .collect()
}

#[cfg(ryn_runtime_debug)]
static LIVE_VECTORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(ryn_runtime_debug)]
pub fn live_vectors() -> usize {
    LIVE_VECTORS.load(std::sync::atomic::Ordering::Relaxed)
}

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}
fn vector<'a>(pointer: *const RynVec) -> &'a RynVec {
    if pointer.is_null() {
        fail("use of a moved Vec");
    }
    // SAFETY: Ryn Guard keeps owning handles live for the duration of a borrow.
    unsafe { &*pointer }
}
fn vector_mut<'a>(pointer: *mut RynVec) -> &'a mut RynVec {
    if pointer.is_null() {
        fail("use of a moved Vec");
    }
    // SAFETY: Mutating Vec operations require a mutable root and an exclusive expression borrow.
    unsafe { &mut *pointer }
}
impl RynVec {
    fn len(&self) -> usize {
        self.data.len() / self.stride
    }
    fn position(&self, index: u64) -> usize {
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.len())
            .unwrap_or_else(|| fail("Vec index is out of bounds"))
            * self.stride
    }
    fn clear(&mut self) {
        if let Some(drop_element) = self.drop_element {
            for index in (0..self.len()).rev() {
                // SAFETY: The compiler-generated callback matches this Vec's element layout.
                unsafe { drop_element(self.data.as_mut_ptr().add(index * self.stride).cast()) };
            }
        }
        self.data.clear();
    }
}
impl Drop for RynVec {
    fn drop(&mut self) {
        self.clear();
        #[cfg(ryn_runtime_debug)]
        LIVE_VECTORS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}
pub(crate) fn allocate_vec(value: RynVec) -> *mut RynVec {
    #[cfg(ryn_runtime_debug)]
    LIVE_VECTORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Box::into_raw(Box::new(value))
}

/// Builds an empty Vec for runtime-internal producers such as Map iteration.
/// The callbacks follow the same ABI as compiler-provided element glue.
pub(crate) fn build_vec(
    stride: usize,
    drop_element: Option<DropElement>,
    clone_element: Option<CloneElement>,
) -> RynVec {
    RynVec {
        data: Vec::new(),
        stride,
        drop_element,
        clone_element,
    }
}

impl RynVec {
    pub(crate) fn push_words(&mut self, words: &[u64]) {
        assert_eq!(words.len(), self.stride, "Vec element word count mismatch");
        self.data.extend_from_slice(words);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_new(
    stride: u64,
    drop_element: Option<DropElement>,
    clone_element: Option<CloneElement>,
) -> *mut RynVec {
    let stride = usize::try_from(stride)
        .ok()
        .filter(|stride| *stride > 0 && *stride <= isize::MAX as usize / 8)
        .unwrap_or_else(|| fail("invalid Vec element layout"));
    allocate_vec(RynVec {
        data: Vec::new(),
        stride,
        drop_element,
        clone_element,
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_drop(pointer: *mut RynVec) {
    if !pointer.is_null() {
        // SAFETY: Moves clear source handles, so each Vec owner is destroyed once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_len(pointer: *const RynVec) -> u64 {
    vector(pointer).len() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_is_empty(pointer: *const RynVec) -> bool {
    vector(pointer).len() == 0
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_slice(
    pointer: *const RynVec,
    start: u64,
    end: u64,
    output_length: *mut u64,
) -> *const u8 {
    if output_length.is_null() {
        fail("null Vec slice length output");
    }
    let value = vector(pointer);
    let len = value.len();
    let start = usize::try_from(start)
        .ok()
        .filter(|index| *index <= len)
        .unwrap_or_else(|| fail("Vec slice start is out of bounds"));
    let end = usize::try_from(end)
        .ok()
        .filter(|index| *index >= start && *index <= len)
        .unwrap_or_else(|| fail("Vec slice end is out of bounds"));
    // SAFETY: The immutable descriptor points into the live Vec allocation; Ryn Guard
    // holds an expression borrow until the receiving function call completes.
    unsafe { output_length.write((end - start) as u64) };
    // SAFETY: `start` is at most the initialized element count and the pointer remains
    // valid while the source Vec is borrowed.
    unsafe { value.data.as_ptr().add(start * value.stride).cast() }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_capacity(pointer: *const RynVec) -> u64 {
    (vector(pointer).data.capacity() / vector(pointer).stride) as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_reserve(pointer: *mut RynVec, additional: u64) {
    let value = vector_mut(pointer);
    let additional = usize::try_from(additional)
        .ok()
        .and_then(|count| count.checked_mul(value.stride))
        .unwrap_or_else(|| fail("Vec capacity exceeds the host address space"));
    value
        .data
        .try_reserve(additional)
        .unwrap_or_else(|_| fail("cannot allocate Vec capacity"));
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_clear(pointer: *mut RynVec) {
    vector_mut(pointer).clear();
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_push(pointer: *mut RynVec, element: *const u8) {
    if element.is_null() {
        fail("null Vec element");
    }
    let value = vector_mut(pointer);
    let start = value.data.len();
    let end = start
        .checked_add(value.stride)
        .unwrap_or_else(|| fail("Vec length overflow"));
    value
        .data
        .try_reserve(value.stride)
        .unwrap_or_else(|_| fail("cannot grow Vec"));
    value.data.resize(end, 0);
    // SAFETY: Source is a compiler-owned stack value with exactly stride * 8 bytes.
    unsafe {
        ptr::copy_nonoverlapping(
            element,
            value.data.as_mut_ptr().add(start).cast(),
            value.stride * 8,
        )
    };
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_insert(pointer: *mut RynVec, index: u64, element: *const u8) {
    if element.is_null() {
        fail("null Vec element");
    }
    let value = vector_mut(pointer);
    let index = usize::try_from(index)
        .ok()
        .filter(|index| *index <= value.len())
        .unwrap_or_else(|| fail("Vec insert index is out of bounds"));
    let mut words = vec![0_u64; value.stride];
    // SAFETY: The compiler supplies an initialized element slot with exactly stride words.
    unsafe { ptr::copy_nonoverlapping(element, words.as_mut_ptr().cast(), value.stride * 8) };
    value
        .data
        .try_reserve(value.stride)
        .unwrap_or_else(|_| fail("cannot grow Vec"));
    value
        .data
        .splice(index * value.stride..index * value.stride, words);
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_reverse(pointer: *mut RynVec) {
    let value = vector_mut(pointer);
    let len = value.len();
    for left in 0..(len / 2) {
        let right_start = (len - left - 1) * value.stride;
        let left_start = left * value.stride;
        let (before_right, from_right) = value.data.split_at_mut(right_start);
        before_right[left_start..left_start + value.stride]
            .swap_with_slice(&mut from_right[..value.stride]);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_sort(pointer: *mut RynVec, kind: u64) {
    let value = vector_mut(pointer);
    let string_sort = kind == 12 && value.stride == 1 && value.drop_element.is_some();
    if !string_sort && (value.drop_element.is_some() || value.clone_element.is_some()) {
        fail("Vec.sort currently supports only scalar elements");
    }
    if !matches!(kind, 0..=9 | 11 | 12) {
        fail("unsupported Vec.sort element type");
    }
    let stride = value.stride;
    if stride != 1 {
        fail("Vec.sort currently supports only one-word scalar elements");
    }
    // Comparisons use type-specific interpretation while storage remains plain words.
    value.data.sort_by(|left, right| {
        let left = left.to_ne_bytes();
        let right = right.to_ne_bytes();
        match kind {
            0 => (left[0] as i8).cmp(&(right[0] as i8)),
            1 => left[0].cmp(&right[0]),
            2 => (u16::from_ne_bytes([left[0], left[1]]) as i16)
                .cmp(&(u16::from_ne_bytes([right[0], right[1]]) as i16)),
            3 => u16::from_ne_bytes([left[0], left[1]])
                .cmp(&u16::from_ne_bytes([right[0], right[1]])),
            4 => (u32::from_ne_bytes(left[..4].try_into().unwrap()) as i32)
                .cmp(&(u32::from_ne_bytes(right[..4].try_into().unwrap()) as i32)),
            5 => u32::from_ne_bytes(left[..4].try_into().unwrap())
                .cmp(&u32::from_ne_bytes(right[..4].try_into().unwrap())),
            6 => (i64::from_ne_bytes(left) as i64).cmp(&(i64::from_ne_bytes(right) as i64)),
            7 => left.cmp(&right),
            8 => f32::from_bits(u32::from_ne_bytes(left[..4].try_into().unwrap())).total_cmp(
                &f32::from_bits(u32::from_ne_bytes(right[..4].try_into().unwrap())),
            ),
            9 => f64::from_bits(u64::from_ne_bytes(left))
                .total_cmp(&f64::from_bits(u64::from_ne_bytes(right))),
            11 => left[0].cmp(&right[0]),
            12 => super::strings::compare_owned(
                u64::from_ne_bytes(left) as usize as *const super::strings::RynString,
                u64::from_ne_bytes(right) as usize as *const super::strings::RynString,
            ),
            _ => unreachable!(),
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_contains(pointer: *const RynVec, element: *const u8, kind: u64) -> bool {
    if element.is_null() {
        fail("null Vec.contains element");
    }
    let value = vector(pointer);
    if value.stride != 1 {
        fail("Vec.contains currently supports one-word elements");
    }
    let needle = unsafe { ptr::read_unaligned(element.cast::<u64>()) };
    if kind == 12 {
        let needle = needle as usize as *const super::strings::RynString;
        return value.data.iter().any(|stored| {
            let stored = *stored as usize as *const super::strings::RynString;
            super::strings::map_equals(stored, needle)
        });
    }
    value.data.iter().any(|stored| match kind {
        8 => f32::from_bits(*stored as u32) == f32::from_bits(needle as u32),
        9 => f64::from_bits(*stored) == f64::from_bits(needle),
        0..=7 | 10..=11 => *stored == needle,
        _ => fail("unsupported Vec.contains element type"),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_get_option(
    pointer: *const RynVec,
    requested_index: u64,
    output: *mut u8,
) -> bool {
    if output.is_null() {
        fail("null Vec.get output buffer");
    }
    let value = vector(pointer);
    let len = value.len();
    let index = if requested_index == u64::MAX {
        match len.checked_sub(1) {
            Some(index) => index,
            None => return false,
        }
    } else {
        match usize::try_from(requested_index) {
            Ok(index) if index < len => index,
            _ => return false,
        }
    };
    let start = index * value.stride;
    let source = unsafe { value.data.as_ptr().add(start).cast::<u8>() };
    unsafe { ptr::copy_nonoverlapping(source, output, value.stride * 8) };
    if let Some(clone_element) = value.clone_element {
        unsafe { clone_element(source, output) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_pop_option(pointer: *mut RynVec, output: *mut u8) -> bool {
    if output.is_null() {
        fail("null Vec.pop output buffer");
    }
    let value = vector_mut(pointer);
    let Some(index) = value.len().checked_sub(1) else {
        return false;
    };
    let start = index * value.stride;
    let source = unsafe { value.data.as_mut_ptr().add(start).cast::<u8>() };
    unsafe { ptr::copy_nonoverlapping(source, output, value.stride * 8) };
    value.data.truncate(start);
    true
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_index(pointer: *mut RynVec, index: u64) -> *mut u8 {
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: Bounds were checked; indexing borrows storage until the current expression completes.
    unsafe { value.data.as_mut_ptr().add(position).cast() }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_take(pointer: *mut RynVec, index: u64, output: *mut u8) {
    if output.is_null() {
        fail("null Vec output buffer");
    }
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: Ownership transfers to the output; draining u64 storage does not destroy the element.
    unsafe {
        ptr::copy_nonoverlapping(
            value.data.as_ptr().add(position).cast(),
            output,
            value.stride * 8,
        )
    };
    value.data.drain(position..position + value.stride);
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_set(pointer: *mut RynVec, index: u64, element: *const u8) {
    if element.is_null() {
        fail("null Vec element");
    }
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: The old element is destroyed before a separately evaluated stack value replaces it.
    unsafe {
        let destination = value.data.as_mut_ptr().add(position).cast();
        if let Some(drop_element) = value.drop_element {
            drop_element(destination);
        }
        ptr::copy_nonoverlapping(element, destination, value.stride * 8);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_extract(pointer: *mut RynVec, index: u64, output: *mut u8) {
    if output.is_null() {
        fail("null Vec output buffer");
    }
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: Into-iteration visits each slot once; clearing it prevents repeated destruction.
    unsafe {
        let source = value.data.as_mut_ptr().add(position).cast::<u8>();
        ptr::copy_nonoverlapping(source, output, value.stride * 8);
        ptr::write_bytes(source, 0, value.stride * 8);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_clone(pointer: *const RynVec) -> *mut RynVec {
    let value = vector(pointer);
    let mut cloned = RynVec {
        data: value.data.clone(),
        stride: value.stride,
        drop_element: value.drop_element,
        clone_element: value.clone_element,
    };
    if let Some(clone_element) = value.clone_element {
        for index in 0..value.len() {
            // SAFETY: The callback replaces each copied owning handle with an independent clone.
            unsafe {
                clone_element(
                    value.data.as_ptr().add(index * value.stride).cast(),
                    cloned.data.as_mut_ptr().add(index * value.stride).cast(),
                )
            };
        }
    }
    allocate_vec(cloned)
}
