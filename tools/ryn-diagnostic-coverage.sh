#!/bin/bash
# Measures the Ryn semantic analysis (`ryn-frontend --sema`) against the Rust analysis on every invalid program of the
# corpus: the `// test: fail` programs of tests/suite and the fixtures of tests/programs/fail.
#
# Usage: tools/ryn-diagnostic-coverage.sh [frontend-executable] > coverage.tsv
#
# Each output line is: CLASS <TAB> path <TAB> rust-code <TAB> rust-message
# Classes: EXACT (the Ryn diagnostic has the same code and message), WRONG_TEXT (same code, other message),
# WRONG_CODE (other code), ACCEPT (Ryn accepts the program: the dangerous class), DECLINE (Ryn reports an unsupported
# construct or fails), PARSE_FAIL (the Ryn parser rejects the program).
#
# The fixture tests/fixtures/ryn_sema_coverage.tsv records these lines, and tests/ryn_sema_ratchet.rs fails when a line
# changes, so improvements must be recorded by regenerating the fixture.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 1
RYN="$ROOT/target/release/ryn"
FRONTEND="${1:-${RYN_FRONTEND_EXE:-}}"
if [ -z "$FRONTEND" ]; then
  FRONTEND=$(ls -t "$HOME"/.cache/ryn/frontend/*/ryn-frontend 2>/dev/null | head -n 1)
fi
WORK=$(mktemp -d "${TMPDIR:-/tmp}/ryn-coverage.XXXXXX") || exit 1

# The corpus: suite programs whose header says `// test: fail`, then the fixtures.
suite_fail=$(grep -rl '^// test: fail' tests/suite --include='*.ryn' | sort)
fixtures=$(ls tests/programs/fail/*.ryn | sort)

for file in $suite_fail $fixtures; do
  rust_full=$(timeout 120 "$RYN" check --frontend rust "$file" 2>&1 >/dev/null | head -n 1)
  rust_code=$(echo "$rust_full" | grep -o 'R[0-9]\{4\}' | head -n 1)
  rust_msg=$(echo "$rust_full" | sed -n 's/^error\[R[0-9]*\]: //p')
  if ! timeout 120 "$FRONTEND" "$file" "$WORK/program.ast" > /dev/null 2>&1; then
    printf 'PARSE_FAIL\t%s\t%s\t%s\n' "$file" "$rust_code" "$rust_msg"
    continue
  fi
  # The parser reports some errors in the syntax tree itself: ` Error <len>:CODE <len>:MESSAGE <start> <end> + ...`, where
  # each string is prefixed by its length.
  parser_error=$(python3 - "$WORK/program.ast" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
if not text.startswith(" Error "):
    sys.exit(0)
rest = text[len(" Error "):]
count, rest = rest.split(":", 1)
code = rest[: int(count)]
rest = rest[int(count):].lstrip(" ")
count, rest = rest.split(":", 1)
print(code + "\t" + rest[: int(count)])
PY
)
  if [ -n "$parser_error" ]; then
    ryn_code=${parser_error%%$'\t'*}
    ryn_msg=${parser_error#*$'\t'}
    case "$ryn_msg" in
      "$rust_msg"*) same_text=1 ;;
      *) same_text=0 ;;
    esac
    if [ "$ryn_code" = "$rust_code" ] && [ "$same_text" = 1 ]; then class=EXACT
    elif [ "$ryn_code" = "$rust_code" ]; then class=WRONG_TEXT
    else class=WRONG_CODE; fi
    printf '%s\t%s\t%s\t%s\n' "$class" "$file" "$rust_code" "$rust_msg"
    continue
  fi
  # The constant checks run before the analysis, as in the compiler: their diagnostic is the one reported.
  rm -f "$WORK/program.comptime"
  timeout 120 "$FRONTEND" --comptime "$WORK/program.ast" "$WORK/program.comptime" > /dev/null 2>&1
  first=$(head -n 1 "$WORK/program.comptime" 2>/dev/null)
  if [ "${first:0:11}" != "diagnostic " ]; then
    rm -f "$WORK/program.out"
    timeout 120 "$FRONTEND" --sema "$WORK/program.ast" "$WORK/program.out" > "$WORK/stdout" 2>&1
    sema_status=$?
    first=$(head -n 1 "$WORK/program.out" 2>/dev/null)
  else
    sema_status=0
  fi
  if [ "${first:0:11}" = "diagnostic " ]; then
    ryn_code=$(echo "$first" | grep -o 'R[0-9]\{4\}' | head -n 1)
    # The message follows the start and end offsets and the line number: `diagnostic CODE START END LINE:message help`.
    ryn_msg=$(echo "$first" | sed -E 's/^diagnostic R[0-9]+ [0-9]+ [0-9]+ [0-9]+://')
    case "$ryn_msg" in
      "$rust_msg"*) same_text=1 ;;
      *) same_text=0 ;;
    esac
    if [ "$ryn_code" = "$rust_code" ] && [ "$same_text" = 1 ]; then class=EXACT
    elif [ "$ryn_code" = "$rust_code" ]; then class=WRONG_TEXT
    else class=WRONG_CODE; fi
  elif [ "$sema_status" = 0 ]; then
    class=ACCEPT
  else
    class=DECLINE
  fi
  printf '%s\t%s\t%s\t%s\n' "$class" "$file" "$rust_code" "$rust_msg"
done
rm -rf "$WORK"
