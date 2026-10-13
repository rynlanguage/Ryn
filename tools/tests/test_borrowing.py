"""Execution and Guard regressions for aggregate loans and mutable, zero-copy slices."""
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from ryn_harness import REPO_ROOT, IS_WINDOWS, ryn_binary


class Borrowing(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ryn-borrow-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.compiler = os.environ.get("RYN_TEST_RYNC") or str(Path(ryn_binary()).with_name("rync.exe" if IS_WINDOWS else "rync"))

    def compile(self, source, expected=None, error=None):
        path = self.root / "main.ryn"
        path.write_text(source, encoding="utf-8")
        out = self.root / ("app.exe" if IS_WINDOWS else "app")
        common = [self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix()]
        for args in (["check", path.as_posix()], ["-o", out.as_posix(), path.as_posix()]):
            p = subprocess.run([*common, *args], capture_output=True, timeout=120)
            if error:
                self.assertNotEqual(p.returncode, 0)
                self.assertIn(error, p.stderr.decode())
            else:
                self.assertEqual(p.returncode, 0, p.stderr.decode())
        if error:
            return p.stderr.decode()
        p = subprocess.run([str(out)], capture_output=True, timeout=15)
        self.assertEqual(p.returncode, 0, p.stderr.decode() + repr(p.stdout))
        self.assertEqual(p.stderr, b"")
        self.assertEqual(p.stdout.decode(), expected)

    def test_nonlexical_scalar_and_aggregate_loans(self):
        self.compile('''struct Point { x: i32, y: i32 }
fun main() {
    mut x: i32 = 1
    r := &mut x
    *r = 2
    echo *r
    x = 3
    echo x
    mut p := Point { x: 4, y: 5 }
    shared := &p
    echo shared.x
    p.y = 6
    loan := &mut p
    loan.x = 7
    echo loan.y
    echo p.x
}
''', "2\n3\n4\n6\n7\n")

    def test_mutable_array_and_vec_slices(self):
        self.compile('''fun bump(items: &mut [i32]) {
    mut i: u64 = 0
    while i < items.len() { items[i] = items[i] + 10 i += 1 }
}
fun sum(items: &[i32]) -> i32 {
    mut result: i32 = 0
    for item in items { result += item }
    return result
}
fun main() {
    mut values: [i32; 3] = [1, 2, 3]
    bump(&mut values)
    echo values[0]
    echo sum(values)
    view: &mut [i32] = &mut values
    view[1] = 30
    echo view[1]
    values[2] = 40
    echo values[2]
    mut v: Vec<i32> = Vec()
    v.push(4) v.push(5) v.push(6)
    bump(v.as_mut_slice())
    bump(v.slice_mut(1, 3))
    echo v[0] echo v[1] echo v[2]
    s := v.as_slice()
    echo s.len()
    v.push(7)
    echo v.len()
}
''', "11\n36\n30\n40\n14\n25\n26\n3\n4\n")

    def test_resource_references_do_not_clone_or_drop_the_owner(self):
        self.compile('''namespace c;
extern "C" fun malloc(size: u64) -> *u8;
extern "C" fun free(pointer: *u8);
namespace;
#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) { echo value.id c::free(value.handle) }
struct Registry { gpu: Gpu, text: String }
fun read(value: &Registry) -> i32 { return value.gpu.id }
fun change(value: &mut Registry) { value.gpu.id = 20 value.text.append("!") }
fun main() {
    mut reg := Registry { gpu: Gpu { handle: c::malloc(8), id: 10 }, text: String("hello") }
    echo read(&reg)
    change(&mut reg)
    r := &reg
    echo (*r).gpu.id
    echo r.text
    reg.text.append("?")
    echo reg.text
}
''', "10\n20\nhello!\nhello!?\n20\n")

    def test_live_loan_diagnostic_names_its_origin(self):
        message = self.compile('''fun main() {
    mut x: i32 = 1
    r := &x
    x = 2
    echo *r
}
''', error="R0249")
        self.assertIn("borrow held by `r`", message)
        self.assertIn("begins at bytes", message)
        self.assertIn("note: borrow starts here", message)
        self.assertIn("main.ryn:3:", message)

    def test_shared_slice_blocks_vec_mutation_until_last_use(self):
        self.compile('''fun main() {
    mut v: Vec<i32> = Vec()
    v.push(1)
    s := v.as_slice()
    v.push(2)
    echo s[0]
}
''', error="R0249")

    def test_exclusive_slice_blocks_source_reads(self):
        self.compile('''fun main() {
    mut v: Vec<i32> = Vec()
    v.push(1)
    s := v.as_mut_slice()
    echo v[0]
    s[0] = 2
}
''', error="R0249")

    def test_loop_backedges_and_defer_keep_loans_live(self):
        for source in ('''fun main() {
    mut x: i32 = 1
    r := &x
    for i in 0..2 { echo *r x = 2 }
}
''', '''fun main() {
    mut x: i32 = 1
    r := &x
    defer { echo *r }
    x = 2
}
''', '''fun main() {
    mut x: i32 = 1
    r := &x
    when true { x = 2 } else { echo *r }
    echo *r
}
'''):
            with self.subTest(source=source):
                self.compile(source, error="R0249")

    def test_slice_mutability_and_reference_escape(self):
        for source, error in (('''fun main() {
    a: [i32; 1] = [1]
    s: &mut [i32] = &mut a
    s[0] = 2
}
''', "R0206"), ('''fun write(s: &[i32]) { s[0] = 2 }
fun main() { a: [i32; 1] = [1] write(a) }
''', "R0206"), ('''fun bad() -> &i32 { x: i32 = 1 return &x }
fun main() {}
''', "R0248")):
            with self.subTest(source=source):
                self.compile(source, error=error)

    def test_cannot_move_resource_through_reference(self):
        self.compile('''#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) {}
fun take(value: Gpu) {}
fun main() {
    gpu := Gpu { handle: null, id: 1 }
    r := &gpu
    take(*r)
}
''', error="R0240")

    def test_reborrows_freeze_parent_only_until_last_use(self):
        self.compile('''fun main() {
    mut x: i32 = 1
    r := &mut x
    child := r
    *child = 2
    *r = 3
    echo x
    mut a: [i32; 2] = [4, 5]
    s: &mut [i32] = &mut a
    p := &mut s[1]
    *p = 6
    s[0] = 7
    echo a[0] echo a[1]
}
''', "3\n7\n6\n")
        self.compile('''fun main() {
    mut x: i32 = 1
    r := &mut x
    child := r
    *r = 2
    echo *child
}
''', error="R0249")

    def test_reference_assignment_replaces_the_loan(self):
        self.compile('''fun main() {
    mut x: i32 = 1
    mut y: i32 = 2
    mut r := &x
    echo *r
    r = &y
    x = 3
    echo *r
    y = 4
    echo x echo y
}
''', "1\n2\n3\n4\n")
        self.compile('''fun main() {
    mut x: i32 = 1
    mut y: i32 = 2
    mut r := &x
    r = &y
    y = 3
    echo *r
}
''', error="R0249")

    def test_call_arguments_cannot_overlap_mutable_slices_or_move_the_owner(self):
        for source in ('''fun both(a: &mut [i32], b: &[i32]) {}
fun main() { mut a: [i32; 1] = [1] both(&mut a, a) }
''', '''fun both(a: &mut [i32], b: &mut [i32]) {}
fun main() { mut v: Vec<i32> = Vec() v.push(1) s := v.as_mut_slice() both(s, s) }
''', '''fun both(a: &[i32], b: Vec<i32>) {}
fun main() { mut v: Vec<i32> = Vec() v.push(1) both(v.as_slice(), v) }
'''):
            with self.subTest(source=source):
                self.compile(source, error="R0249")

    def test_custom_mutating_method_respects_shared_references(self):
        for source in ('''struct P { x: i32 }
extend P { fun bump(mut self) { self.x += 1 } }
fun main() { mut p := P { x: 1 } r := &p p.bump() echo r.x }
''', '''struct P { x: i32 }
extend P { fun bump(mut self) { self.x += 1 } }
fun main() { p := P { x: 1 } r := &p r.bump() }
''', '''struct P { text: String }
fun bad(p: &P) { p.text.append("!") }
fun main() { p := P { text: String("hello") } bad(&p) }
'''):
            with self.subTest(source=source):
                self.compile(source, error="R02")

    def test_zero_copy_aggregate_slices_and_owned_replacement(self):
        self.compile('''struct Entry { text: String, id: i32 }
fun replace(s: &mut [Entry]) { s[0] = Entry { text: String("new"), id: 20 } }
fun read(s: &[Entry]) { echo s[0].text echo s[1].id }
fun main() {
    mut a: [Entry; 2] = [Entry { text: String("old"), id: 1 }, Entry { text: String("other"), id: 2 }]
    replace(&mut a)
    read(a)
    echo a[0].text
}
''', "new\n2\nnew\n")

    def test_reference_return_keeps_source_alive(self):
        self.compile('''fun first(items: &[i32]) -> &i32 { return &items[0] }
fun main() {
    mut v: Vec<i32> = Vec() v.push(1)
    p := first(v.as_slice())
    echo *p
    v.push(2)
    echo v.len()
}
''', "1\n2\n")
        self.compile('''fun first(items: &[i32]) -> &i32 { return &items[0] }
fun main() {
    mut v: Vec<i32> = Vec() v.push(1)
    p := first(v.as_slice())
    v.push(2)
    echo *p
}
''', error="R0249")

    def test_local_slices_and_implicit_references_cannot_escape(self):
        for source, code in (('''fun bad(x: i32) -> &i32 { &x }
fun main() {}
''', "R0248"), ('''fun bad() -> &[i32] { a: [i32; 1] = [1] a }
fun main() {}
''', "R0248"), ('''fun main() {
    mut a: [i32; 1] = [1]
    mut s: &[i32] = &a
    when true { b: [i32; 1] = [2] s = &b }
    echo s[0]
}
''', "R0251")):
            with self.subTest(source=source):
                self.compile(source, error=code)

    def test_generic_mutable_slice_and_small_element_widths(self):
        self.compile('''fun set<T>(s: &mut [T], value: T) { s[0] = value }
fun main() {
    mut a: [u8; 2] = [1, 2]
    set::<u8>(&mut a, 255)
    echo a[0] echo a[1]
    mut f: [f32; 2] = [1.5, 2.5]
    set::<f32>(&mut f, 3.5)
    echo f[0] echo f[1]
    mut empty: [i32; 0] = []
    s: &mut [i32] = &mut empty
    echo s.len()
}
''', "255\n2\n3.5\n2.5\n0\n")

    def test_zero_iteration_loops_and_branches_merge_loan_sources(self):
        for control in ("while false { r = &y }", "for i in 0..0 { r = &y }",
                        "when true { r = &y } else {}"):
            with self.subTest(control=control):
                self.compile(f'''fun main() {{
    mut x: i32 = 1
    mut y: i32 = 2
    mut r := &x
    {control}
    x = 3
    echo *r
}}
''', error="R0249")

    def test_resource_slice_borrows_and_replacement_drop_once(self):
        self.compile('''#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) { echo value.id }
fun edit(s: &mut [Gpu]) {
    r := &mut s[0]
    r.id = 20
    s[1] = Gpu { handle: 1 as *u8, id: 30 }
}
fun main() {
    mut v: Vec<Gpu> = Vec()
    v.push(Gpu { handle: 1 as *u8, id: 1 })
    v.push(Gpu { handle: 1 as *u8, id: 2 })
    edit(v.as_mut_slice())
}
''', "2\n30\n20\n")

    def test_move_only_slice_element_reads_and_stored_descriptors_are_rejected(self):
        self.compile('''#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) {}
fun read(s: &[Gpu]) { value := s[0] }
fun main() {}
''', error="R0240")
        self.compile('''struct Holder { slice: &[i32] }
fun main() {}
''', error="R0250")

    def test_slice_bounds_fail_at_runtime(self):
        path = self.root / "bounds.ryn"
        path.write_text('''fun bad(s: &mut [i32]) { s[1] = 9 }
fun main() { mut a: [i32; 1] = [1] bad(&mut a) }
''', encoding="utf-8")
        out = self.root / ("bounds.exe" if IS_WINDOWS else "bounds")
        p = subprocess.run([self.compiler, "--stdlib", (REPO_ROOT / "stdlib/std/src").as_posix(),
                            "-o", out.as_posix(), path.as_posix()], capture_output=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr.decode())
        p = subprocess.run([str(out)], capture_output=True, timeout=15)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(p.stdout, b"")
        self.assertIn(b"index is out of bounds", p.stderr)

    def test_mutable_slices_can_be_lent_readonly(self):
        self.compile('''fun read(s: &[i32]) -> i32 { return s[0] }
fun first(s: &[i32]) -> &i32 { &s[0] }
fun edit(s: &mut [i32]) { echo read(s) echo read(s.as_slice()) s[0] = 2 }
fun main() {
    mut a: [i32; 1] = [1]
    edit(&mut a)
    p := first(a)
    echo *p
    a[0] = 3
    echo a[0]
}
''', "1\n1\n2\n3\n")
        self.compile('''fun bad(v: &Vec<i32>) { (*v).push(2) }
fun main() { mut v: Vec<i32> = Vec() v.push(1) bad(&v) }
''', error="R0206")

    def test_resource_arrays_construct_replace_and_drop_in_order(self):
        self.compile('''#[drop(release)]
struct Gpu { handle: *u8, id: i32 }
fun release(value: Gpu) { echo value.id }
fun main() {
    mut a: [Gpu; 2] = [Gpu { handle: 1 as *u8, id: 1 }, Gpu { handle: 1 as *u8, id: 2 }]
    a[0] = Gpu { handle: 1 as *u8, id: 3 }
}
''', "1\n2\n3\n")

    @unittest.skipUnless(os.environ.get("RYN_TEST_BIN"), "requires the packaged CLI")
    def test_cli_check_build_run_agree_on_borrowing(self):
        source = self.root / "cli.ryn"
        source.write_text('''fun edit(s: &mut [i32]) { s[0] = 2 }
fun main() { mut a: [i32; 1] = [1] edit(&mut a) echo a[0] }
''', encoding="utf-8")
        out = self.root / ("cli.exe" if IS_WINDOWS else "cli")
        for arguments, stdout in ((["check", str(source)], ""),
                                  (["build", str(source), "-o", str(out)], None),
                                  (["run", str(source)], "2\n")):
            result = subprocess.run([ryn_binary(), *arguments], capture_output=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            if stdout is not None:
                self.assertEqual(result.stdout.decode(), stdout)
        source.write_text('''fun main() {
    mut x: i32 = 1
    r := &x
    x = 2
    echo *r
}
''', encoding="utf-8")
        for arguments in (["check", str(source)], ["build", str(source), "-o", str(out)], ["run", str(source)]):
            result = subprocess.run([ryn_binary(), *arguments], capture_output=True, timeout=120)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"R0249", result.stderr)
            self.assertIn(b"note: borrow starts here", result.stderr)
