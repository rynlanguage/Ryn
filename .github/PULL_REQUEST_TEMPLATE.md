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

- [ ] `tools/bootstrap-native.sh` reaches a fixpoint (stage 2 equals stage 3)
- [ ] `python3 tools/test-native-sources.py <rync> $(python3 tools/list-suite.py)`
- [ ] `python3 tools/run-tests.py <ryn>`
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
