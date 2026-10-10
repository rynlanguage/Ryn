---
name: Language proposal
about: Propose a change to Ryn's syntax, semantics, memory model or standard library surface
title: "[Language] "
labels: language, proposal
assignees: ""
---

**Summary**
One paragraph: what should change, in the language's own terms.

**Motivation**
The program you cannot write today, or the safety guarantee that is missing. Concrete examples only.

**Proposed semantics**

- Syntax:
- Types involved:
- Ownership and moves (how Ryn Guard treats it):
- Diagnostics: which `Rxxxx` codes this introduces or changes, and the rejected cases
- Interaction with `comptime`, generics, `choose`, FFI and both backends:

```ryn
# before / after, with a runnable sketch
```

**Costs and risks**

- What becomes harder or slower to compile
- Migration cost for existing programs
- Which tests, examples and docs must change

**Alternatives**
Existing constructs that come close, and the reason they are not enough.

**Implementation notes**
Which parts of `src/` and `selfhost/` are affected, and whether the Ryn front end and the Rust
bootstrap path can both carry the change. Proposals that touch both paths need a plan for keeping
`tests/self_hosted_parity.rs` green.

**Open questions**
Decisions that need discussion before implementation starts.
