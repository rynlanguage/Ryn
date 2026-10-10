# Pull request

**What this change does**
One paragraph: the behavior that changes, and the reason.

**Related issue**
Fixes #

**Type of change**

- [ ] Bug fix (no change to intended behavior)
- [ ] New feature (language, compiler, runtime, or standard library)
- [ ] Breaking change (an existing program's meaning, diagnostic, or CLI changes)
- [ ] Documentation only
- [ ] Build, CI, or tooling

**Tests**

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (with `RUST_MIN_STACK=16777216` for deep compiler tests)
- [ ] `cargo test --release --test suite`
- [ ] New or updated tests in `tests/suite/` or the relevant integration test, covering both the
      accepted and the rejected program
- [ ] Backends verified: [ ] native [ ] cranelift — or the change is backend-independent
- [ ] `ryn bootstrap --native` still reaches a byte-identical fixpoint (native-compiler changes)

**Compatibility**

- Programs that change behavior:
- Diagnostics added, removed, or renumbered:
- Documentation updated: [ ] `README.md` [ ] `CHANGELOG.md` [ ] `docs/`

**Notes for the reviewer**
What to look at first, what is deliberately out of scope, and any follow-up this enables.
