#!/usr/bin/env python3
"""Compile language-suite programs directly from source with the native compiler `rync`.

Usage: tools/test-native-sources.py /absolute/path/to/rync SOURCE...

A SOURCE is a `.ryn` file or a project directory (with `ryn.yaml` and `src/main.ryn`); a project's `src/main.ryn` is
compiled through its directory. The header comments state the expectation, as in `tests/suite.rs`:

    // test: run    the program compiles, and its stdout (`// out:` lines), stderr (`// err:` texts) and exit
                    status (`// exit:`) match
    // test: fail   the compiler rejects the program with status 1 and the diagnostic `// code:` and `// msg:` texts
    // target: linux   the program uses Linux-only C functions; it is skipped when RYN_TEST_TARGET=windows

No Rust frontend, IR dump, system linker or C library is used to compile; programs that declare C functions are
linked against the system's shared C library at run time. Every process runs under a 1 GiB address-space limit.
"""
from pathlib import Path
import resource
import subprocess
import sys
import tempfile

import os

ROOT = Path(__file__).resolve().parents[1]
# RYN_TEST_TARGET=windows compiles every program for Windows and runs it under Wine in the container `ryn-wine`
# (`podman run`), so the result is the behaviour under Wine, not on Windows itself.
WINDOWS = os.environ.get("RYN_TEST_TARGET") == "windows"
# RYN_TEST_WINE_COMPILER=/path/to/rync.exe runs that Windows compiler under Wine too (it compiles inside the container).
WINE_COMPILER = os.environ.get("RYN_TEST_WINE_COMPILER")
if WINE_COMPILER:
    WINDOWS = True
COMPILER = str(Path(sys.argv[1]).resolve())
STDLIB = str(ROOT / "stdlib/std/src")


def limit():
    resource.setrlimit(resource.RLIMIT_AS, (1073741824, 1073741824))


def header(source):
    case = {"target": None, "kind": None, "out": [], "err": [], "exit": 0, "code": None, "msg": []}
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
        elif key == "target":
            case["target"] = value.strip()
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
    if WINDOWS and case["target"] == "linux":
        return "skip", path, "Linux-only program (`// target: linux`)"
    if case["kind"] not in ("run", "fail"):
        return "skip", path, "no `// test:` header"
    with tempfile.TemporaryDirectory(prefix="ryn-native-source-") as directory:
        executable = Path(directory) / ("program.exe" if WINDOWS else "program")
        build_command = [COMPILER, "build"] + (["--target", "windows"] if WINDOWS else []) + ["--stdlib", STDLIB, "-o", str(executable), str(target)]
        if WINE_COMPILER:
            exe = Path(WINE_COMPILER).resolve()
            relative = os.path.relpath(target, ROOT)
            build_command = ["podman", "run", "--rm", "--network=none", "-v", f"{exe.parent}:/k:ro,Z", "-v", f"{ROOT}:/repo:ro,Z", "-v", f"{directory}:/w:Z", "-w", "/repo", "ryn-wine", "wine", f"/k/{exe.name}", "build", "--stdlib", "/repo/stdlib/std/src", "-o", "/w/program.exe", relative]
        build = subprocess.run(build_command,
                               cwd=ROOT, capture_output=True, preexec_fn=None if WINE_COMPILER else limit, timeout=600)
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
        if build.returncode == 3 and WINDOWS and "in a Windows program" in stderr:
            return "declined", path, stderr.strip().splitlines()[0][:200] if stderr.strip() else "needs the C library"
        if build.returncode:
            return "compilefail", path, stderr.strip()[:600]
        if WINDOWS:
            run_command = ["wine", str(executable)] if os.environ.get("RYN_TEST_WINE") == "local" else ["podman", "run", "--rm", "--network=none", "-v", f"{directory}:/w:Z", "-v", f"{ROOT}:/repo:ro,Z", "-w", "/repo", "ryn-wine", "wine", "/w/program.exe"]
            result = subprocess.run(run_command, cwd=ROOT, capture_output=True, timeout=120)
            if not result.stdout and result.returncode == 0 and case["out"]:
                # Wine sometimes starts without output when many containers start at once: one more try.
                result = subprocess.run(run_command, cwd=ROOT, capture_output=True, timeout=120)
        else:
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
    with ProcessPoolExecutor(max_workers=int(os.environ.get('RYN_TEST_JOBS', '8'))) as pool:
        results = list(pool.map(run, sys.argv[2:]))
    for status, path, detail in results:
        print(status, path, detail, flush=True)
    print(f"{sum(status == 'pass' for status, _, _ in results)}/{len(results)} passed", flush=True)
    declined = sum(status == "declined" for status, _, _ in results)
    if declined:
        print(f"{declined} declined (not a failure of the program, but not run)", flush=True)
    sys.exit(any(status not in ("pass", "skip", "declined") for status, _, _ in results))
