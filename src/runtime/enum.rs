use std::{io::Write, slice};

type DropFn = unsafe extern "C" fn(*mut u8);
type CloneFn = unsafe extern "C" fn(*const u8) -> *mut u8;

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}

/// One entry in the static drop plan for an enum type.
///
/// `kind` selects the runtime free function for the owned pointer stored in
/// payload word `offset` of variant `variant`.
#[repr(C)]
pub struct EnumDropEntry {
    pub variant: u64,
    pub offset: u64,
    pub kind: u64,
}

pub const DROP_STRING: u64 = 0;
pub const DROP_VEC: u64 = 1;
pub const DROP_ENUM: u64 = 2;
pub const DROP_MAP: u64 = 3;

pub struct RynEnum {
    tag: i64,
    payload: Vec<u8>,
    drops: Vec<EnumDropEntry>,
}

fn drop_fn(kind: u64) -> DropFn {
    match kind {
        DROP_STRING => {
            // SAFETY: ABI-compatible transmute of the exported RynString drop.
            unsafe {
                std::mem::transmute::<usize, DropFn>(super::strings::ryn_string_drop as usize)
            }
        }
        DROP_VEC => {
            // SAFETY: ABI-compatible transmute of the exported RynVec drop.
            unsafe { std::mem::transmute::<usize, DropFn>(super::vectors::ryn_vec_drop as usize) }
        }
        DROP_ENUM => {
            // SAFETY: Enum payload words store owning RynEnum handles.
            unsafe { std::mem::transmute::<usize, DropFn>(ryn_enum_drop as usize) }
        }
        DROP_MAP => {
            // SAFETY: Enum payload words store owning RynMap handles.
            unsafe { std::mem::transmute::<usize, DropFn>(super::maps::ryn_map_drop as usize) }
        }
        _ => fail("unknown enum drop kind"),
    }
}

fn clone_fn(kind: u64) -> CloneFn {
    match kind {
        DROP_STRING => unsafe {
            std::mem::transmute::<usize, CloneFn>(super::strings::ryn_string_clone as usize)
        },
        DROP_VEC => unsafe {
            std::mem::transmute::<usize, CloneFn>(super::vectors::ryn_vec_clone as usize)
        },
        DROP_ENUM => unsafe { std::mem::transmute::<usize, CloneFn>(ryn_enum_clone as usize) },
        DROP_MAP => unsafe {
            std::mem::transmute::<usize, CloneFn>(super::maps::ryn_map_clone as usize)
        },
        _ => fail("unknown enum clone kind"),
    }
}

impl RynEnum {
    fn word(&self, index: usize) -> u64 {
        let start = index * 8;
        if start + 8 > self.payload.len() {
            fail("enum payload word is out of bounds");
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.payload[start..start + 8]);
        u64::from_ne_bytes(bytes)
    }
    fn write_word(&mut self, index: usize, value: u64) {
        let start = index * 8;
        if start + 8 > self.payload.len() {
            fail("enum payload word is out of bounds");
        }
        self.payload[start..start + 8].copy_from_slice(&value.to_ne_bytes());
    }
}

impl Drop for RynEnum {
    fn drop(&mut self) {
        let tag = self.tag as u64;
        for entry in &self.drops {
            if entry.variant == tag {
                let word = self.word(entry.offset as usize) as *mut u8;
                if word.is_null() {
                    continue;
                }
                // SAFETY: The word stores an owning handle whose drop is registered.
                unsafe { drop_fn(entry.kind)(word) };
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_new(
    tag: i64,
    payload: *const u8,
    payload_len: u64,
    drops: *const u8,
    drops_len: u64,
) -> *mut RynEnum {
    let payload = if payload_len == 0 {
        Vec::new()
    } else {
        if payload.is_null() {
            fail("null enum payload");
        }
        usize::try_from(payload_len)
            .ok()
            .filter(|len| *len <= isize::MAX as usize)
            .map(|len| unsafe { slice::from_raw_parts(payload, len) }.to_vec())
            .unwrap_or_else(|| fail("enum payload exceeds the host address space"))
    };
    // The drop plan is a byte buffer of 24-byte entries: variant | offset | kind.
    let mut drops_list = Vec::new();
    if drops_len > 0 {
        if drops.is_null() {
            fail("null enum drop plan");
        }
        let len = usize::try_from(drops_len)
            .ok()
            .filter(|len| *len <= isize::MAX as usize && *len % 24 == 0)
            .unwrap_or_else(|| fail("enum drop plan has an invalid length"));
        // SAFETY: drops points to `len` bytes of a static plan.
        let raw = unsafe { slice::from_raw_parts(drops, len) };
        for chunk in raw.chunks_exact(24) {
            let variant = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
            let offset = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
            let kind = u64::from_le_bytes(chunk[16..24].try_into().unwrap());
            drops_list.push(EnumDropEntry {
                variant,
                offset,
                kind,
            });
        }
    }
    Box::into_raw(Box::new(RynEnum {
        tag,
        payload,
        drops: drops_list,
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_clone(pointer: *const RynEnum) -> *mut RynEnum {
    if pointer.is_null() {
        fail("cannot clone a moved enum");
    }
    // SAFETY: The compiler passes a live owning enum handle while borrowing it.
    let source = unsafe { &*pointer };
    let mut cloned = RynEnum {
        tag: source.tag,
        payload: source.payload.clone(),
        drops: source
            .drops
            .iter()
            .map(|entry| EnumDropEntry {
                variant: entry.variant,
                offset: entry.offset,
                kind: entry.kind,
            })
            .collect(),
    };
    let active_tag = cloned.tag as u64;
    let active_owners = cloned
        .drops
        .iter()
        .filter(|entry| entry.variant == active_tag)
        .map(|entry| (entry.offset, entry.kind))
        .collect::<Vec<_>>();
    for (offset, kind) in active_owners {
        let offset = usize::try_from(offset)
            .ok()
            .unwrap_or_else(|| fail("enum payload offset exceeds the host address space"));
        let source_pointer = cloned.word(offset) as *const u8;
        if !source_pointer.is_null() {
            // SAFETY: The active variant drop plan identifies the owning payload type.
            let owned_copy = unsafe { clone_fn(kind)(source_pointer) };
            cloned.write_word(offset, owned_copy as usize as u64);
        }
    }
    Box::into_raw(Box::new(cloned))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_tag(pointer: *const RynEnum) -> i64 {
    if pointer.is_null() {
        fail("use of a moved value");
    }
    // SAFETY: The compiler keeps owning handles live.
    unsafe { (*pointer).tag }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_word(pointer: *mut RynEnum, index: u64) -> u64 {
    if pointer.is_null() {
        fail("use of a moved value");
    }
    let index = usize::try_from(index).unwrap_or_else(|_| fail("enum word index overflows"));
    // SAFETY: The compiler keeps owning handles live.
    unsafe { (*pointer).word(index) }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_word_ptr(pointer: *mut RynEnum, index: u64) -> *mut u8 {
    if pointer.is_null() {
        fail("use of a moved value");
    }
    let index = usize::try_from(index).unwrap_or_else(|_| fail("enum word index overflows"));
    // SAFETY: The compiler keeps owning handles live.
    let word = unsafe { (*pointer).word(index) };
    word as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_clear_word(pointer: *mut RynEnum, index: u64) {
    if pointer.is_null() {
        fail("use of a moved value");
    }
    let index = usize::try_from(index).unwrap_or_else(|_| fail("enum word index overflows"));
    // SAFETY: The compiler keeps owning handles live.
    unsafe { (*pointer).write_word(index, 0) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_enum_drop(pointer: *mut RynEnum) {
    if !pointer.is_null() {
        // SAFETY: Moves clear source handles, so each RynEnum owner is destroyed once.
        drop(unsafe { Box::from_raw(pointer) });
    }
}
