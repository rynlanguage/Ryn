use ryn::sema::{IrStatement, LocalType};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ryn-owned-{}-{unique}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(source: &str) -> std::process::Output {
    let directory = Directory::new();
    let output = directory.0.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });
    ryn::compile(source, &output).expect("owning String program compiles");
    Command::new(output).output().expect("native program runs")
}

#[test]
fn dynamic_strings_run_with_unicode_growth_slicing_and_predicates() {
    let output = run(r#"
        fun main() {
            mut text := String("Ryn")
            text.append(" 🦀")
            text.push('\n')
            echo text.len()
            echo text.char_count()
            echo text.char_at(4)
            echo text.byte_at(0)
            echo text.slice(0, 3)
            echo text.trim()
            echo text.starts_with("Ryn")
            echo text.ends_with("\n")
            echo text.contains(String("🦀"))
            echo text.concat("done")
            echo text
        }
    "#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "9\n6\n🦀\n82\nRyn\nRyn 🦀\ntrue\ntrue\ntrue\nRyn 🦀\ndone\nRyn 🦀\n\n"
    );
}

#[test]
fn owned_values_move_through_structures_functions_branches_and_loop_exits() {
    let output = run(r#"
        struct Item { text: String, value: i32 }
        fun make(value: String) -> Item { Item { text: value, value: 7 } }
        fun update(value: String) -> String {
            mut result := value
            result.append("!")
            return result
        }
        fun discard(value: String) { echo value.len() }
        fun main() {
            mut item := make(String("hello"))
            echo item
            taken := item.text
            item.text = update(taken)
            echo item.text
            moved := when true { item } else { make(String("other")) }
            echo moved
            for index in 0..100 {
                local := String("loop")
                when index == 0 { continue }
                discard(local)
                when index == 2 { break }
            }
            echo '\u{1F980}'
            echo 'a' < 'z'
        }
    "#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Item { text: hello, value: 7 }\nhello!\nItem { text: hello!, value: 7 }\n4\n4\n🦀\ntrue\n"
    );
}

#[test]
fn guard_rejects_use_after_moves_including_partial_struct_moves_and_branch_merges() {
    for source in [
        "fun main() { first := String(\"a\") second := first echo first }",
        "fun take(value: String) {} fun main() { value := String(\"a\") take(value) echo value }",
        "struct Pair { first: String, second: String } fun main() { pair := Pair { first: String(\"a\"), second: String(\"b\") } first := pair.first echo pair }",
        "fun main() { value := String(\"a\") when true { moved := value } echo value }",
    ] {
        let error = ryn::check(source).expect_err("moved owners cannot be observed");
        assert_eq!(error.code, "R0240", "{source}: {error}");
        assert!(error.span.end > error.span.start);
    }
}

#[test]
fn guard_preserves_borrows_while_evaluating_later_arguments() {
    let error = ryn::check("fun consume(value: String) -> String { value } fun main() { mut text := String(\"a\") text.append(consume(text)) }")
        .expect_err("receiver remains borrowed until append executes");
    assert_eq!(error.code, "R0242");
    let error =
        ryn::check("fun main() { text := String(\"a\") for index in 0..2 { moved := text } }")
            .expect_err("the second iteration cannot reuse a moved owner");
    assert_eq!(error.code, "R0241");
}

#[test]
fn loop_ownership_merges_allow_reinitialization_and_returning_paths() {
    let output = run(r#"
        fun stop(value: String) -> bool { value.len() == 0 }
        fun leave(value: String) { while stop(value) { return } }
        fun main() {
            mut text := String("start")
            for index in 0..3 {
                moved := text
                text = moved.concat("!")
                when index == 1 { continue }
            }
            echo text
            leave(String())
            mut condition := String("done")
            while stop(condition) { condition = String("done") }
            echo "finished"
        }
    "#);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "start!!!\nfinished\n"
    );
}

#[test]
fn mutation_requires_mut_and_clone_preserves_the_original() {
    let error = ryn::check("fun main() { text := String() text.append(\"a\") }").unwrap_err();
    assert_eq!(error.code, "R0204");
    let output = run(
        "fun main() { first := String(\"a\") mut second := first.clone() second.append(second) echo first echo second second.clear() echo second.len() }",
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "a\naa\n0\n");
}

#[test]
fn guard_emits_scope_cleanup_and_runtime_rejects_invalid_utf8_slices() {
    let ir = ryn::check("fun main() { for index in 0..2 { local := String(\"loop\") } }").unwrap();
    assert!(
        ir.functions[0]
            .local_types
            .iter()
            .any(|ty| matches!(ty, LocalType::OwnedPtr))
    );
    let loop_body = ir.functions[0]
        .statements
        .iter()
        .find_map(|statement| match statement {
            IrStatement::For { body, .. } => Some(body),
            _ => None,
        })
        .unwrap();
    assert!(
        loop_body
            .iter()
            .any(|statement| matches!(statement, IrStatement::Drop { slots } if !slots.is_empty()))
    );
    let output = run("fun main() { text := String(\"🦀\") echo text.slice(1, 3) }");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8 byte boundaries"));
}

#[test]
fn character_literals_validate_scalar_values_and_escape_forms() {
    for source in [
        "fun main() { echo '' }",
        "fun main() { echo 'ab' }",
        "fun main() { echo '\\u{D800}' }",
    ] {
        assert_eq!(ryn::check(source).unwrap_err().code, "R0019");
    }
    let output = run("fun main() { echo 'é' echo '\\'' echo '\\\\' }");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "é\n'\n\\\n");
}

#[test]
fn generated_cleanup_releases_every_allocation_after_returns_and_loop_exits() {
    let source = r#"
        enum Payload { Text(String), Values(Vec<i32>), Empty }
        struct Record { first: String, second: String }
        fun make() -> String { String("returned") }
        fun record(value: String) -> Record { Record { first: value, second: String("sibling") } }
        fun extract(value: Record) -> String { value.first }
        fun inspect_payload(value: Payload) -> u64 {
            choose value {
                Payload::Text(text) => text.char_count(),
                Payload::Values(values) => values.len(),
                Payload::Empty => 0 as u64
            }
        }
        fun take_payload(value: Payload) -> String {
            choose value {
                Payload::Text(text) => text,
                Payload::Values(values) => String("values"),
                Payload::Empty => String("empty")
            }
        }
        fun early(value: String) -> i32 {
            local := String("scope")
            when true { nested := String("nested") return 7 }
            0
        }
        fun main() {
            mut replacement := String("start")
            for index in 0..3000 {
                inspect_payload(Payload::Text(String("🦀")))
                moved_text := take_payload(Payload::Text(String("returned")))
                mut values: Vec<i32> = Vec()
                values.push(index as i32)
                inspect_payload(Payload::Values(values))
                inspect_payload(Payload::Empty)
                record := Record { first: String("one"), second: String("two") }
                replacement = extract(record(make()))
                make()
                record.first.clone()
                echoless := record.second.concat("suffix").slice(0, 3)
                answer := early(echoless)
                when index % 3 == 0 { continue }
                when index == 2000 { break }
            }
            echo replacement
        }
    "#;
    let ir = ryn::check(source).expect("instrumented program passes ownership checks");
    let object = ryn::codegen::emit_object(&ir).expect("native object is emitted");
    let directory = Directory::new();
    let object_path = directory.0.join(if cfg!(windows) {
        "program.obj"
    } else {
        "program.o"
    });
    fs::write(&object_path, object).unwrap();
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime_shim.rs");
    let wrapper = directory.0.join("wrapper.rs");
    fs::write(
        &wrapper,
        format!(
            r#"
        #[path = {runtime:?}] mod runtime;
        unsafe extern "C" {{ fn ryn_main() -> i32; }}
        fn main() {{
            let status = unsafe {{ ryn_main() }};
            let live = runtime::debug_live_strings();
            let live_vectors = runtime::debug_live_vectors();
            println!("live_strings={{live}} live_vectors={{live_vectors}}");
            assert_eq!(status, 0);
            assert_eq!(live, 0, "generated code leaked an owning String");
            assert_eq!(live_vectors, 0, "generated code leaked an owning Vec");
        }}
    "#
        ),
    )
    .unwrap();
    let executable = directory.0.join(if cfg!(windows) {
        "instrumented.exe"
    } else {
        "instrumented"
    });
    let compiled = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2024")
        .arg("--cfg")
        .arg("ryn_runtime_debug")
        .arg(&wrapper)
        .arg("-C")
        .arg(format!("link-arg={}", object_path.display()))
        .arg("-o")
        .arg(&executable)
        .current_dir(&directory.0)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(executable).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("live_strings=0"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("live_vectors=0"));
}
