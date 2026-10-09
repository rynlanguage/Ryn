//! The Ryn semantic analysis reports some diagnostics itself (`selfhost/src/middle/lower.ryn`,
//! currently `R0203` and `R0204`). With `RYN_SEMA_STRICT=1` a diagnostic that the bootstrap analyzer
//! does not also report becomes an `R0904` error, so every program of the corpus is a check that
//! the Ryn analysis reproduces the bootstrap's code, message, span, and help exactly. Valid programs
//! are in the corpus too: a diagnostic the bootstrap does not report is a false positive.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ryn::{ast, ast_codec, comptime, generics, parser, patterns, sema};

fn check_strict(path: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .env("RYN_SEMA_STRICT", "1")
        .arg("check")
        .arg(path)
        .output()
        .expect("ryn process starts");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn corpus_programs() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for directory in [
        "tests/programs/fail",
        "tests/programs/pass",
        "tests/suite",
        "examples",
    ] {
        let mut pending = vec![root.join(directory)];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory)
                .expect("directory is readable")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    if path.join("src/main.ryn").is_file() {
                        continue;
                    }
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "ryn") {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files
}

#[test]
fn ryn_diagnostics_agree_with_the_bootstrap_on_the_corpus() {
    let mut disagreements = Vec::new();
    for path in corpus_programs() {
        if check_strict(&path).contains("[R0904]") {
            disagreements.push(path.display().to_string());
        }
    }
    assert!(disagreements.is_empty(), "disagreements: {disagreements:?}");
}

/// The project directories (with a `src/main.ryn`) of the corpus, and the self-hosted compiler. Each is checked as a whole
/// program.
fn project_roots() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut projects = Vec::new();
    for directory in ["tests/suite", "tests/programs/fail", "examples"] {
        let mut pending = vec![root.join(directory)];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory)
                .expect("directory is readable")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    if path.join("src/main.ryn").is_file() {
                        projects.push(path);
                    } else {
                        pending.push(path);
                    }
                }
            }
        }
    }
    // The self-hosted compiler is a project of its own: the Ryn analysis checks the sources that implement it.
    let selfhost = root.join("selfhost");
    if selfhost.join("src/main.ryn").is_file() {
        projects.push(selfhost);
    }
    projects.sort();
    projects
}

#[test]
fn ryn_diagnostics_agree_with_the_bootstrap_on_the_projects() {
    let mut disagreements = Vec::new();
    for path in project_roots() {
        if check_strict(&path).contains("[R0904]") {
            disagreements.push(path.display().to_string());
        }
    }
    assert!(disagreements.is_empty(), "disagreements: {disagreements:?}");
}

#[test]
fn unknown_variable_suggestions_match() {
    let directory =
        std::env::temp_dir().join(format!("ryn-sema-diagnostics-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory");
    let cases = [
        (
            "fun add(a: i64, b: i64) -> i64 {\n    return a + bb\n}\nfun main() {\n    echo add(1, 2)\n}\n",
            "did you mean `b`?",
        ),
        (
            "fun main() {\n    x := None\n    echo 1\n}\n",
            "Option::None",
        ),
        (
            "fun main() {\n    total := 1\n    value := total + zzzzzz\n    echo value\n}\n",
            "unknown variable `zzzzzz`",
        ),
        (
            "fun main() {\n    ab := 1\n    ac := 2\n    echo ab + ad\n}\n",
            "unknown variable `ad`",
        ),
    ];
    for (index, (source, expected)) in cases.iter().enumerate() {
        let path = directory.join(format!("case{index}.ryn"));
        fs::write(&path, source).expect("case is written");
        let stderr = check_strict(&path);
        assert!(stderr.contains("[R0203]"), "{stderr}");
        assert!(
            stderr.contains(expected),
            "expected `{expected}` in:\n{stderr}"
        );
        assert!(!stderr.contains("[R0904]"), "{stderr}");
    }
    let _ = fs::remove_dir_all(directory);
}

/// Runs one Ryn mode on a program and returns the text it writes.
fn ryn_mode(
    tool: &Path,
    directory: &Path,
    name: &str,
    mode: &str,
    program: &ast::Program,
) -> String {
    let input = directory.join(format!("{name}{mode}.ast"));
    let output = directory.join(format!("{name}{mode}.out"));
    fs::write(&input, ast_codec::encode_program(program)).expect("syntax tree is written");
    let status = Command::new(tool)
        .arg(mode)
        .arg(&input)
        .arg(&output)
        .output()
        .expect("the frontend starts");
    fs::read_to_string(&output)
        .unwrap_or_else(|_| String::from_utf8_lossy(&status.stdout).into_owned())
}

/// Runs the Ryn checks on a program in the order the frontend runs them: the compile-time checks before the
/// bootstrap derives and evaluates, then the pattern checks and the analysis on the evaluated tree. Returns the
/// diagnostic a check reported itself, or the analysis text (or the reason it declined the program).
fn ryn_analysis(directory: &Path, name: &str, source: &str) -> String {
    let tool =
        ryn::frontend::locate_or_build_self_hosted().expect("the self-hosted frontend builds");
    let mut program = generics::monomorphize(parser::parse(source).expect("case parses"))
        .expect("case specializes");
    let comptime = ryn_mode(&tool, directory, name, "--comptime", &program);
    if comptime.starts_with("diagnostic ") {
        return comptime;
    }
    if comptime::derive_defaults(&mut program)
        .and_then(|()| comptime::evaluate(&mut program))
        .is_err()
    {
        return "the Ryn checks do not report the bootstrap's derive or constant evaluation error"
            .into();
    }
    let patterns = ryn_mode(&tool, directory, name, "--patterns", &program);
    if patterns.starts_with("diagnostic ") {
        return patterns;
    }
    if patterns::desugar(&mut program).is_err() {
        return "the Ryn checks do not report the bootstrap's pattern error".into();
    }
    ryn_mode(&tool, directory, name, "--sema", &program)
}

#[test]
fn ryn_analysis_writes_the_bootstrap_diagnostic_exactly() {
    let directory = std::env::temp_dir().join(format!("ryn-sema-exact-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("scratch directory");
    let cases = [
        // An unknown name inside a function: the diagnostic keeps its own span, with no location suffix.
        "fun main() {\n    total := 1\n    value := total + zzzzzz\n    echo value\n}\n",
        // A plain assignment: the span is the whole statement.
        "fun main() {\n    x := 1\n    x = 2\n    echo x\n}\n",
        // A compound assignment: the span is the name.
        "fun main() {\n    x := 1\n    x += 2\n    echo x\n}\n",
        // An element assignment.
        "fun main() {\n    values := [1, 2, 3]\n    values[0] = 4\n    echo values[0]\n}\n",
        // A field assignment: the span is the object.
        "struct Point {\n    x: i64\n}\n\nfun main() {\n    point := Point { x: 1 }\n    point.x = 2\n    echo point.x\n}\n",
        // A field assignment through an immutable binding of a scalar: the same rule applies.
        "fun main() {\n    total := 1\n    total.value = 2\n    echo total\n}\n",
        // A call with too few arguments, inside an expression: the span is the call.
        "fun add(left: i32, right: i32) -> i32 {\n    return left + right\n}\n\nfun main() {\n    echo add(1)\n}\n",
        // A call statement with too many arguments: the hint is singular for one parameter.
        "fun greet(name: str) {\n    echo name\n}\n\nfun main() {\n    greet(\"a\", \"b\")\n}\n",
        // A function without parameters called with one: the hint is plural.
        "fun run() {\n    echo 1\n}\n\nfun main() {\n    run(2)\n}\n",
        // A string literal where an integer is required: the argument's span is the literal.
        "fun double(value: i32) -> i32 {\n    return value * 2\n}\n\nfun main() {\n    echo double(\"four\")\n}\n",
        // A boolean local in a call statement (a boolean literal is declined by the expression lowering).
        "fun show(value: i64) {\n    echo value\n}\n\nfun main() {\n    flag := true\n    show(flag)\n}\n",
        // An owned string, named by its source spelling `String`.
        "fun twice(value: i32) -> i32 {\n    return value * 2\n}\n\nfun main() {\n    echo twice(String(\"x\"))\n}\n",
        // A float literal where an integer is required.
        "fun twice(value: i32) -> i32 {\n    return value * 2\n}\n\nfun main() {\n    echo twice(1.5)\n}\n",
        // An annotated binding with a string literal: the initializer is the span.
        "fun main() {\n    total: i32 = \"four\"\n    echo total\n}\n",
        // An annotated binding with an owned string local.
        "fun main() {\n    name := String(\"x\")\n    total: i64 = name\n    echo total\n}\n",
        // An annotated binding with an owned string expression.
        "fun main() {\n    total: i32 = String(\"x\")\n    echo total\n}\n",
        // An annotated binding with a float literal.
        "fun main() {\n    total: i32 = 1.5\n    echo total\n}\n",
        // Arithmetic on two integer types: the right operand is the one that differs.
        "fun main() {\n    a: i32 = 1\n    b: i64 = 2\n    echo a + b\n}\n",
        // A float literal added to an integer: the literal refuses the integer context, so its own type decides.
        "fun main() {\n    a: i32 = 1\n    echo a + 1.5\n}\n",
        // `%` on floating-point operands of the same type.
        "fun main() {\n    echo 1.0 % 2.0\n}\n",
        // A compound assignment: the span runs from the name to the end of the value.
        "fun main() {\n    mut narrow: i32 = 1\n    wide: i64 = 2\n    narrow |= wide\n    echo narrow\n}\n",
        // A shift count that is not `u32`.
        "fun main() {\n    value: i32 = 1\n    count: i64 = 2\n    echo value << count\n}\n",
        // Equality between an integer and a boolean.
        "fun main() {\n    a: i32 = 1\n    b: bool = true\n    echo a == b\n}\n",
        // Ordering on booleans: the left operand is the one that is not numeric.
        "fun main() {\n    flag := true\n    other := false\n    echo flag < other\n}\n",
        // A name declared twice in one scope: the span is the second declaration.
        "fun main() {\n    total := 1\n    total := 2\n    echo total\n}\n",
        // A name declared again in a nested block.
        "fun main() {\n    total := 1\n    when true {\n        total := 2\n        echo total\n    }\n    echo total\n}\n",
        // Negating an unsigned value: the span is the negation.
        "fun main() {\n    value: u32 = 3\n    negative := -value\n    echo negative\n}\n",
        // `!` on an integer value.
        "fun main() {\n    count := 1\n    echo !count\n}\n",
        // `~` on a floating-point value.
        "fun main() {\n    ratio := 1.5\n    echo ~ratio\n}\n",
        // `!` on an integer literal: the literal refuses the boolean context, so its own type decides.
        "fun main() {\n    echo !1\n}\n",
        // `~` on a floating-point literal where an integer is expected.
        "fun main() {\n    value: i32 = ~1.5\n    echo value\n}\n",
        // `&&` with an integer on the left: the span is the left operand.
        "fun main() {\n    count := 1\n    flag := true\n    echo count && flag\n}\n",
        // `&&` with a string on the right: the span is the right operand.
        "fun main() {\n    flag := true\n    name := \"x\"\n    echo flag && name\n}\n",
        // `||` with an integer on the left.
        "fun main() {\n    count := 1\n    flag := true\n    echo count || flag\n}\n",
        // A range loop variable that is already declared: the span is the variable.
        "fun main() {\n    total := 1\n    for total in 0..3 {\n        echo total\n    }\n}\n",
        // A range with a floating-point start bound.
        "fun main() {\n    for index in 1.5..3.0 {\n        echo index\n    }\n}\n",
        // A range whose bounds are named integers of different types.
        "fun main() {\n    low: i32 = 0\n    high: i64 = 3\n    for index in low..high {\n        echo index\n    }\n}\n",
        // A range whose end is a floating-point literal where the start is an integer.
        "fun main() {\n    low: i32 = 0\n    for index in low..2.5 {\n        echo index\n    }\n}\n",
        // A collection loop variable that is already declared: the span is the variable.
        "fun main() {\n    item := 1\n    items := [1, 2]\n    for item in items {\n        echo item\n    }\n}\n",
        // A collection that is an integer, not iterable.
        "fun main() {\n    count := 3\n    for item in count {\n        echo item\n    }\n}\n",
        // A collection that is a floating-point value.
        "fun main() {\n    ratio := 1.5\n    for item in ratio {\n        echo item\n    }\n}\n",
        // An enum variant built with too many field values: the span is the call.
        "enum Figure {\n    Circle(f64),\n    Square(f64),\n}\n\nfun main() {\n    figure := Figure::Circle(1.0, 2.0)\n    echo 1\n}\n",
        // A unit variant built with a value.
        "enum Light {\n    Red,\n    Green,\n}\n\nfun main() {\n    light := Light::Red(1)\n    echo 1\n}\n",
        // `return` inside a `defer` body.
        "fun main() -> i32 {\n    defer {\n        return 3\n    }\n    return 0\n}\n",
        // `break` inside a `defer` body in a loop.
        "fun main() -> i32 {\n    mut i: i32 = 0\n    while i < 2 {\n        defer {\n            break\n        }\n        i += 1\n    }\n    return 0\n}\n",
        // `break` outside any loop.
        "fun main() {\n    break\n}\n",
        // `continue` outside any loop.
        "fun main() {\n    continue\n}\n",
        // `defer` in a function that returns a reference.
        "fun first(values: &i32) -> &i32 {\n    defer {\n        echo \"cleanup\"\n    }\n    return values\n}\n\nfun main() -> i32 {\n    return 0\n}\n",
        // A field read that the structure does not declare: the span is the field name.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 3, y: 4 }\n    echo point.widt\n}\n",
        // A destructuring pattern that names a field the structure does not declare.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 3, y: 4 }\n    Point { x, z } := point\n    echo x\n}\n",
        // An enum value that names a variant the enum does not declare: the span is the expression.
        "enum Mode {\n    Read,\n    Write\n}\n\nfun main() {\n    mode := Mode::Execute\n    echo 0\n}\n",
        // A `choose` arm that names a variant the enum does not declare: the span is the arm.
        "enum Mode {\n    Read,\n    Write\n}\n\nfun main() {\n    mode := Mode::Read\n    code := choose mode {\n        Mode::Read => 0,\n        Mode::Execute => 1,\n        _ => 2\n    }\n    echo code\n}\n",
        // A structure literal that leaves out a field: the span is the literal.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 1 }\n    echo point.x\n}\n",
        // A structure literal with a field the structure does not declare.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 1, widt: 2, y: 3 }\n    echo point.x\n}\n",
        // A structure literal field given a string where an integer is declared.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: \"one\", y: 2 }\n    echo point.x\n}\n",
        // A structure literal that initializes a field twice.
        "struct Point {\n    x: i32,\n    y: i32\n}\n\nfun main() {\n    point := Point { x: 1, x: 2, y: 3 }\n    echo point.x\n}\n",
        // A function declared twice: the span is the second declaration.
        "fun duplicate() {\n    echo 1\n}\n\nfun duplicate() {\n    echo 2\n}\n\nfun main() {\n    duplicate()\n}\n",
        // A function with an integer result that can end without a value: the span is the function.
        "fun answer() -> i32 {\n    echo \"computing\"\n}\n\nfun main() {\n    echo answer()\n}\n",
        // The same for a boolean result.
        "fun check() -> bool {\n    echo 1\n}\n\nfun main() {\n    echo check()\n}\n",
        // An expression body in a function without a `->` result type.
        "fun double(value: i32) => value * 2\n\nfun main() {\n    echo double(2)\n}\n",
        // An array repeat whose length differs from the annotated array: the span is the repeat.
        "fun main() {\n    values: [i32; 3] = [1; 2]\n}\n",
        // An array repeat of owned strings, which cannot be copied.
        "fun main() {\n    labels := [String(\"ryn\"); 2]\n}\n",
        // An array repeat whose element is a string in an integer array.
        "fun main() {\n    values: [i32; 2] = [\"one\"; 2]\n}\n",
        // An array repeat assigned where an integer is declared.
        "fun main() {\n    value: i32 = [1; 2]\n}\n",
        // A `choose` that leaves out a variant and has no `_` arm: the span is the `choose`.
        "enum Shape {\n    Dot,\n    Box\n}\n\nfun main() {\n    figure := Shape::Dot\n    size := choose figure {\n        Shape::Dot => 1\n    }\n    echo size\n}\n",
        // Two variants left out, listed in declaration order.
        "enum Shape {\n    Dot,\n    Box,\n    Line\n}\n\nfun main() {\n    figure := Shape::Dot\n    size := choose figure {\n        Shape::Box => 1\n    }\n    echo size\n}\n",
        // `??` on a value that is not an option: the span is the whole expression.
        "fun main() -> i32 {\n    value := 4\n    echo value ?? 9\n    return 0\n}\n",
        // A path that names an enum which the program does not declare.
        "fun main() {\n    mode := Mode::Read\n    echo 0\n}\n",
        // A path into a lowercase module that has no such item.
        "fun main() {\n    value := util::Missing\n    echo 0\n}\n",
        // `main` with a parameter: the span is the function.
        "fun main(value: i32) {\n    echo value\n}\n",
        // `return` with a value in a function without a result type: the span is the `return`.
        "fun answer() {\n    return 3\n}\n\nfun main() {\n    answer()\n}\n",
        // A structure that declares a field twice: the span is the second declaration.
        "struct Point {\n    x: i32,\n    x: i32\n}\n\nfun main() {\n    echo 1\n}\n",
        // A structure that contains itself by value: the span is the field that closes the cycle.
        "struct Node {\n    value: i32,\n    next: Node\n}\n\nfun main() {\n    echo 0\n}\n",
        // Two structures that contain each other by value; the search starts from the first.
        "struct A {\n    b: B\n}\n\nstruct B {\n    a: A\n}\n\nfun main() {\n    echo 1\n}\n",
        // A reference to a local returned from a function: the span is the returned expression.
        "fun invalid_reference() -> &i32 {\n    value := 5\n    return &value\n}\n\nfun main() {\n    echo 0\n}\n",
        // A reference local that is not a parameter.
        "fun local_reference(input: &i32) -> &i32 {\n    local := input\n    return local\n}\n\nfun main() {\n    echo 0\n}\n",
        // A reference returned from a call.
        "fun other(value: &i32) -> &i32 {\n    return value\n}\n\nfun pick(value: &i32) -> &i32 {\n    return other(value)\n}\n\nfun main() {\n    echo 0\n}\n",
        // An `extern "C"` function with a `String` parameter, which the C ABI does not pass.
        "extern \"C\" fun consume(text: String);\n\nfun main() {\n    echo 0\n}\n",
        // An `extern "C"` function named `main`.
        "extern \"C\" fun main() -> i32;\n",
        // An owned string passed to a function twice: the second use is the error.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    consume(text)\n    consume(text)\n}\n",
        // An owned string moved by a `let`, then printed.
        "fun main() {\n    name := String(\"ryn\")\n    other := name\n    echo name\n}\n",
        // An owned string moved before a `break`, then used after the loop.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    mut i: i32 = 0\n    while i < 2 {\n        consume(text)\n        break\n    }\n    consume(text)\n}\n",
        // An owned string moved before a `continue`, so the next iteration would reuse it.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    mut i: i32 = 0\n    while i < 2 {\n        i += 1\n        consume(text)\n        continue\n    }\n}\n",
        // The same from inside a `when` branch of the loop body.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    mut i: i32 = 0\n    while i < 2 {\n        i += 1\n        when i == 1 {\n            consume(text)\n            continue\n        }\n    }\n}\n",
        // A raw pointer that comes to point at a local of a deeper block: the span is the assignment.
        "fun main() {\n    mut p: *i32 = (0 as u64) as *i32\n    when true {\n        mut x: i32 = 5\n        q := &raw mut x\n        p = q\n    }\n    *p = 8\n    echo 1\n}\n",
        // A reference that comes to point at a local of a deeper block, directly.
        "fun main() {\n    a: i32 = 1\n    mut r: &i32 = &a\n    when true {\n        b: i32 = 5\n        r = &b\n    }\n    echo *r\n}\n",
        // A mutable reference to a binding that is not declared `mut`: the span is the name.
        "fun set(v: &mut i32) {\n    *v = 1\n}\n\nfun main() {\n    x: i32 = 1\n    set(&mut x)\n}\n",
        // An assignment through a shared reference: the span is the assignment.
        "fun set(v: &i32) {\n    *v = 1\n}\n\nfun main() {\n    mut x: i32 = 1\n    set(&x)\n}\n",
        // The address of a temporary: the span is the `&`.
        "fun main() {\n    mut r: &i32 = &0\n    echo *r\n}\n",
        // A bare variant name used as a value: the hint names the variant.
        "fun main() {\n    x := None\n    echo 1\n}\n",
        // A `String` method that changes the string, on a binding that is not `mut`.
        "fun main() {\n    s := String(\"a\")\n    s.append(\"b\")\n}\n",
        // A bare variant name called as a function: the hint names the variant.
        "fun main() {\n    x := Some(3)\n    echo 1\n}\n",
        // A local read while a mutable reference to it is in scope.
        "fun main() {\n    mut x: i32 = 1\n    r := &mut x\n    echo x\n    *r = 3\n}\n",
        // A local moved while a reference to it is in scope.
        "fun main() {\n    mut s := String(\"a\")\n    r := &s\n    t := s\n    echo 1\n}\n",
        // A local assigned while a shared reference to it is in scope.
        "fun main() {\n    mut x: i32 = 1\n    r := &x\n    x = 2\n    echo *r\n}\n",
        // An owned field moved out of a value whose structure has a custom destructor.
        "#[drop(done)]\nstruct Named { h: *i32, label: String }\nfun done(n: Named) {\n    echo \"released\"\n}\nfun main() {\n    mut x: i32 = 0\n    n := Named { h: &raw mut x, label: String(\"file\") }\n    taken := n.label\n    echo taken\n}\n",
        // An owned string moved in one `choose` arm, then used after the `choose`: the moved value may be gone.
        "fun consume(text: String) -> i32 {\n    echo text\n    return 1\n}\n\nfun main() {\n    name := String(\"ryn\")\n    value := Option::Some(3)\n    size := choose value {\n        Option::Some(n) => consume(name),\n        _ => 0\n    }\n    echo size\n    consume(name)\n}\n",
        // An owned string moved twice within one `choose` arm.
        "fun consume(text: String) -> i32 {\n    echo text\n    return 1\n}\n\nfun main() {\n    name := String(\"ryn\")\n    value := Option::Some(3)\n    size := choose value {\n        Option::Some(n) => consume(name) + consume(name),\n        _ => 0\n    }\n    echo size\n}\n",
        // A vector moved by a loop, then used as a method receiver after it: the span is the receiver's name.
        "fun main() {\n    mut v: Vec<i32> = Vec()\n    v.push(1)\n    for x in v {\n        echo x\n    }\n    echo v.len()\n}\n",
        // An owned string moved in one `when` branch, then used after it: the moved value may be gone.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    when true {\n        consume(text)\n    }\n    consume(text)\n}\n",
        // An owned string moved in a loop body, though declared outside the loop.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    mut i: i32 = 0\n    while i < 2 {\n        consume(text)\n        i += 1\n    }\n}\n",
        // An owned string moved twice in one `when` branch.
        "fun consume(text: String) {\n    echo text\n}\n\nfun main() {\n    text := String(\"ryn\")\n    when true {\n        consume(text)\n        consume(text)\n    }\n}\n",
        // A raw pointer to a local returned from a function: the span is the returned expression.
        "fun leak() -> *i32 {\n    mut x: i32 = 5\n    return &raw mut x\n}\nfun main() {\n    p := leak()\n    echo *p\n}\n",
        // `choose` over an integer: the span is the `choose`.
        "fun main() {\n    count := 3\n    size := choose count {\n        _ => 1\n    }\n    echo size\n}\n",
        // A `mut self` method called on an immutable local: the span is the method call.
        "struct Rect {\n    w: i64\n}\n\nextend Rect {\n    fun grow(mut self, k: i64) -> i64 {\n        self.w = self.w * k\n        return self.w\n    }\n}\n\nfun main() {\n    rect := Rect { w: 3 }\n    echo rect.grow(2)\n}\n",
        // A reference taken to an owned string after the string was moved: the span is the name.
        "fun eat(s: String) {\n    echo s\n}\n\nfun main() {\n    s := String(\"a\")\n    eat(s)\n    r := &s\n    echo 1\n}\n",
        // A raw pointer used after the value it points at was moved: the span is the pointer's use.
        "fun main() {\n    mut v: Vec<i32> = Vec()\n    v.push(3)\n    p := &raw mut v\n    w := v\n    echo w.len()\n    echo pointer_is_null(p as *u8)\n}\n",
        // An owned value returned by value out of a reference: the span is the dereference.
        "fun take(r: &String) -> String {\n    return *r\n}\n\nfun main() {\n    s := String(\"a\")\n    t := take(&s)\n    echo t\n}\n",
        // An owned value passed by value out of a reference: the span is the reference.
        "fun eat(s: String) {\n    echo s\n}\n\nfun take(r: &String) {\n    eat(*r)\n}\n\nfun main() {\n    s := String(\"a\")\n    take(&s)\n    echo s\n}\n",
        // A `choose` through a reference that binds an owned payload: the span is the reference.
        "enum Outcome {\n    Value(i32),\n    Text(String),\n    Empty,\n}\n\nfun kind(o: &Outcome) -> i32 {\n    choose *o {\n        Outcome::Value(v) => v,\n        Outcome::Text(t) => 2,\n        Outcome::Empty => 0,\n    }\n}\n\nfun main() {\n    e := Outcome::Text(String(\"x\"))\n    echo kind(&e)\n}\n",
        // A shared reference and a mutable reference to one local in one call: the span is the second argument.
        "fun two(a: &mut i32, b: &i32) {\n    *a = 1\n}\n\nfun main() {\n    mut x: i32 = 1\n    two(&mut x, &x)\n}\n",
        // Two mutable references to one local in one call: the span is the second argument.
        "fun two(a: &mut i32, b: &mut i32) {\n    *a = 1\n}\n\nfun main() {\n    mut x: i32 = 1\n    two(&mut x, &mut x)\n}\n",
        // A built-in output call whose format is a name, not a string literal: the span is the call.
        "fun main() {\n    text := \"x\"\n    print(text)\n}\n",
        // A format string with two placeholders for one value: the span is the call.
        "fun main() {\n    print(\"a {} b {}\", 1)\n}\n",
        // A format string with a brace that starts no placeholder: the span is the call.
        "fun main() {\n    print(\"open {\")\n}\n",
        // A literal index past the end of an array: the span is the indexing expression.
        "fun main() {\n    a: [i32; 3] = [1, 2, 3]\n    echo a[5]\n}\n",
        // A structure that declares no field: the span is the structure.
        "struct E {}\n\nfun main() {\n    e := E {}\n}\n",
        // A `when` statement on an integer local: the span is the condition.
        "fun main() {\n    count := 2\n    when count {\n        echo 1\n    }\n}\n",
        // A `when` statement on an integer literal: the span is the condition.
        "fun main() {\n    when 1 {\n        echo 1\n    }\n}\n",
        // A `when` expression on an integer local: the span is the condition.
        "fun main() {\n    count := 2\n    v := when count { 1 } else { 2 }\n    echo v\n}\n",
        // `?` on an `Option` in a function that returns a `Result`: the span is the `?` expression.
        "fun pos(n: i32) -> Option<i32> {\n    return Option::None\n}\n\nfun f() -> Result<i32, String> {\n    v := pos(1)?\n    return Result::Ok(v)\n}\n\nfun main() {\n    echo 1\n}\n",
        // `null` where no raw pointer type is expected: the span is the `null`.
        "fun main() {\n    x := null\n    echo 1\n}\n",
        // A reference stored into a field from a deeper block: the span is the stored reference.
        "struct Holder {\n    r: &i32\n}\n\nfun main() {\n    a: i32 = 1\n    mut h := Holder { r: &a }\n    when true {\n        b: i32 = 5\n        h.r = &b\n    }\n    echo h.r\n}\n",
        // A raw pointer stored into a field from a deeper block: the span is the stored pointer.
        "struct Holder {\n    p: *i32\n}\n\nfun main() {\n    mut h := Holder { p: (0 as u64) as *i32 }\n    when true {\n        mut x: i32 = 5\n        h.p = &raw mut x\n    }\n    echo pointer_is_null(h.p as *u8)\n}\n",
        // A structure literal with a raw pointer to a local of a deeper block, assigned to an outer structure: the span is the statement.
        "struct H {\n    p: *i32\n}\n\nfun main() {\n    mut y: i32 = 1\n    mut h := H { p: &raw mut y }\n    when true {\n        mut x: i32 = 5\n        h = H { p: &raw mut x }\n    }\n    echo *h.p\n}\n",
        // A structure local built from a deeper local's raw pointer, assigned to an outer structure: the span is the statement.
        "struct H {\n    p: *i32\n}\n\nfun main() {\n    mut y: i32 = 1\n    mut h := H { p: &raw mut y }\n    when true {\n        mut x: i32 = 5\n        k := H { p: &raw mut x }\n        h = k\n    }\n    echo *h.p\n}\n",
        // A structure returned with the address of its own local: the span is the first field's value.
        "struct H {\n    p: *i32\n}\n\nfun make() -> H {\n    mut x: i32 = 5\n    return H { p: &raw mut x }\n}\n\nfun main() {\n    h := make()\n    echo *h.p\n}\n",
        // A raw pointer that a branch points at a deeper local, copied to an outer pointer: the span is the copy.
        "fun main() {\n    mut x: i32 = 1\n    mut p: *i32 = &raw mut x\n    mut t: i32 = 0\n    when true {\n        mut y: i32 = 2\n        mut s: *i32 = &raw mut x\n        when t == 0 {\n            s = &raw mut y\n        } else {\n            s = &raw mut x\n        }\n        p = s\n    }\n    echo 1\n}\n",
        // A reference to an element of a slice that is a call result: the span is the `let`.
        "fun main() {\n    mut v: Vec<i32> = Vec()\n    v.push(1)\n    r := &v.as_slice()[0]\n    v.push(2)\n    echo *r\n}\n",
        // A custom destructor whose structure does not start with a raw pointer: the span is the structure.
        "#[drop(done)]\nstruct Bad {\n    id: i32\n}\n\nfun done(b: Bad) {\n    echo \"x\"\n}\n\nfun main() {\n    echo 1\n}\n",
        // A borrowed slice bound to a name: the span is the value.
        "fun main() {\n    mut v: Vec<i32> = Vec()\n    v.push(1)\n    s := v.as_slice()\n    echo s.len()\n}\n",
        // A literal choose with no `_` arm: the span is the first arm.
        "fun main() {\n    n: i32 = 3\n    echo choose n { 1 => 10, 2 => 20 }\n}\n",
        // A string choose with no `_` arm: the span is the first arm.
        "fun main() {\n    echo choose \"a\" {\n        \"a\" => 1,\n        \"b\" => 2\n    }\n}\n",
        // A literal arm after a variant arm: the span is the literal's arm.
        "enum E {\n    A\n}\n\nfun main() {\n    e := E::A\n    echo choose e { 1 => 1, E::A => 2 }\n}\n",
        // A literal that an earlier arm already names: the span is the repeated arm.
        "fun main() {\n    n: i32 = 3\n    echo choose n { 1 => 10, 1 => 11, _ => 0 }\n}\n",
        // An arm after a wildcard: the span is the arm.
        "fun main() {\n    n: i32 = 3\n    echo choose n { _ => 0, 1 => 10 }\n}\n",
        // An arm after a name that binds every value: the span is the arm.
        "fun main() {\n    n: i32 = 3\n    echo choose n {\n        x => x,\n        3 => 0\n    }\n}\n",
        // A variant that an earlier arm already handles: the span is the repeated arm.
        "enum E {\n    A,\n    B\n}\n\nfun main() {\n    e := E::A\n    echo choose e { E::A => 1, E::B => 2, E::A => 3 }\n}\n",
        // A static assertion that fails: the span is the statement.
        "const SIZE: i32 = 8\nfun main() {\n    static_assert(SIZE == 9, \"size must be nine\")\n}\n",
        // A constant that does not fit its type: the span is the value.
        "const BIG: u8 = 255 + 1\nfun main() {\n    echo BIG\n}\n",
        // A division by zero in a constant: the span is the division.
        "const BAD: i32 = 1 / 0\nfun main() {\n    echo BAD\n}\n",
        // A constant of the wrong type: the span is the value.
        "const FLAG: i32 = true\nfun main() {\n    echo FLAG\n}\n",
        // A constant that depends on itself: the span is the use that closes the cycle.
        "const A: i32 = B + 1\nconst B: i32 = A + 1\nfun main() {\n    echo A\n}\n",
        // A local read in a static assertion: the span is the name.
        "fun main() {\n    x: i32 = 3\n    static_assert(x == 3)\n}\n",
        // A constant that is assigned: the span is the assignment.
        "const K: i64 = 2\nfun main() {\n    K = 3\n    echo K\n}\n",
        // A `when` on a constant whose value is an integer: the span is the constant's use.
        "const N: i32 = 3\nfun main() {\n    when N {\n        echo 1\n    }\n}\n",
        // A reference returned from a call, to a local of a deeper block: the span is the assignment.
        "fun pick(a: &i32, b: &i32) -> &i32 {\n    return b\n}\n\nfun main() {\n    mut x: i32 = 1\n    mut r := &x\n    when true {\n        mut y: i32 = 7\n        r = pick(&x, &y)\n    }\n    echo *r\n}\n",
        // A structure field with no default under `#[derive(Default)]`: the span is the field.
        "struct Plain {\n    n: i32\n}\n#[derive(Default)]\nstruct Holder {\n    p: Plain\n}\n\nfun main() {\n    echo 1\n}\n",
        // A structure whose field count is not what the assertion says: the span is the statement.
        "struct Pair {\n    a: i32,\n    b: i32\n}\n\nfun main() {\n    static_assert(field_count::<Pair>() == 3, \"expected three fields\")\n}\n",
        // A metadata call without its type argument, outside an assertion: the span is the call.
        "struct Pair {\n    a: i32\n}\n\nfun main() {\n    echo field_count()\n}\n",
        // Tuple arms of different lengths: the span is the arm that differs from the first.
        "fun main() {\n    t := (1 as i32, 2 as i32)\n    echo choose t {\n        (1, 2) => 1,\n        (1, 2, 3) => 2,\n        _ => 0\n    }\n}\n",
        // An arm after a tuple of names, which matches every tuple: the span is the arm.
        "fun main() {\n    t := (1 as i32, 2 as i32)\n    echo choose t {\n        (a, b) => a,\n        (1, 2) => 0\n    }\n}\n",
        // A tuple arm that a wildcard in the first element already covers: the span is the arm.
        "fun f(t: (i32, i32)) -> i32 {\n    choose t {\n        (0, _) => 1,\n        (0, 5) => 2,\n        _ => 3\n    }\n}\n\nfun main() {\n    echo f((0, 5))\n}\n",
        // Boolean tuples without a wildcard: the span is the first arm that is not a wildcard, after the literal columns split.
        "fun f(a: bool, b: bool) -> i32 {\n    choose (a, b) {\n        (true, _) => 1,\n        (false, true) => 2\n    }\n}\n\nfun main() {\n    echo f(false, false)\n}\n",
        // A tuple pattern over an integer: the span is the `choose`.
        "fun main() {\n    n: i32 = 3\n    echo choose n {\n        (a, b) => a\n    }\n}\n",
        // A tuple pattern with a different number of elements from the value: the span is the `choose`.
        "fun main() {\n    t := (1 as i32, 2 as i32)\n    echo choose t {\n        (a, b, c) => a\n    }\n}\n",
        // A destructor called by hand: the span is the call.
        "#[drop(rel)]\nstruct R { h: *i32, id: i32 }\n\nfun rel(r: R) {\n    echo r.id\n}\n\nfun main() {\n    mut x: i32 = 0\n    a := R { h: &raw mut x, id: 1 }\n    rel(a)\n}\n",
        // A method whose return type differs from the shape's: the span is the extend block.
        "shape Show {\n    fun show(self) -> String\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Show {\n    fun show(self) -> i32 {\n        return 1\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A structure with no method of the shape's name.
        "shape Show {\n    fun show(self) -> String\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Show {\n    fun other(self) -> String {\n        return \"b\"\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A method whose first parameter is not the receiver: the shape's `self` is a reference, the method's is a value.
        "shape Show {\n    fun show(self) -> String\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Show {\n    fun show(value: B) -> String {\n        return \"b\"\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A method that takes a shared receiver where the shape takes `mut self`.
        "shape Bump {\n    fun bump(mut self)\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Bump {\n    fun bump(self) {\n        echo self.x\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // An `extern "C"` function with a structure that is not `#[repr(C)]`: the C ABI does not pass it.
        "struct P {\n    x: i32,\n    y: i32,\n}\n\nextern \"C\" fun take(p: P) -> i32;\n\nfun main() {\n    echo 1\n}\n",
        // A `#[repr(C)]` structure whose packed layout is three bytes: the C ABI passes no record of that size.
        "#[repr(C)]\nstruct Three {\n    a: u8,\n    b: u8,\n    c: u8,\n}\n\nextern \"C\" fun take(t: Three) -> i32;\n\nfun main() {\n    echo 1\n}\n",
        // A callback that is not `extern "C"`: the C ABI cannot call it, so the function is refused.
        "extern \"C\" fun each(f: fun(i32) -> i32) -> i32;\n\nfun main() {\n    echo 1\n}\n",
        // A callback that takes a structure: callbacks take scalars and pointers only.
        "#[repr(C)]\nstruct Pair {\n    a: i32,\n    b: i32,\n}\n\nextern \"C\" fun each(f: extern \"C\" fun(Pair) -> i32) -> i32;\n\nfun main() {\n    echo 1\n}\n",
        // A `bool` passed to an `i32` parameter of a method: the span is the argument, and there is no help.
        "struct Counter {\n    n: i32,\n}\n\nextend Counter {\n    fun add(self, by: i32) -> i32 {\n        return self.n + by\n    }\n}\n\nfun main() {\n    c := Counter { n: 1 }\n    echo c.add(true)\n}\n",
        // A destructor that names no function of the program: the span is the structure.
        "#[drop(missing)]\nstruct R { h: *i32, id: i32 }\n\nfun main() {\n    echo 1\n}\n",
        // A `mut self` method called on a temporary: a mutable receiver cannot borrow a temporary, at the call.
        "struct P {\n    n: i32,\n}\n\nextend P {\n    fun bump(mut self) {\n        self.n += 1\n    }\n}\n\nfun make() -> P {\n    return P { n: 1 }\n}\n\nfun main() {\n    make().bump()\n}\n",
        // A shared method called on a temporary that owns a string: only copyable temporaries are stored, at the call.
        "struct Name {\n    s: String,\n}\n\nextend Name {\n    fun size(self) -> i32 {\n        return 1\n    }\n}\n\nfun make() -> Name {\n    return Name { s: String(\"x\") }\n}\n\nfun main() {\n    echo make().size()\n}\n",
        // An `echo` of a vector: the bootstrap refuses to print a `Vec`, at the statement.
        "fun main() {\n    v: Vec<i32> = Vec()\n    echo v\n}\n",
        // A `print` of a vector: the same refusal, under the name `print`.
        "fun main() {\n    v: Vec<i32> = Vec()\n    print(\"{}\", v)\n}\n",
        // A structure with a vector field, printed by `echo`: the check looks through the fields.
        "struct Bag {\n    items: Vec<i32>,\n}\n\nfun main() {\n    b: Bag = Bag { items: Vec() }\n    echo b\n}\n",
        // A shape with two methods of which one does not conform: the span is the extend block.
        "shape Pair {\n    fun first(self) -> i32\n    fun second(self) -> String\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Pair {\n    fun first(self) -> i32 {\n        return 1\n    }\n    fun second(self) -> i32 {\n        return 2\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A method with one parameter fewer than the shape's.
        "shape Scale {\n    fun scale(self, factor: i32) -> i32\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Scale {\n    fun scale(self) -> i32 {\n        return 1\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A parameter whose builtin type differs from the shape's.
        "shape Scale {\n    fun scale(self, factor: i32) -> i32\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Scale {\n    fun scale(self, factor: i64) -> i32 {\n        return 1\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
        // A method with a parameter where the shape has none, and no return type on either side: the count decides.
        "shape Reset {\n    fun reset(self)\n}\n\nstruct B {\n    x: i32\n}\n\nextend B as Reset {\n    fun reset(self, amount: i32) {\n        echo amount\n    }\n}\n\nfun main() {\n    echo 1\n}\n",
    ];
    for (index, source) in cases.iter().enumerate() {
        let expected = sema::analyze(parser::parse(source).expect("case parses"))
            .expect_err("the bootstrap rejects the case");
        assert!(
            matches!(
                expected.code,
                "R0014"
                    | "R0016"
                    | "R0017"
                    | "R0207"
                    | "R0221"
                    | "R0250"
                    | "R0257"
                    | "R0460"
                    | "R0461"
                    | "R0462"
                    | "R0464"
                    | "R0465"
                    | "R0466"
                    | "R0467"
                    | "R0202"
                    | "R0203"
                    | "R0204"
                    | "R0205"
                    | "R0206"
                    | "R0211"
                    | "R0212"
                    | "R0013"
                    | "R0201"
                    | "R0208"
                    | "R0213"
                    | "R0222"
                    | "R0232"
                    | "R0247"
                    | "R0248"
                    | "R0214"
                    | "R0225"
                    | "R0210"
                    | "R0230"
                    | "R0234"
                    | "R0241"
                    | "R0249"
                    | "R0251"
                    | "R0252"
                    | "R0255"
                    | "R0235"
                    | "R0240"
                    | "R0256"
                    | "R0228"
                    | "R0229"
                    | "R0233"
                    | "R0267"
                    | "R0268"
                    | "R0269"
                    | "R0450"
            ),
            "{source}: {}",
            expected.code
        );
        let help = expected.help.clone().unwrap_or_default();
        let text = format!(
            "diagnostic {} {} {} {}:{} {}:{}",
            expected.code,
            expected.span.start,
            expected.span.end,
            expected.message.len(),
            expected.message,
            help.len(),
            help
        );
        let actual = ryn_analysis(&directory, &format!("immutable{index}"), source);
        assert_eq!(
            actual, text,
            "the Ryn analysis must report the bootstrap's diagnostic for:\n{source}"
        );
    }
    let _ = fs::remove_dir_all(directory);
}
