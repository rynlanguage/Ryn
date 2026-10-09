# Ryn — Agent Instructions

Ryn is a statically typed, native systems programming language.

**Motto:** Reliable. Fast. Native.

## Core Principles
- No garbage collector.
- Safe by default, with explicit low-level capabilities.
- Ryn Guard handles ownership and memory safety.
- Preserve Ryn's distinctive syntax. Do not blindly copy Rust, C++, or C#.
- Linux is the primary development target.
- Full compiler self-hosting is a major goal.

## Development Rules
- Inspect existing code before making changes.
- Reuse existing implementations; avoid duplication.
- Implement complete features, not parser-only placeholders.
- Keep `ryn check`, `ryn build`, and `ryn run` consistent.
- Preserve compatibility with existing programs.
- Never remove valid tests to improve pass rates.
- Add regression tests for every fixed bug.
- Run relevant tests after each change.
- Keep documentation synchronized with actual behavior.

## Compiler & Testing
- Preserve the existing bootstrap compiler.
- Keep compiler stages compatible.
- Test real program execution, not just parsing.
- Verify stdout, stderr, exit codes, and diagnostics.
- Distinguish implemented features from planned features.
- Do not claim full self-hosting until compiler bootstrap is verified.

## Agent Workflow
1. Inspect the relevant code.
2. Implement the requested changes.
3. Run tests and fix regressions.
4. Report what changed, what passed, and what remains.

**Repository code is the source of truth.**