#!/usr/bin/env python3
"""Compile // test: run programs directly from source with native rync.

Usage: tools/test-native-sources.py /absolute/path/to/rync SOURCE...
No Rust frontend, IR dump, system linker or libc is used in these runs.
"""
from pathlib import Path
import resource
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
COMPILER = str(Path(sys.argv[1]).resolve())

def limit():
    resource.setrlimit(resource.RLIMIT_AS, (1073741824, 1073741824))

def run(path):
    source = ROOT / path
    text = source.read_text()
    if "// test: run" not in text:
        return "skip", path, "not a run fixture with expected output"
    expected = "".join(line[len("// out:"):].removeprefix(" ") + "\n"
                       for line in text.splitlines() if line.startswith("// out:"))
    expected_exit = next((int(line[len("// exit:"):].strip()) for line in text.splitlines()
                          if line.startswith("// exit:")), 0)
    with tempfile.TemporaryDirectory(prefix="ryn-native-source-") as directory:
        executable = Path(directory) / "program"
        with executable.open("wb") as output:
            build = subprocess.run([COMPILER, "--stdlib", str(ROOT / "stdlib/std/src"), str(source)],
                                   cwd=ROOT, stdout=output, stderr=subprocess.PIPE,
                                   preexec_fn=limit, timeout=180)
        if build.returncode:
            return "compilefail", path, build.stderr.decode(errors="replace").strip()
        executable.chmod(0o755)
        result = subprocess.run([str(executable)], cwd=ROOT, capture_output=True, preexec_fn=limit, timeout=30)
        if result.returncode != expected_exit or result.stdout.decode() != expected:
            return "mismatch", path, repr((result.returncode, result.stdout[:160], result.stderr[:160]))
    return "pass", path, ""

# Popen preexec_fn must not run in a threaded parent. Use worker processes.
from concurrent.futures import ProcessPoolExecutor
if __name__ == '__main__':
    with ProcessPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(run, sys.argv[2:]))
    for status, path, detail in results:
        print(status, path, detail, flush=True)
    print(f"{sum(status == 'pass' for status, _, _ in results)}/{len(results)} passed", flush=True)
    sys.exit(any(status != "pass" for status, _, _ in results))
