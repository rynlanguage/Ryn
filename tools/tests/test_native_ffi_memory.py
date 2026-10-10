"""Port of tests/native_ffi_memory.rs: raw pointers, resource drop and owned-field rules at the FFI boundary.

Each program is written to a fresh directory and run with `ryn run`.
"""

import unittest

from ryn_harness import TempDir, run_ryn


def run(text):
    with TempDir("ffi-memory") as directory:
        path = directory / "main.ryn"
        path.write_text(text, encoding="utf-8")
        return run_ryn("run", path)


class NativeFfiMemoryTests(unittest.TestCase):
    def test_repr_c_pointer_fields_aggregate_copy_and_pointer_slots(self):
        output = run(r"""
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
""")
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(
            output.stdout,
            "9\n123456\n2.5\n9\n123456\n2.5\n10\n123456\n2.5\n10\n123456\n2.5\n123456\n123456\n",
        )

    def test_borrowing_a_resource_does_not_duplicate_its_destructor(self):
        output = run(r"""
#[drop(release)]
struct Resource { pub handle: *u8, pub length: u32 }
fun release(value: Resource) { echo "drop" echo value.handle as u64 }
fun inspect(value: &Resource) -> u32 => value.length
fun main() { value := Resource { handle: 99 as *u8, length: 7 } echo inspect(&value) }
""")
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(output.stdout, "7\ndrop\n99\n")

    def test_dereferencing_a_resource_cannot_copy_ownership(self):
        output = run(r"""
#[drop(release)]
struct Resource { pub handle: *u8 }
fun release(value: Resource) {}
fun main() { value := Resource { handle: 99 as *u8 } reference := &value copy := *reference echo copy.handle as u64 }
""")
        self.assertNotEqual(output.code, 0)
        self.assertIn("R0206", output.stderr)

    def test_borrowed_resource_handle_cannot_be_overwritten(self):
        output = run(r"""
#[drop(release)]
struct Resource { pub handle: *u8 }
fun release(value: Resource) {}
fun mutate(value: &mut Resource) { value.handle = 1 as *u8 }
fun main() { mut value := Resource { handle: 99 as *u8 } mutate(&mut value) }
""")
        self.assertNotEqual(output.code, 0)
        self.assertIn("R0255", output.stderr)

    def test_whole_record_pointer_copy_rejects_nested_owned_fields(self):
        output = run(r"""
struct Text { pub data: String }
fun main() { value := Text { data: String("owned") } reference := &value copy := *reference echo copy.data }
""")
        self.assertNotEqual(output.code, 0)
        self.assertIn("R0206", output.stderr)


if __name__ == "__main__":
    unittest.main()
