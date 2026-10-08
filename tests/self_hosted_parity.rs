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
    for directory in ["examples", "tests/programs", "stdlib", "selfhost/src"] {
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
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_same_parse(&format!("edge case {index}: {text:?}"), text);
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
";
    let cases = [
        "fun main() { p := id(Point { x: 1 }) q := pick(true, 1, 2) n := id(-5) c := id(1 < 2) }",
        "fun main() { a := id(2.5) b := id(\"text\") d := id(String(\"owned\")) e := count(Vec<i32>()) }",
        "fun main() { m := size(Map<String, u64>()) n := nested(7) arr := [1, 2, 3] x := first(arr) }",
        "fun main() { e := id::<u8>(3 as u8) for i in 0..3 { echo id(i) } t := when true { id(1) } else { id(2) } }",
        "fun main() { figure: Shape = Shape::Box(2) s := choose figure { Shape::Box(w) => id(w), Shape::Dot => 0 } }",
        "fun main() { for item in Vec<u8>() { echo id(item) } r := id(sizeof(Point)) w := id(1 as u16) }",
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
