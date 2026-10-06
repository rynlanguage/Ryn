use std::{collections::BTreeMap, io::Write, ptr};

const KEY_STRING: u64 = 1;
const VALUE_STRING: u64 = 1;
const VALUE_VEC: u64 = 2;
const VALUE_MAP: u64 = 3;
const VALUE_CUSTOM: u64 = 4;
const VALUE_STRUCT: u64 = 5;
const VALUE_MOVE_ONLY_STRUCT: u64 = 6;

pub type MapDropValue = unsafe extern "C" fn(*mut u8);
pub type MapCloneValue = unsafe extern "C" fn(*const u8, *mut u8);

struct Entry {
    key: Vec<u64>,
    value: Vec<u64>,
}

pub struct RynMap {
    buckets: BTreeMap<u64, Vec<Entry>>,
    key_stride: usize,
    value_stride: usize,
    key_kind: u64,
    value_kind: u64,
    drop_custom_value: Option<MapDropValue>,
    clone_custom_value: Option<MapCloneValue>,
}

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}

fn map<'a>(pointer: *const RynMap) -> &'a RynMap {
    if pointer.is_null() {
        fail("use of a moved Map");
    }
    // SAFETY: Ryn Guard keeps the Map owner alive while a method borrows it.
    unsafe { &*pointer }
}

fn map_mut<'a>(pointer: *mut RynMap) -> &'a mut RynMap {
    if pointer.is_null() {
        fail("use of a moved Map");
    }
    // SAFETY: Mutating Map methods require a mutable binding and do not retain borrows.
    unsafe { &mut *pointer }
}

fn read_words(pointer: *const u8, stride: usize) -> Vec<u64> {
    if pointer.is_null() {
        fail("null Map key or value");
    }
    let mut words = vec![0_u64; stride];
    // SAFETY: Compiler-generated slots contain `stride` initialized 8-byte words.
    unsafe { ptr::copy_nonoverlapping(pointer, words.as_mut_ptr().cast(), stride * 8) };
    words
}

fn key_hash(key: &[u64], kind: u64) -> u64 {
    if kind == KEY_STRING {
        let pointer = key[0] as usize as *const super::strings::RynString;
        return super::strings::map_hash(pointer);
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for word in key {
        for byte in word.to_ne_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn keys_equal(left: &[u64], right: &[u64], kind: u64) -> bool {
    if kind == KEY_STRING {
        let left = left[0] as usize as *const super::strings::RynString;
        let right = right[0] as usize as *const super::strings::RynString;
        return super::strings::map_equals(left, right);
    }
    left == right
}

fn drop_word(value: &mut [u64], kind: u64, custom_drop: Option<MapDropValue>) {
    if kind == VALUE_CUSTOM {
        if value.first().copied().unwrap_or(0) == 0 {
            return;
        }
        let callback = custom_drop.unwrap_or_else(|| fail("missing custom Map drop callback"));
        unsafe { callback(value.as_mut_ptr().cast()) };
        value.fill(0);
        return;
    }
    if kind == VALUE_STRUCT {
        if let Some(callback) = custom_drop {
            unsafe { callback(value.as_mut_ptr().cast()) };
        }
        value.fill(0);
        return;
    }
    if kind == VALUE_MOVE_ONLY_STRUCT {
        let callback =
            custom_drop.unwrap_or_else(|| fail("missing move-only Map struct drop callback"));
        unsafe { callback(value.as_mut_ptr().cast()) };
        value.fill(0);
        return;
    }
    let pointer = value[0] as usize as *mut u8;
    if pointer.is_null() {
        return;
    }
    match kind {
        VALUE_STRING => super::strings::ryn_string_drop(pointer.cast()),
        VALUE_VEC => super::vectors::ryn_vec_drop(pointer.cast()),
        VALUE_MAP => super::maps::ryn_map_drop(pointer.cast()),
        _ => {}
    }
    value[0] = 0;
}

fn clone_value(value: &[u64], kind: u64, custom_clone: Option<MapCloneValue>) -> Vec<u64> {
    let mut cloned = value.to_vec();
    match kind {
        VALUE_STRING => {
            let pointer = value[0] as usize as *const super::strings::RynString;
            cloned[0] = super::strings::ryn_string_clone(pointer) as usize as u64;
        }
        VALUE_VEC => {
            let pointer = value[0] as usize as *const super::vectors::RynVec;
            cloned[0] = super::vectors::ryn_vec_clone(pointer) as usize as u64;
        }
        VALUE_MAP => {
            let pointer = value[0] as usize as *const RynMap;
            cloned[0] = ryn_map_clone(pointer) as usize as u64;
        }
        VALUE_CUSTOM => {
            let callback =
                custom_clone.unwrap_or_else(|| fail("missing custom Map clone callback"));
            unsafe { callback(value.as_ptr().cast(), cloned.as_mut_ptr().cast()) };
        }
        VALUE_MOVE_ONLY_STRUCT => fail("move-only custom Map structs cannot be cloned"),
        VALUE_STRUCT => {
            if let Some(callback) = custom_clone {
                unsafe { callback(value.as_ptr().cast(), cloned.as_mut_ptr().cast()) };
            }
        }
        _ => {}
    }
    cloned
}

fn drop_entry(
    entry: &mut Entry,
    key_kind: u64,
    value_kind: u64,
    custom_drop: Option<MapDropValue>,
) {
    drop_word(&mut entry.key, key_kind, None);
    drop_word(&mut entry.value, value_kind, custom_drop);
}

impl RynMap {
    fn clear(&mut self) {
        for bucket in self.buckets.values_mut() {
            for entry in bucket.iter_mut().rev() {
                drop_entry(
                    entry,
                    self.key_kind,
                    self.value_kind,
                    self.drop_custom_value,
                );
            }
        }
        self.buckets.clear();
    }
}

impl Drop for RynMap {
    fn drop(&mut self) {
        self.clear();
    }
}

fn allocate_map(value: RynMap) -> *mut RynMap {
    #[cfg(ryn_runtime_debug)]
    LIVE_MAPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Box::into_raw(Box::new(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_new(
    key_stride: u64,
    value_stride: u64,
    key_kind: u64,
    value_kind: u64,
    drop_custom_value: Option<MapDropValue>,
    clone_custom_value: Option<MapCloneValue>,
) -> *mut RynMap {
    let key_stride = usize::try_from(key_stride)
        .ok()
        .filter(|stride| *stride > 0 && *stride <= isize::MAX as usize / 8)
        .unwrap_or_else(|| fail("unsupported Map key layout"));
    let value_stride = usize::try_from(value_stride)
        .ok()
        .filter(|stride| *stride > 0 && *stride <= isize::MAX as usize / 8)
        .unwrap_or_else(|| fail("unsupported Map value layout"));
    if key_kind > KEY_STRING || value_kind > VALUE_MOVE_ONLY_STRUCT {
        fail("unsupported Map key or value operations");
    }
    if matches!(value_kind, VALUE_CUSTOM | VALUE_MOVE_ONLY_STRUCT)
        && (drop_custom_value.is_none() || clone_custom_value.is_none())
    {
        fail("custom Map values require generated callbacks");
    }
    allocate_map(RynMap {
        buckets: BTreeMap::new(),
        key_stride,
        value_stride,
        key_kind,
        value_kind,
        drop_custom_value,
        clone_custom_value,
    })
}

fn clone_map(value: &RynMap) -> RynMap {
    if matches!(value.value_kind, VALUE_CUSTOM | VALUE_MOVE_ONLY_STRUCT) {
        fail("custom Map values are move-only and cannot be cloned");
    }
    let buckets = value
        .buckets
        .iter()
        .map(|(hash, bucket)| {
            let entries = bucket
                .iter()
                .map(|entry| Entry {
                    key: clone_value(&entry.key, value.key_kind, None),
                    value: clone_value(&entry.value, value.value_kind, value.clone_custom_value),
                })
                .collect();
            (*hash, entries)
        })
        .collect();
    RynMap {
        buckets,
        key_stride: value.key_stride,
        value_stride: value.value_stride,
        key_kind: value.key_kind,
        value_kind: value.value_kind,
        drop_custom_value: value.drop_custom_value,
        clone_custom_value: value.clone_custom_value,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_clone(pointer: *const RynMap) -> *mut RynMap {
    allocate_map(clone_map(map(pointer)))
}

unsafe extern "C" fn ryn_vec_elem_map_drop(slot: *mut u8) {
    let pointer = unsafe { ptr::read(slot.cast::<*mut RynMap>()) };
    super::maps::ryn_map_drop(pointer);
}

unsafe extern "C" fn ryn_vec_elem_map_clone(source: *const u8, output: *mut u8) {
    let pointer = unsafe { ptr::read(source.cast::<*const RynMap>()) };
    unsafe { ptr::write(output.cast::<*mut RynMap>(), ryn_map_clone(pointer)) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_vec_map_element(drop_out: *mut usize, clone_out: *mut usize) {
    if drop_out.is_null() || clone_out.is_null() {
        fail("null Vec<Map> callback output");
    }
    unsafe {
        ptr::write(drop_out, ryn_vec_elem_map_drop as usize);
        ptr::write(clone_out, ryn_vec_elem_map_clone as usize);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_drop(pointer: *mut RynMap) {
    if !pointer.is_null() {
        // SAFETY: Moves clear the Map handle and each owner is destroyed exactly once.
        drop(unsafe { Box::from_raw(pointer) });
        #[cfg(ryn_runtime_debug)]
        LIVE_MAPS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_len(pointer: *const RynMap) -> u64 {
    map(pointer)
        .buckets
        .values()
        .map(|bucket| bucket.len())
        .sum::<usize>() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_is_empty(pointer: *const RynMap) -> bool {
    ryn_map_len(pointer) == 0
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_clear(pointer: *mut RynMap) {
    map_mut(pointer).clear();
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_contains_key(pointer: *const RynMap, key: *const u8) -> i8 {
    let map = map(pointer);
    let key = read_words(key, map.key_stride);
    map.buckets
        .get(&key_hash(&key, map.key_kind))
        .is_some_and(|bucket| {
            bucket
                .iter()
                .any(|entry| keys_equal(&entry.key, &key, map.key_kind))
        })
        .into()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_insert(pointer: *mut RynMap, key: *const u8, value: *const u8) -> i8 {
    let map = map_mut(pointer);
    let key = read_words(key, map.key_stride);
    let value = read_words(value, map.value_stride);
    let hash = key_hash(&key, map.key_kind);
    let bucket = map.buckets.entry(hash).or_default();
    if let Some(entry) = bucket
        .iter_mut()
        .find(|entry| keys_equal(&entry.key, &key, map.key_kind))
    {
        let mut old_value = std::mem::replace(&mut entry.value, value);
        drop_word(&mut old_value, map.value_kind, map.drop_custom_value);
        if map.key_kind == KEY_STRING {
            let mut consumed_key = key;
            drop_word(&mut consumed_key, KEY_STRING, None);
        } else {
            entry.key = key;
        }
        1
    } else {
        bucket.push(Entry { key, value });
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_get(pointer: *const RynMap, key: *const u8, output: *mut u8) -> i8 {
    if output.is_null() {
        fail("null Map output slot");
    }
    let map = map(pointer);
    if matches!(map.value_kind, VALUE_CUSTOM | VALUE_MOVE_ONLY_STRUCT) {
        fail("Map.get cannot clone a move-only custom value");
    }
    let key = read_words(key, map.key_stride);
    let hash = key_hash(&key, map.key_kind);
    let Some(entry) = map.buckets.get(&hash).and_then(|bucket| {
        bucket
            .iter()
            .find(|entry| keys_equal(&entry.key, &key, map.key_kind))
    }) else {
        return 0;
    };
    let value = clone_value(&entry.value, map.value_kind, map.clone_custom_value);
    // SAFETY: Output points to the compiler-allocated slot for this Map's value type.
    unsafe { ptr::copy_nonoverlapping(value.as_ptr().cast(), output, map.value_stride * 8) };
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_remove(pointer: *mut RynMap, key: *const u8) -> i8 {
    let map = map_mut(pointer);
    let key = read_words(key, map.key_stride);
    let hash = key_hash(&key, map.key_kind);
    let Some(bucket) = map.buckets.get_mut(&hash) else {
        return 0;
    };
    let Some(index) = bucket
        .iter()
        .position(|entry| keys_equal(&entry.key, &key, map.key_kind))
    else {
        return 0;
    };
    let mut entry = bucket.remove(index);
    drop_entry(
        &mut entry,
        map.key_kind,
        map.value_kind,
        map.drop_custom_value,
    );
    if bucket.is_empty() {
        map.buckets.remove(&hash);
    }
    1
}

fn owned_element_callbacks(
    kind: u64,
    custom: Option<(MapDropValue, MapCloneValue)>,
) -> (
    Option<super::vectors::DropElement>,
    Option<super::vectors::CloneElement>,
) {
    match kind {
        VALUE_STRING => (
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::DropElement>(
                    super::strings::ryn_string_drop as usize,
                )
            }),
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::CloneElement>(
                    super::strings::ryn_string_clone as usize,
                )
            }),
        ),
        VALUE_VEC => (
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::DropElement>(
                    super::vectors::ryn_vec_drop as usize,
                )
            }),
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::CloneElement>(
                    super::vectors::ryn_vec_clone as usize,
                )
            }),
        ),
        VALUE_MAP => (
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::DropElement>(ryn_map_drop as usize)
            }),
            Some(unsafe {
                std::mem::transmute::<usize, super::vectors::CloneElement>(ryn_map_clone as usize)
            }),
        ),
        VALUE_STRUCT => match custom {
            Some((drop_callback, clone_callback)) => (
                Some(unsafe {
                    std::mem::transmute::<usize, super::vectors::DropElement>(
                        drop_callback as usize,
                    )
                }),
                Some(unsafe {
                    std::mem::transmute::<usize, super::vectors::CloneElement>(
                        clone_callback as usize,
                    )
                }),
            ),
            None => (None, None),
        },
        0 => (None, None),
        _ => fail("Map iteration cannot own this element kind"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_keys(pointer: *const RynMap) -> *mut super::vectors::RynVec {
    let map = map(pointer);
    let (drop_element, clone_element) = if map.key_kind == KEY_STRING {
        owned_element_callbacks(VALUE_STRING, None)
    } else {
        (None, None)
    };
    let mut result = super::vectors::build_vec(map.key_stride, drop_element, clone_element);
    for bucket in map.buckets.values() {
        for entry in bucket {
            let mut key = clone_value(&entry.key, map.key_kind, None);
            result.push_words(&key);
            key.fill(0);
        }
    }
    super::vectors::allocate_vec(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_map_values(pointer: *const RynMap) -> *mut super::vectors::RynVec {
    let map = map(pointer);
    if matches!(map.value_kind, VALUE_CUSTOM | VALUE_MOVE_ONLY_STRUCT) {
        fail("Map.values cannot clone a move-only custom value");
    }
    let custom = map.drop_custom_value.zip(map.clone_custom_value);
    let (drop_element, clone_element) = owned_element_callbacks(map.value_kind, custom);
    let mut result = super::vectors::build_vec(map.value_stride, drop_element, clone_element);
    for bucket in map.buckets.values() {
        for entry in bucket {
            let mut value = clone_value(&entry.value, map.value_kind, map.clone_custom_value);
            result.push_words(&value);
            value.fill(0);
        }
    }
    super::vectors::allocate_vec(result)
}

#[cfg(ryn_runtime_debug)]
static LIVE_MAPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(ryn_runtime_debug)]
pub fn live_maps() -> usize {
    LIVE_MAPS.load(std::sync::atomic::Ordering::Relaxed)
}
