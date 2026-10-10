"""Port of tests/owned_strings.rs: owned String and Vec ownership, moves, cleanup and literal rules.

The Rust tests call the library directly (`ryn::check`, `ryn::compile`, the IR and codegen APIs). Here,
`ryn check` stands in for `ryn::check` and `ryn build` plus running the executable stands in for
`ryn::compile`. Diagnostic spans are checked through the rendered caret. Parts that need the library IR or
the Rust runtime shim are not ported; see RUST_ONLY_misc.md.
"""

import subprocess
import unittest

from ryn_harness import IS_WINDOWS, Run, TempDir, run_ryn


def check_source(source_text):
    """`ryn check` on a single source file, like `ryn::check(source)`."""
    with TempDir("owned-check") as directory:
        source = directory / "main.ryn"
        source.write_text(source_text, encoding="utf-8")
        return run_ryn("check", source)


def run_built(source_text):
    """Compiles `source_text` natively, then runs the program, like `ryn::compile` plus `Command::output`."""
    with TempDir("owned") as directory:
        source = directory / "main.ryn"
        source.write_text(source_text, encoding="utf-8")
        program = directory / ("program.exe" if IS_WINDOWS else "program")
        build = run_ryn("build", source, "-o", program)
        if build.code != 0:
            raise AssertionError(f"owning String program compiles: {build.stderr}")
        proc = subprocess.run([str(program)], capture_output=True)
        return Run(
            proc.returncode,
            proc.stdout.decode("utf-8", errors="replace"),
            proc.stderr.decode("utf-8", errors="replace"),
        )


class OwnedStringTests(unittest.TestCase):
    def test_dynamic_strings_run_with_unicode_growth_slicing_and_predicates(self):
        output = run_built(r"""
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
    """)
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(
            output.stdout,
            "9\n6\n🦀\n82\nRyn\nRyn 🦀\ntrue\ntrue\ntrue\nRyn 🦀\ndone\nRyn 🦀\n\n",
        )

    def test_owned_values_move_through_structures_functions_branches_and_loop_exits(self):
        output = run_built(r"""
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
    """)
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(
            output.stdout,
            "Item { text: hello, value: 7 }\nhello!\nItem { text: hello!, value: 7 }\n4\n4\n🦀\ntrue\n",
        )

    def test_guard_rejects_use_after_moves_including_partial_struct_moves_and_branch_merges(self):
        for source in [
            'fun main() { first := String("a") second := first echo first }',
            'fun take(value: String) {} fun main() { value := String("a") take(value) echo value }',
            'struct Pair { first: String, second: String } fun main() { pair := Pair { first: String("a"), second: String("b") } first := pair.first echo pair }',
            'fun main() { value := String("a") when true { moved := value } echo value }',
        ]:
            with self.subTest(source=source):
                result = check_source(source)
                self.assertNotEqual(result.code, 0, "moved owners cannot be observed")
                self.assertIn("error[R0240]", result.stderr, f"{source}: {result.stderr}")
                # The Rust test checks that the reported span is non-empty; the rendered caret stands in for it.
                self.assertIn("^", result.stderr, f"{source}: {result.stderr}")

    def test_guard_preserves_borrows_while_evaluating_later_arguments(self):
        result = check_source(
            'fun consume(value: String) -> String { value } fun main() { mut text := String("a") text.append(consume(text)) }'
        )
        self.assertNotEqual(result.code, 0, "receiver remains borrowed until append executes")
        self.assertIn("error[R0242]", result.stderr, result.stderr)

        result = check_source(
            'fun main() { text := String("a") for index in 0..2 { moved := text } }'
        )
        self.assertNotEqual(result.code, 0, "the second iteration cannot reuse a moved owner")
        self.assertIn("error[R0241]", result.stderr, result.stderr)

    def test_loop_ownership_merges_allow_reinitialization_and_returning_paths(self):
        output = run_built(r"""
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
    """)
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(output.stdout, "start!!!\nfinished\n")

    def test_mutation_requires_mut_and_clone_preserves_the_original(self):
        result = check_source('fun main() { text := String() text.append("a") }')
        self.assertIn("error[R0204]", result.stderr, result.stderr)
        self.assertNotEqual(result.code, 0)

        output = run_built(
            'fun main() { first := String("a") mut second := first.clone() second.append(second) echo first echo second second.clear() echo second.len() }'
        )
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(output.stdout, "a\naa\n0\n")

    def test_runtime_rejects_invalid_utf8_slices(self):
        # The Rust test also inspects the lowered IR for scope cleanup; that part needs the
        # library IR and is not ported (see RUST_ONLY_misc.md).
        output = run_built('fun main() { text := String("🦀") echo text.slice(1, 3) }')
        self.assertEqual(output.code, 1)
        self.assertIn("UTF-8 byte boundaries", output.stderr)

    def test_character_literals_validate_scalar_values_and_escape_forms(self):
        for source in [
            "fun main() { echo '' }",
            "fun main() { echo 'ab' }",
            r"fun main() { echo '\u{D800}' }",
        ]:
            with self.subTest(source=source):
                result = check_source(source)
                self.assertNotEqual(result.code, 0)
                self.assertIn("error[R0019]", result.stderr, result.stderr)

        output = run_built(r"fun main() { echo 'é' echo '\'' echo '\\' }")
        self.assertEqual(output.code, 0, output.stderr)
        self.assertEqual(output.stdout, "é\n'\n\\\n")

    def test_generated_cleanup_releases_every_allocation_after_returns_and_loop_exits(self):
        # The Rust test also checks the live String and Vec counters through a rustc-built
        # wrapper around the emitted object; that needs the library codegen API and is not
        # ported (see RUST_ONLY_misc.md). Here the instrumented program must pass the ownership
        # checks and run to completion.
        source = r"""
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
    """
        check = check_source(source)
        self.assertEqual(check.code, 0, f"instrumented program passes ownership checks: {check.stderr}")
        output = run_built(source)
        self.assertEqual(output.code, 0, output.stderr)


if __name__ == "__main__":
    unittest.main()
