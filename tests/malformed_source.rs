const FRAGMENTS: &[&str] = &[
    "fn",
    "struct",
    "main",
    "for",
    "in",
    "let",
    "mut",
    "echo",
    "if",
    "else",
    "while",
    "break",
    "continue",
    "return",
    "true",
    "false",
    "value",
    "i32",
    "u64",
    "f32",
    "str",
    "Pair",
    "call(1 + , 2)",
    "fun pair(first: i32, second: ) { }",
    "struct Pair { a: , b: i32 }",
    "Pair { a: 1 + , b: }",
    "when true { := 1 }",
    "(",
    ")",
    "{",
    "}",
    ":",
    ",",
    "->",
    "..",
    ".",
    "=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "&&",
    "||",
    "!",
    "1",
    "1.0",
    "2e3",
    "\"text\"",
    "\"Łódź 🦀\"",
    "\"e\u{301}\"",
    "\"\\u{1F980}\"",
    "\"\\u{}\"",
    "\"\\u{D800}\"",
    "\"bad\\q\"",
    "\"unfinished\\q",
    "\"unfinished",
    "// komentarz 猫\n",
    "猫",
    "💥",
    "\u{00a0}",
    "\0",
    "@",
    "// comment\n",
];

fn next_random(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 32) as usize
}

#[test]
fn full_check_pipeline_does_not_panic_on_deterministic_malformed_sources() {
    let separators = [" ", "\n", "\t", ""];
    let mut state = 0x5259_4e5f_4348_4543;

    for case in 0..4_096 {
        let count = 1 + next_random(&mut state) % 32;
        let mut source = String::new();
        for _ in 0..count {
            let fragment = FRAGMENTS[next_random(&mut state) % FRAGMENTS.len()];
            source.push_str(fragment);
            source.push_str(separators[next_random(&mut state) % separators.len()]);
        }

        assert!(
            std::panic::catch_unwind(|| ryn::check(&source)).is_ok(),
            "compiler panicked for deterministic malformed case {case}: {source:?}"
        );
        assert!(
            std::panic::catch_unwind(|| ryn::check_recovering(&source)).is_ok(),
            "recovering compiler panicked for deterministic malformed case {case}: {source:?}"
        );
        assert!(
            std::panic::catch_unwind(|| ryn::parser::parse_recovering(&source)).is_ok(),
            "recovering parser panicked for deterministic malformed case {case}: {source:?}"
        );
    }
}
