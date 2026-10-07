use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn run(text: &str) -> std::process::Output {
    let directory = std::env::temp_dir().join(format!(
        "ryn-ffi-memory-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.ryn");
    fs::write(&path, text).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .args(["run", path.to_str().unwrap()])
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(directory);
    result
}
#[test]
fn repr_c_pointer_fields_aggregate_copy_and_pointer_slots() {
    let output = run(r#"
#[repr(C)]
struct Record { pub small: u8, pub wide: u64, pub value: f32 }
fun set(pointer: *Record) { pointer.small = 9 pointer.wide = 123456 pointer.value = 2.5 }
fun main() {
    mut a := Record { small: 1, wide: 2, value: 3.0 }
    p := &raw mut a
    set(p)
    echo p.small echo p.wide echo p.value
    echo a.small echo a.wide echo a.value
    a.small = 10
    echo p.small echo p.wide echo p.value
    mut b := Record { small: 0, wide: 0, value: 0.0 }
    q := &raw mut b
    *q = *p
    echo q.small echo q.wide echo q.value
    address := p as u64
    roundtrip := address as *Record
    echo roundtrip.wide
    mut slot := 0 as *Record
    out := &raw mut slot
    *out = p
    restored := *out
    echo restored.wide
}
"#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "9\n123456\n2.5\n9\n123456\n2.5\n10\n123456\n2.5\n10\n123456\n2.5\n123456\n123456\n"
    );
}
#[test]
fn borrowing_a_resource_does_not_duplicate_its_destructor() {
    let output = run(r#"
#[drop(release)]
struct Resource { pub handle: *u8, pub length: u32 }
fun release(value: Resource) { echo "drop" echo value.handle as u64 }
fun inspect(value: &Resource) -> u32 => value.length
fun main() { value := Resource { handle: 99 as *u8, length: 7 } echo inspect(&value) }
"#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\ndrop\n99\n");
}
#[test]
fn dereferencing_a_resource_cannot_copy_ownership() {
    let output = run(r#"
#[drop(release)]
struct Resource { pub handle: *u8 }
fun release(value: Resource) {}
fun main() { value := Resource { handle: 99 as *u8 } reference := &value copy := *reference echo copy.handle as u64 }
"#);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("R0206"));
}
#[test]
fn borrowed_resource_handle_cannot_be_overwritten() {
    let output = run(r#"
#[drop(release)]
struct Resource { pub handle: *u8 }
fun release(value: Resource) {}
fun mutate(value: &mut Resource) { value.handle = 1 as *u8 }
fun main() { mut value := Resource { handle: 99 as *u8 } mutate(&mut value) }
"#);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("R0255"));
}

#[test]
fn whole_record_pointer_copy_rejects_nested_owned_fields() {
    let output = run(r#"
struct Text { pub data: String }
fun main() { value := Text { data: String("owned") } reference := &value copy := *reference echo copy.data }
"#);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("R0206"));
}
