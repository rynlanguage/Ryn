---
name: Bug report
about: Report a defect in the Ryn compiler, runtime or standard library
title: "[Bug] "
labels: bug
assignees: ""
---

**Describe the bug**
A clear description of what is wrong.

**Reproduction**

```sh
# exact command
ryn --version
```

```ryn
# the smallest program that shows the problem
fun main() {
    echo "hello"
}
```

**Expected behavior**
What you expected to happen, and why.

**Actual behavior**
What happened instead: full stdout/stderr and the exit code.

**Environment**

- OS and architecture:
- `ryn --version`:
- Target (Linux x86-64 or Windows x86-64):
- How you built the compiler (release archive, `tools/bootstrap-native.sh`, or a commit):

**Additional context**
Diagnostic codes, related issues, or anything that narrows the defect down. If the program passes on
one backend and fails on the other, say so.
