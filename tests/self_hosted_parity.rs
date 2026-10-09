//! The self-hosted frontend (written in Ryn, `selfhost/`) must produce exactly
//! the syntax tree and diagnostics of the bootstrap parser (written in Rust)
//! for every input, and must reproduce itself when it builds itself.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use ryn::{
    ast_codec::{encode_recovering_result, encode_result},
    frontend::{Frontend, build_self_hosted},
    generics::monomorphize,
    parser::{parse, parse_recovering},
};

fn work_directory() -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("self-hosted-parity");
    fs::create_dir_all(&directory).expect("create test directory");
    directory
}

fn executable(directory: &Path, stem: &str) -> PathBuf {
    directory.join(if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    })
}

fn run_with_large_stack(name: &str, task: fn()) {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(8 * 1024 * 1024)
        .spawn(task)
        .expect("spawn parser parity worker")
        .join()
        .expect("parser parity worker completed");
}

/// Stage 1: the self-hosted frontend built by the bootstrap parser.
fn stage1() -> &'static Frontend {
    static STAGE1: OnceLock<Frontend> = OnceLock::new();
    STAGE1.get_or_init(|| {
        let directory = work_directory();
        let output = executable(&directory, "stage1");
        build_self_hosted(&directory.join("stage1-src"), &output, &Frontend::Bootstrap)
            .expect("the bootstrap parser builds the self-hosted frontend");
        Frontend::SelfHosted(output)
    })
}

fn assert_same_parse(label: &str, text: &str) {
    let frontend = stage1();
    let expected = encode_result(&parse(text));
    let actual = encode_result(&frontend.parse(text));
    assert!(
        expected == actual,
        "first-error parse differs for {label}\nbootstrap: {}\nself-hosted: {}",
        excerpt(&expected),
        excerpt(&actual)
    );
    let expected = encode_recovering_result(&parse_recovering(text));
    let actual = encode_recovering_result(&frontend.parse_recovering(text));
    assert!(
        expected == actual,
        "recovering parse differs for {label}\nbootstrap: {}\nself-hosted: {}",
        excerpt(&expected),
        excerpt(&actual)
    );
}

fn excerpt(text: &str) -> &str {
    let mut end = text.len().min(600);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn ryn_sources() -> Vec<PathBuf> {
    fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "build") {
                    continue;
                }
                collect(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "ryn") {
                files.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for directory in [
        "examples",
        "tests/programs",
        "tests/suite",
        "stdlib",
        "selfhost/src",
    ] {
        collect(&root.join(directory), &mut files);
    }
    files.sort();
    files
}

#[test]
fn self_hosted_frontend_matches_the_bootstrap_parser_on_every_source() {
    run_with_large_stack("self-hosted-source-parity", self_hosted_source_parity);
}

fn self_hosted_source_parity() {
    let sources = ryn_sources();
    assert!(sources.len() > 100, "expected the repository's Ryn sources");
    for path in sources {
        let text = fs::read_to_string(&path).expect("read source");
        assert_same_parse(&path.display().to_string(), &text);
    }
}

#[test]
fn self_hosted_frontend_matches_on_edge_cases_and_diagnostics() {
    run_with_large_stack(
        "self-hosted-edge-parity",
        self_hosted_edge_cases_and_diagnostics,
    );
}

fn self_hosted_edge_cases_and_diagnostics() {
    let cases = [
        "",
        "fun main() { echo 42 }",
        "fun main() { x := 0b1010 + 0xFF + 1_000 + 2.5e3 + 5ms + 2min echo x }",
        "fun main() { echo \"a\\u{1F980}b\\n\\t\\0\\\\\" echo '\\u{E9}' echo 'é' }",
        "fun main() { echo \"{name}, {point.x}, {pair.0} {{literal}}\" }",
        "fun main() { echo(\"{grouped}\") }",
        "fun main() { v := Vec<Vec<i32>>() m := Map<String, Vec<Option<i32>>>() s := Set<u8>() }",
        "struct Box<T> { value: T, items: Vec<T> }\nenum Maybe<T> { Some(T), None }\ntype Pair<T> = (T, T)\nfun main() { b := Box::<i32> { value: 1, items: Vec<i32>() } m := Maybe::<String>::None p: Pair<u8> = (1, 2) }",
        "namespace math::geometry;\nstruct Point { x: f64 }\ntype Meter = f64\nnamespace;\nfun main() {}",
        "#[derive(Clone, Eq, Hash)]\nstruct Key { id: u64 }\n#[repr(C)]\nstruct Raw { a: i32 }\n#[drop(close)]\nstruct Handle { raw: u64 }\nfun close(handle: Handle) {}",
        "shape Named { fun name(self) -> String fun loud(self) -> String => self.name() }\nextend Key as Named { pub fun name(self) -> String => String(\"k\") pub const ZERO: u64 = 0 }",
        "extern \"C\" fun abs(value: i32) -> i32;\nfun main() { f: extern \"C\" fun(i32) -> i32 = abs }",
        "fun pick(x: i32) -> i32 { when x > 0 { 1 } else when x < 0 { -1 } else { 0 } }",
        "fun f() -> Result<u64, String> { value := g()? h()? return Result::Ok(value) }",
        "fun main() { choose value { Shape::Dot => 0, Shape::Box(w, h) => w * h, _ => 1 } }",
        "fun main() { a := [1, 2, 3] b := [0; 8] c := a[1] d := &raw mut x e := *p *p = 4 }",
        "fun main() { for i in 0..10 { when i % 2 == 0 { continue } } for item in items { break } }",
        "fun main() { x := 1 + 2 * 3 << 4 & 5 ^ 6 | 7 == 8 && !y || ~z as u8 >= 9 }",
        "fun main() { mut p := Point { x: 1, y: 2 } p.x += 1 p.inner.0 = 3 arr[2] = 4 counter <<= 1 }",
        "fun main() { x := sizeof(Point) + alignof(u64) io::println(\"hi\") list.push(1) }",
        "fun main() { (a, mut b, _) := pair Point { x, y: mut py, .. } := p Outer { inner: Inner { z }, name } := o (q, Point { x: r }) := t }",
        "fun main() { echo 1\n(count, label) := pair\nPoint { x } := make(1) (1, 2) }",
        "fun main() { Point { x, y: mut z } := p }",
        "fun main() { (a) := x }",
        "fun main() { Point { x, } := p Point { .. } := p Point { x: } := p (a, b := c }",
        "fun main() { x := a?.b?.c y := f()?.g() z := (h()?).i w := arr[0]?.name v := t?.0 }",
        "fun main() { x := a?. }",
        "fun main() { x := a?.b(1)?.c::<T>() }",
        // Literal and nested choose patterns, and top-level constants.
        "fun main() { choose n { 1 => 10, -2 => 20, 'x' => 30, true => 1, _ => 0 } }",
        "fun main() { choose o { Outer::Nested(Inner::Data(5)) => 1, Outer::Nested(Inner::Data(n)) => n, Outer::Direct(_) => 0 } }",
        "fun main() { choose o { Option::Some(-1) => 1, Option::Some(Inner::A(Inner::B(x, 'c'))) => x } }",
        "fun main() { choose s { \"ryn\" => 1, \"\" => 2, _ => 0 } choose t { (0, y) => y, (x, 0) => x, _ => 0 } }",
        "fun main() { choose p { Point { x: 0, y } => y, Point { x, y: (1, 2) } => x, Point { } => 0 } }",
        "fun main() { choose o { Option::Some((1, Point { x, y: \"a\" })) => x } }",
        "fun main() { choose t { (0, => 1 } }",
        "fun main() { choose p { Point { x: } => 1 } }",
        "struct Stack<T> { items: Vec<T> }\nextend<T> Stack<T> { pub fun size(self) -> u64 { return self.items.len() } pub const TAG: u8 = 1 }\nfun main() { s := Stack::<i32> { items: Vec<i32>() } t := Stack::<String> { items: Vec<String>() } }",
        "fun main() { s := Box::<u8> { value: 1 } }\nstruct Box<T> { value: T }\nextend<T> Box<T> { pub fun get(self) -> T { return self.value } }",
        "struct P<A, B> { a: A, b: B }\nextend<A, B> P<A, B> { fun a(self) -> A { return self.a } }\nextend<A, B> P<B, A> {}\nextend<T> Missing<T> {}\nextend<T, T> P<T, T> {}\nextend<T> P<T> { fun x() {} } extend<T> Box<T> as Shape {}",
        "struct B<T> { v: T }\nextend<T> B<T> { fun make(self) -> B<B<T>> { return B::<B<T>> { v: self } } }\nfun main() { b := B::<u8> { v: 1 } }",
        "struct B<T> { v: T }\nextend<T> B<T> { fun f(self) { x := 1 +* } }\nfun main() { b := B::<u8> { v: 1 } }",
        "struct B<T> { v: T }\nextend<T> B<T> { fun f(self)",
        "fun main() { choose n { 0 => 1, other => other } choose s { A::B => 1, rest => 2 } choose x { y => y, => 1 } }",
        "struct S<T> { v: T }\nfun f<T>(s: S<T>) -> T { return s.v }\nfun g<T>(v: T) -> S<T> { return S::<T> { v: v } }\nfun main() { a := S::<i32> { v: 1 } echo f(a) b: S<u8> = g(1 as u8) }",
        "struct W<T> { inner: S<T> }\nstruct S<T> { v: T }\nfun main() { w := W::<i32> { inner: S::<i32> { v: 1 } } }",
        "const N: u64 = 3\nconst M: u64 = 1 + 1\nstruct G { c: [i32; N] }\nfun main() { a: [i32; N] = [0; N] b: [u8; M] = [0; M] c: [u8; Z] = [0; Z] }",
        "fun main() { choose o { Option::Some(1 => 1 } }",
        "fun main() { choose o { Option::Some(Inner::) => 1 } }",
        "const LIMIT: i32 = 5\npub const NAME: str = \"ryn\"\nfun main() { echo LIMIT }",
        "namespace m;\nconst INNER: u8 = 1 + 2\nnamespace;\nconst OUTER: u8 = 1",
        "const BAD i32 = 1",
        "const BAD: i32 1",
        "#[derive(Clone, Default)]\nstruct S { a: i32 }\n#[derive(Magic)]\nstruct T { a: i32 }",
        "fun main() { r := 1..10 s := a..=b + 1 for i in 0..n {} for i in x..=y {} t := a..b..c }",
        "fun main() { x := 1.. }",
        // Lexical errors.
        "fun main() { echo \"unterminated }",
        "fun main() { x := 1__2 + 0b102 + 0x + 1e + 1e999 + 18446744073709551616 }",
        "fun main() { @ echo(💥) # }",
        "fun main() { echo \"\\q\" echo \"\\u{D800}\" echo 'ab' echo '' }",
        "/* unterminated /* nested */",
        // Syntax errors and recovery.
        "fun main() { echo 1 +* }",
        "fun main( { }",
        "struct Point { x: , y: f64, z }\nfun main() {}",
        "fun main() { when x { } else { } when { } while 1 + { } for in 0..3 { } }",
        "fun main() { f(1, , 3) g(1 2) Point { x: 1 y: 2 } Point { x: , y: 2 } }",
        "fun broken() { x := } fun also() { y = } struct S { a: } enum E { A(, B }",
        "fun main() { choose v { A => , B::C => 1, => 2 } }",
        "pub use other\npub extend Thing {}\n#[repr(C)] enum Nope {}\n#[bad] struct S {}",
        "fun f<T, T>() {} fun g<i32>() {} struct S<T, T> {} type A<T, T> = T",
        "fun main() { x := (((((((((((((((((((((((((1))))))))))))))))))))))))) }",
        "fun main() { echo \"{unterminated\" }",
        "fun main() { echo \"{bad path!}\" }",
        "fun main() { echo \"stray } here\" }",
        "fun main() { when a { when b { when c { } } } else when { } }",
        "fun f() -> i32 { when a { 1 } else { 2 }",
        // Whitespace, comments, and line endings in unusual places.
        "fun\tmain()\t{\n\t\techo\t1\n}\n",
        "fun main() { echo 1 }",
        "fun main() {\r\n    echo 1\r\n}\r\n",
        "fun add(a: i32 /* first */, /* second */ b: i32) -> i32 { return a + b }\nfun main() { echo add(1, 2) }\n",
        "fun main() // comment\n{\n    echo 1\n}\n",
        "fun main() {}\n// trailing comment without newline",
        "fun main() {}\n/* never closed",
        "/* a /* b */ c */ fun main() {}",
        "fun main() {}\n/* a */ /* b */\n// c",
        "fun main() { /* only comment */ }\n",
        "// nothing here\n",
        "   \n\t\n   ",
        "fun main() {};\n;\n",
        "fun main() { s := \"line1\nline2\" }",
        // Trailing and doubled commas.
        "fun main() { echo(1, 2,) }\n",
        "fun f(a: i32, b: i32,) -> i32 { return a }\nfun main() {}\n",
        "struct P { x: i32, }\nfun main() { p := P { x: 1, } echo p.x }\n",
        "enum E { A, B, }\nfun main() {}\n",
        "enum Shape { Dot, Box(i32, i32,) }\nfun main() {}\n",
        "fun main() { echo(1,,2) }\n",
        "fun main() { choose x { 1 => 2, } }\n",
        // Nested generics and ambiguous comparison operators.
        "fun main() { v := Vec<Vec<Vec<i32>>>() echo 1 }\n",
        "fun main() { m := Map<String, Vec<Option<Vec<u8>>>>() }\n",
        "fun main() { a := 1 < 2 b := 3 > 4 c := a < b }\n",
        "fun id<T>(v: T) -> T { return v }\nfun main() { echo id<i32>(1) }\n",
        "struct B<T { v: T }\n",
        // Truncated declarations and unbalanced delimiters.
        "fun main(",
        "fun main() -> ",
        "struct Point",
        "enum E { A, B",
        "fun main() { x := 1 y: ",
        "type X =",
        "extend Foo {",
        "#[derive(Clone",
        "#[repr(C)]\n",
        "pub pub fun main() {}\n",
        "extern \"C\" fun abs(v: i32) -> i32\nfun main() {}\n",
        "fun main() {\n    echo 1\n",
        "fun main() {\n    echo 1\n}\n}\n",
        "fun main() {\n    echo \"hello\n}\n",
        "fun main() { echo \"a\\",
        "fun main() { echo \"{x\" }",
        "fun main() { echo \"{}\" }\n",
        "fun main() { echo \"{ {x} }\" }\n",
        "fun main() { echo \"{p.x} {p.z}\" }\n",
        "return 1\n",
        // Bad tokens, unicode identifiers, and byte-order or zero-width characters.
        "fun main() { x := 'a }\n",
        "fun main() { echo `x` }\n",
        "fun main() { $x := 1 }\n",
        "fun café() {}\nfun main() { café() }\n",
        "fun main() { π := 3 echo π }\n",
        "fun main() { 💥 := 1 }\n",
        "fun main() { echo \"héllo wörld 🦀\" }\n",
        "fun main()\u{200b}{ echo 1 }\n",
        "\u{feff}fun main() { echo 1 }\n",
        "fun main() {\0 echo 1 }\n",
        "fun main() { echo \"\\u{110000}\" }\n",
        // Malformed numeric literals and member access.
        "fun main() { x := 12abc }\n",
        "fun main() { x := 1.2.3 }\n",
        "fun main() { x := 99999999999999999999999999 }\n",
        "fun main() { t := (1, 2) echo t.5 }\n",
        "fun main() { b := !!true echo b }\n",
        // Keywords as identifiers and empty struct literals.
        "fun main() { shape := 1 }\n",
        "fun main() { when := 1 }\n",
        "struct E {}\nfun main() { e := E {} }\n",
        "fun main() { choose x { } }\n",
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_same_parse(&format!("edge case {index}: {text:?}"), text);
    }
}

#[test]
fn self_hosted_frontend_matches_on_generated_nesting_and_size() {
    run_with_large_stack("self-hosted-generated-parity", self_hosted_generated_parity);
}

fn self_hosted_generated_parity() {
    let parentheses = format!(
        "fun main() {{ x := {}1{} }}\n",
        "(".repeat(300),
        ")".repeat(300)
    );
    let blocks = format!(
        "fun main() {{\n{}{}}}\n",
        "{\n".repeat(300),
        "}\n".repeat(300)
    );
    let whens = format!(
        "fun main() {{ {}1{} }}\n",
        "when true { ".repeat(100),
        " }".repeat(100)
    );
    let elements = (0..3000)
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let array = format!("fun main() {{ a := [{elements}] }}\n");
    let identifier = format!("fun {}() {{}}\nfun main() {{}}\n", "a".repeat(5000));
    let cases = [
        ("300 nested parentheses", parentheses),
        ("300 nested blocks", blocks),
        ("100 nested when expressions", whens),
        ("3000 array elements", array),
        ("5000-character identifier", identifier),
    ];
    for (label, text) in &cases {
        assert_same_parse(label, text);
    }
}

fn assert_same_specialization(label: &str, text: &str) {
    let Ok(program) = parse(text) else {
        return;
    };
    let expected = encode_result(&monomorphize(program));
    let program = parse(text).expect("the source parsed above");
    let actual = encode_result(&stage1().monomorphize(program));
    assert!(
        expected == actual,
        "generic specialization differs for {label}
bootstrap: {}
self-hosted: {}",
        excerpt(&expected),
        excerpt(&actual)
    );
}

#[test]
fn self_hosted_specialization_matches_the_bootstrap_pass() {
    run_with_large_stack(
        "self-hosted-specialization-parity",
        self_hosted_specialization_parity,
    );
}

fn self_hosted_specialization_parity() {
    for path in ryn_sources() {
        let text = fs::read_to_string(&path).expect("read source");
        assert_same_specialization(&path.display().to_string(), &text);
    }
    let generic = "struct Point { x: i32 }
enum Shape { Dot, Box(i32) }
        fun id<T>(value: T) -> T { return value }
        fun pick<T>(flag: bool, a: T, b: T) -> T { return when flag { a } else { b } }
        fun first<T>(items: &[T]) -> T { return items[0] }
        fun count<T>(items: Vec<T>) -> u64 { return items.len() }
        fun size<K, V>(map: Map<K, V>) -> u64 { return map.len() }
        fun nested<T>(value: T) -> T { return id(value) }
        fun twice<T>(value: T) -> T { return id(id(value)) }
        fun pair<A, B>(a: A, b: B) -> A { return a }
        fun fixed<T>(count: u64, value: T, label: str) -> T { return value }
";
    let cases = [
        "fun main() { p := id(Point { x: 1 }) q := pick(true, 1, 2) n := id(-5) c := id(1 < 2) }",
        "fun main() { a := id(2.5) b := id(\"text\") d := id(String(\"owned\")) e := count(Vec<i32>()) }",
        "fun main() { m := size(Map<String, u64>()) n := nested(7) arr := [1, 2, 3] x := first(arr) }",
        "fun main() { e := id::<u8>(3 as u8) for i in 0..3 { echo id(i) } t := when true { id(1) } else { id(2) } }",
        "fun main() { figure: Shape = Shape::Box(2) s := choose figure { Shape::Box(w) => id(w), Shape::Dot => 0 } }",
        "fun main() { for item in Vec<u8>() { echo id(item) } r := id(sizeof(Point)) w := id(1 as u16) }",
        "fun main() { x := twice(8 as i32) y := pair::<i32, bool>(3, true) z := pair(1 as u8, 2) w := fixed(1 as u64, 2.5, \"label\") }",
        "fun main() { n := field_count::<Point>() t := type_name::<Shape>() h := has_field::<Point>(\"x\") v := variant_count::<Shape>() }",
        "fun main() { x := id() }",
        "fun main() { x := pick(true, 1, \"two\") }",
        "fun main() { x := id::<i32, u8>(1) }",
        "fun make<U>() -> Vec<U> { return Vec<U>() }
fun main() { x := make() }",
        "fun grow<U>(value: U, depth: u64) -> u64 { when depth == 0 { return 0 } return grow([value], depth - 1) }
fun main() { x := grow(1, 3) }",
        "fun main() { x := plain::<i32>(1) }",
    ];
    for (index, case) in cases.iter().enumerate() {
        assert_same_specialization(
            &format!("generic case {index}"),
            &format!("{generic}{case}"),
        );
    }
}

#[test]
fn self_hosted_frontend_matches_on_truncated_sources() {
    run_with_large_stack("self-hosted-truncated-parity", self_hosted_truncated_parity);
}

fn self_hosted_truncated_parity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in ["examples/generic_enums.ryn", "examples/objects.ryn"] {
        let text = fs::read_to_string(root.join(relative)).expect("read source");
        let mut cut = 0;
        while cut < text.len() {
            if text.is_char_boundary(cut) {
                assert_same_parse(&format!("{relative} truncated at {cut}"), &text[..cut]);
            }
            cut += 37;
        }
    }
}

#[test]
fn self_hosted_frontend_reproduces_itself() {
    run_with_large_stack("self-hosted-fixpoint", self_hosted_fixpoint);
}

fn self_hosted_fixpoint() {
    let directory = work_directory();
    let Frontend::SelfHosted(stage1) = stage1() else {
        unreachable!("stage 1 is self-hosted");
    };
    let stage2 = executable(&directory, "stage2");
    build_self_hosted(
        &directory.join("stage2-src"),
        &stage2,
        &Frontend::SelfHosted(stage1.clone()),
    )
    .expect("stage 1 builds the self-hosted frontend");
    assert_eq!(
        fs::read(stage1).expect("read stage 1"),
        fs::read(&stage2).expect("read stage 2"),
        "the frontend built by itself must be identical to the one built by the bootstrap parser"
    );
}
