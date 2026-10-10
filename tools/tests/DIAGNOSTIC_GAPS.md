# Diagnostic gaps

`test_diagnostic_corpus.py` runs `rync check` on every row of `tests/fixtures/ryn_sema_coverage.tsv` and compares the first
diagnostic's code and message with the ones recorded when the original Rust analyzer produced them. All 195 rows match.

What the corpus does not pin: the exact spans, help texts and suggestions of the Rust analyzer (`ryn_sema_diagnostics.rs`
compared those). They are in the git history (branch `rust-reference-0.1.6` in a checkout that has it) and in the
messages above; a change in a span or a help text is not caught by this test.
