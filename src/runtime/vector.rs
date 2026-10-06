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
    value.data.iter().map(|item| {
        let pointer = *item as usize as *const super::strings::RynString;
        super::strings::copy_owned(pointer)
    }).collect()
}

#[cfg(ryn_runtime_debug)]
static LIVE_VECTORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(ryn_runtime_debug)]
pub fn live_vectors() -> usize { LIVE_VECTORS.load(std::sync::atomic::Ordering::Relaxed) }

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}
fn vector<'a>(pointer: *const RynVec) -> &'a RynVec {
    if pointer.is_null() { fail("use of a moved Vec"); }
    // SAFETY: Ryn Guard keeps owning handles live for the duration of a borrow.
    unsafe { &*pointer }
}
fn vector_mut<'a>(pointer: *mut RynVec) -> &'a mut RynVec {
    if pointer.is_null() { fail("use of a moved Vec"); }
    // SAFETY: Mutating Vec operations require a mutable root and an exclusive expression borrow.
    unsafe { &mut *pointer }
}
impl RynVec {
    fn len(&self) -> usize { self.data.len() / self.stride }
    fn position(&self, index: u64) -> usize {
        usize::try_from(index).ok().filter(|index| *index < self.len())
            .unwrap_or_else(|| fail("Vec index is out of bounds")) * self.stride
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
pub extern "C" fn ryn_vec_new(stride: u64, drop_element: Option<DropElement>, clone_element: Option<CloneElement>) -> *mut RynVec {
    let stride = usize::try_from(stride).ok().filter(|stride| *stride > 0 && *stride <= isize::MAX as usize / 8)
        .unwrap_or_else(|| fail("invalid Vec element layout"));
    allocate_vec(RynVec { data: Vec::new(), stride, drop_element, clone_element })
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_drop(pointer: *mut RynVec) {
    if !pointer.is_null() {
        // SAFETY: Moves clear source handles, so each Vec owner is destroyed once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_len(pointer: *const RynVec) -> u64 { vector(pointer).len() as u64 }
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_slice(
    pointer: *const RynVec,
    start: u64,
    end: u64,
    output_length: *mut u64,
) -> *const u8 {
    if output_length.is_null() { fail("null Vec slice length output"); }
    let value = vector(pointer);
    let len = value.len();
    let start = usize::try_from(start).ok().filter(|index| *index <= len)
        .unwrap_or_else(|| fail("Vec slice start is out of bounds"));
    let end = usize::try_from(end).ok().filter(|index| *index >= start && *index <= len)
        .unwrap_or_else(|| fail("Vec slice end is out of bounds"));
    // SAFETY: The immutable descriptor points into the live Vec allocation; Ryn Guard
    // holds an expression borrow until the receiving function call completes.
    unsafe { output_length.write((end - start) as u64) };
    // SAFETY: `start` is at most the initialized element count and the pointer remains
    // valid while the source Vec is borrowed.
    unsafe { value.data.as_ptr().add(start * value.stride).cast() }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_capacity(pointer: *const RynVec) -> u64 { (vector(pointer).data.capacity() / vector(pointer).stride) as u64 }
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_reserve(pointer: *mut RynVec, additional: u64) {
    let value = vector_mut(pointer);
    let additional = usize::try_from(additional).ok().and_then(|count| count.checked_mul(value.stride))
        .unwrap_or_else(|| fail("Vec capacity exceeds the host address space"));
    value.data.try_reserve(additional).unwrap_or_else(|_| fail("cannot allocate Vec capacity"));
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_clear(pointer: *mut RynVec) { vector_mut(pointer).clear(); }
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_push(pointer: *mut RynVec, element: *const u8) {
    if element.is_null() { fail("null Vec element"); }
    let value = vector_mut(pointer);
    let start = value.data.len();
    let end = start.checked_add(value.stride).unwrap_or_else(|| fail("Vec length overflow"));
    value.data.try_reserve(value.stride).unwrap_or_else(|_| fail("cannot grow Vec"));
    value.data.resize(end, 0);
    // SAFETY: Source is a compiler-owned stack value with exactly stride * 8 bytes.
    unsafe { ptr::copy_nonoverlapping(element, value.data.as_mut_ptr().add(start).cast(), value.stride * 8) };
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
    if output.is_null() { fail("null Vec output buffer"); }
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: Ownership transfers to the output; draining u64 storage does not destroy the element.
    unsafe { ptr::copy_nonoverlapping(value.data.as_ptr().add(position).cast(), output, value.stride * 8) };
    value.data.drain(position..position + value.stride);
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_set(pointer: *mut RynVec, index: u64, element: *const u8) {
    if element.is_null() { fail("null Vec element"); }
    let value = vector_mut(pointer);
    let position = value.position(index);
    // SAFETY: The old element is destroyed before a separately evaluated stack value replaces it.
    unsafe {
        let destination = value.data.as_mut_ptr().add(position).cast();
        if let Some(drop_element) = value.drop_element { drop_element(destination); }
        ptr::copy_nonoverlapping(element, destination, value.stride * 8);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_extract(pointer: *mut RynVec, index: u64, output: *mut u8) {
    if output.is_null() { fail("null Vec output buffer"); }
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
    let mut cloned = RynVec { data: value.data.clone(), stride: value.stride,
        drop_element: value.drop_element, clone_element: value.clone_element };
    if let Some(clone_element) = value.clone_element {
        for index in 0..value.len() {
            // SAFETY: The callback replaces each copied owning handle with an independent clone.
            unsafe { clone_element(value.data.as_ptr().add(index * value.stride).cast(), cloned.data.as_mut_ptr().add(index * value.stride).cast()) };
        }
    }
    allocate_vec(cloned)
}
