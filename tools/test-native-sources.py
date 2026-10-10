#!/usr/bin/env python3
"""Compile language-suite programs directly from source with the native compiler `rync`.

Usage: tools/test-native-sources.py /absolute/path/to/rync SOURCE...

A SOURCE is a `.ryn` file or a project directory (with `ryn.yaml` and `src/main.ryn`); a project's `src/main.ryn` is
compiled through its directory. The header comments state the expectation, as in `tests/suite.rs`:

    // test: run    the program compiles, and its stdout (`// out:` lines), stderr (`// err:` texts) and exit
                    status (`// exit:`) match
    // test: fail   the compiler rejects the program with status 1 and the diagnostic `// code:` and `// msg:` texts

No Rust frontend, IR dump, system linker or C library is used to compile; programs that declare C functions are
linked against the system's shared C library at run time. Every process runs under a 1 GiB address-space limit.
"""
from pathlib import Path
import resource
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
COMPILER = str(Path(sys.argv[1]).resolve())
STDLIB = str(ROOT / "stdlib/std/src")


def limit():
    resource.setrlimit(resource.RLIMIT_AS, (1073741824, 1073741824))


def header(source):
    case = {"kind": None, "out": [], "err": [], "exit": 0, "code": None, "msg": []}
    for line in source.read_text().splitlines():
        if not line.startswith("//"):
            break
        rest = line[2:].removeprefix(" ")
        if ":" not in rest:
            continue
        key, value = rest.split(":", 1)
        value = value.removeprefix(" ")
        if key == "test":
            case["kind"] = value.strip()
        elif key in ("out", "err", "msg"):
            case[key].append(value)
        elif key == "exit":
            case["exit"] = int(value.strip())
        elif key == "code":
            case["code"] = value.strip()
    return case


def run(path):
    given = ROOT / path
    # A project's entry file is compiled through its project directory, so the manifest's dependencies are found.
    target = given
    if given.is_dir():
        source = given / "src/main.ryn"
    else:
        source = given
        if given.name == "main.ryn" and (given.parent.parent / "ryn.yaml").is_file():
            target = given.parent.parent
    case = header(source)
    if case["kind"] not in ("run", "fail"):
        return "skip", path, "no `// test:` header"
    with tempfile.TemporaryDirectory(prefix="ryn-native-source-") as directory:
        executable = Path(directory) / "program"
        build = subprocess.run([COMPILER, "build", "--stdlib", STDLIB, "-o", str(executable), str(target)],
                               cwd=ROOT, capture_output=True, preexec_fn=limit, timeout=300)
        stderr = build.stderr.decode(errors="replace")
        if case["kind"] == "fail":
            if build.returncode == 0:
                return "accepted", path, "the compiler accepted an invalid program"
            if build.returncode != 1:
                return "wrongstatus", path, f"status {build.returncode}: {stderr.strip()[:300]}"
            if case["code"] and f"[{case['code']}]" not in stderr:
                return "wrongcode", path, f"expected {case['code']}: {stderr.strip()[:300]}"
            for text in case["msg"]:
                if text not in stderr:
                    return "wrongmsg", path, f"lacks `{text}`: {stderr.strip()[:300]}"
            return "pass", path, ""
        if build.returncode:
            return "compilefail", path, stderr.strip()[:600]
        result = subprocess.run([str(executable)], cwd=ROOT, capture_output=True, preexec_fn=limit, timeout=60)
        expected = "".join(line + "\n" for line in case["out"])
        if result.returncode != case["exit"] or result.stdout.decode(errors="replace") != expected:
            return "mismatch", path, repr((result.returncode, result.stdout[:160], result.stderr[:160]))
        program_stderr = result.stderr.decode(errors="replace")
        for text in case["err"]:
            if text not in program_stderr:
                return "mismatch", path, f"stderr lacks `{text}`: {program_stderr[:300]}"
    return "pass", path, ""


# Popen preexec_fn must not run in a threaded parent. Use worker processes.
from concurrent.futures import ProcessPoolExecutor
if __name__ == '__main__':
    with ProcessPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(run, sys.argv[2:]))
    for status, path, detail in results:
        print(status, path, detail, flush=True)
    print(f"{sum(status == 'pass' for status, _, _ in results)}/{len(results)} passed", flush=True)
    sys.exit(any(status not in ("pass", "skip") for status, _, _ in results))
