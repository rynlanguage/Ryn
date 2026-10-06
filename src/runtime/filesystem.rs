use std::{
    fs,
    io::{BufRead, Read, Write},
    path::PathBuf,
};

use super::{
    enums::{self, DROP_STRING, EnumDropEntry},
    strings::{self, RynString},
};

fn path(pointer: *const u8, length: u64) -> PathBuf {
    PathBuf::from(strings::text(pointer, length))
}

fn fail(message: &str) -> ! {
    let _ = writeln!(std::io::stderr().lock(), "Ryn runtime error: {message}");
    std::process::exit(1)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_read_file(pointer: *const u8, length: u64) -> *mut RynString {
    let path = path(pointer, length);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| fail(&format!("could not read `{}`: {error}", path.display())));
    let contents =
        String::from_utf8(bytes).unwrap_or_else(|_| fail("file contents are not valid UTF-8"));
    strings::ryn_string_new(contents.as_ptr(), contents.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_try_read_file(pointer: *const u8, length: u64) -> *mut enums::RynEnum {
    let path = path(pointer, length);
    let Ok(contents) = fs::read_to_string(path) else {
        return enums::ryn_enum_new(1, std::ptr::null(), 0, std::ptr::null(), 0);
    };
    let string = strings::ryn_string_new(contents.as_ptr(), contents.len() as u64);
    let payload = (string as usize as u64).to_le_bytes();
    let drop = EnumDropEntry {
        variant: 0,
        offset: 0,
        kind: DROP_STRING,
    };
    enums::ryn_enum_new(
        0,
        payload.as_ptr(),
        payload.len() as u64,
        (&drop as *const EnumDropEntry).cast(),
        std::mem::size_of::<EnumDropEntry>() as u64,
    )
}

/// Reads UTF-8 text and returns `Result<String, i32>` as a runtime enum handle.
/// Error codes are 1 for not found, 2 for permission denied, 3 for invalid UTF-8,
/// 4 for invalid input, and 5 for other I/O errors.
#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_read_file_result(pointer: *const u8, length: u64) -> *mut enums::RynEnum {
    let path = path(pointer, length);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let code = match error.kind() {
                std::io::ErrorKind::NotFound => 1i32,
                std::io::ErrorKind::PermissionDenied => 2,
                std::io::ErrorKind::InvalidInput => 4,
                _ => 5,
            };
            let payload = i64::from(code).to_ne_bytes();
            return enums::ryn_enum_new(1, payload.as_ptr(), 8, std::ptr::null(), 0);
        }
    };
    let contents = match String::from_utf8(bytes) {
        Ok(contents) => contents,
        Err(_) => {
            let payload = 3i64.to_ne_bytes();
            return enums::ryn_enum_new(1, payload.as_ptr(), 8, std::ptr::null(), 0);
        }
    };
    let string = strings::ryn_string_new(contents.as_ptr(), contents.len() as u64);
    let payload = (string as usize as u64).to_ne_bytes();
    let drop = EnumDropEntry {
        variant: 0,
        offset: 0,
        kind: DROP_STRING,
    };
    enums::ryn_enum_new(
        0,
        payload.as_ptr(),
        8,
        (&drop as *const EnumDropEntry).cast(),
        std::mem::size_of::<EnumDropEntry>() as u64,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_write_file(
    path_ptr: *const u8,
    path_len: u64,
    data_ptr: *const u8,
    data_len: u64,
) -> bool {
    fs::write(path(path_ptr, path_len), strings::text(data_ptr, data_len)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_append_file(
    path_ptr: *const u8,
    path_len: u64,
    data_ptr: *const u8,
    data_len: u64,
) -> bool {
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(path_ptr, path_len))
    else {
        return false;
    };
    file.write_all(strings::text(data_ptr, data_len).as_bytes())
        .is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_create_dir(pointer: *const u8, length: u64) -> bool {
    fs::create_dir(path(pointer, length)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_create_dir_all(pointer: *const u8, length: u64) -> bool {
    fs::create_dir_all(path(pointer, length)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_copy_file(
    from_ptr: *const u8,
    from_len: u64,
    to_ptr: *const u8,
    to_len: u64,
) -> bool {
    fs::copy(path(from_ptr, from_len), path(to_ptr, to_len)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_rename(
    from_ptr: *const u8,
    from_len: u64,
    to_ptr: *const u8,
    to_len: u64,
) -> bool {
    fs::rename(path(from_ptr, from_len), path(to_ptr, to_len)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_delete_file(pointer: *const u8, length: u64) -> bool {
    fs::remove_file(path(pointer, length)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_delete_dir(pointer: *const u8, length: u64) -> bool {
    fs::remove_dir(path(pointer, length)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_exists(pointer: *const u8, length: u64) -> bool {
    path(pointer, length).exists()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_is_file(pointer: *const u8, length: u64) -> bool {
    path(pointer, length).is_file()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_is_directory(pointer: *const u8, length: u64) -> bool {
    path(pointer, length).is_dir()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_delete_dir_all(pointer: *const u8, length: u64) -> bool {
    fs::remove_dir_all(path(pointer, length)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_join(
    base_ptr: *const u8,
    base_len: u64,
    child_ptr: *const u8,
    child_len: u64,
) -> *mut RynString {
    let mut joined = PathBuf::from(strings::text(base_ptr, base_len));
    joined.push(strings::text(child_ptr, child_len));
    let joined = joined.to_string_lossy();
    strings::ryn_string_new(joined.as_ptr(), joined.len() as u64)
}

fn path_component_text(component: Option<&std::ffi::OsStr>) -> *mut RynString {
    let text = component.map_or_else(String::new, |value| value.to_string_lossy().into_owned());
    strings::ryn_string_new(text.as_ptr(), text.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_parent(pointer: *const u8, length: u64) -> *mut RynString {
    let value = path(pointer, length);
    path_component_text(value.parent().map(std::path::Path::as_os_str))
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_file_name(pointer: *const u8, length: u64) -> *mut RynString {
    let value = path(pointer, length);
    path_component_text(value.file_name())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_extension(pointer: *const u8, length: u64) -> *mut RynString {
    let value = path(pointer, length);
    path_component_text(value.extension())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_is_absolute(pointer: *const u8, length: u64) -> bool {
    path(pointer, length).is_absolute()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_absolute(pointer: *const u8, length: u64) -> *mut RynString {
    let value = path(pointer, length);
    let absolute = if value.is_absolute() {
        value
    } else {
        std::env::current_dir().unwrap_or_default().join(value)
    };
    let text = absolute.to_string_lossy();
    strings::ryn_string_new(text.as_ptr(), text.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_path_canonical(pointer: *const u8, length: u64) -> *mut RynString {
    let text = fs::canonicalize(path(pointer, length))
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    strings::ryn_string_new(text.as_ptr(), text.len() as u64)
}

struct RynFile {
    file: Option<std::io::BufReader<fs::File>>,
}

unsafe fn file_mut<'a>(pointer: *mut std::ffi::c_void) -> Option<&'a mut RynFile> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: Handles are allocated by ryn_file_open/create and freed once by ryn_file_drop.
    Some(unsafe { &mut *pointer.cast::<RynFile>() })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_open(pointer: *const u8, length: u64) -> *mut std::ffi::c_void {
    let requested_path = path(pointer, length);
    let file = fs::OpenOptions::new()
        .read(true)
        .open(&requested_path)
        .unwrap_or_else(|error| {
            fail(&format!(
                "could not open `{}`: {error}",
                requested_path.display()
            ))
        });
    Box::into_raw(Box::new(RynFile {
        file: Some(std::io::BufReader::new(file)),
    }))
    .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_create(pointer: *const u8, length: u64) -> *mut std::ffi::c_void {
    let requested_path = path(pointer, length);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&requested_path)
        .unwrap_or_else(|error| {
            fail(&format!(
                "could not create `{}`: {error}",
                requested_path.display()
            ))
        });
    Box::into_raw(Box::new(RynFile {
        file: Some(std::io::BufReader::new(file)),
    }))
    .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_read(pointer: *mut std::ffi::c_void) -> *mut RynString {
    let mut contents = String::new();
    if let Some(file) = unsafe { file_mut(pointer) }.and_then(|handle| handle.file.as_mut()) {
        if file.read_to_string(&mut contents).is_err() {
            contents.clear();
        }
    }
    strings::ryn_string_new(contents.as_ptr(), contents.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_read_line(pointer: *mut std::ffi::c_void) -> *mut RynString {
    let mut line = String::new();
    if let Some(file) = unsafe { file_mut(pointer) }.and_then(|handle| handle.file.as_mut()) {
        if file.read_line(&mut line).is_err() {
            line.clear();
        }
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    strings::ryn_string_new(line.as_ptr(), line.len() as u64)
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_write(
    pointer: *mut std::ffi::c_void,
    data: *const u8,
    length: u64,
) -> bool {
    unsafe { file_mut(pointer) }
        .and_then(|handle| handle.file.as_mut())
        .is_some_and(|file| {
            file.get_mut()
                .write_all(strings::text(data, length).as_bytes())
                .is_ok()
        })
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_flush(pointer: *mut std::ffi::c_void) -> bool {
    unsafe { file_mut(pointer) }
        .and_then(|handle| handle.file.as_mut())
        .is_some_and(|file| file.get_mut().flush().is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_close(pointer: *mut std::ffi::c_void) -> bool {
    unsafe { file_mut(pointer) }
        .and_then(|handle| handle.file.take())
        .is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_file_drop(pointer: *mut std::ffi::c_void) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: Ryn's custom destructor calls this once for each File owner.
    drop(unsafe { Box::from_raw(pointer.cast::<RynFile>()) });
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ryn_fs_read_dir(pointer: *const u8, length: u64) -> *mut super::vectors::RynVec {
    let directory = path(pointer, length);
    let entries = fs::read_dir(&directory)
        .unwrap_or_else(|error| {
            fail(&format!(
                "could not read directory `{}`: {error}",
                directory.display()
            ))
        })
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    fail(&format!(
                        "could not read an entry in `{}`: {error}",
                        directory.display()
                    ))
                })
                .path()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let mut entries = entries;
    entries.sort();
    let vector = super::vectors::ryn_vec_new(
        1,
        Some(strings::ryn_vec_elem_string_drop),
        Some(strings::ryn_vec_elem_string_clone),
    );
    for entry in entries {
        let value = strings::ryn_string_new(entry.as_ptr(), entry.len() as u64);
        // Vec<String> stores one owning RynString pointer per eight-byte slot.
        super::vectors::ryn_vec_push(vector, (&value as *const *mut RynString).cast());
    }
    vector
}
