#!/usr/bin/env python3
"""Runs the SPIR-V kernels of tests/gpu on a Vulkan device and checks their buffers against the expected output.

Usage: tools/tests/test_spirv_gpu.py /absolute/path/to/rync [--keep DIR]

For each tests/gpu/NAME.ryn the compiler writes NAME.spv with `--target spirv`; `spirv-val` (from the Vulkan SDK, found
through VULKAN_SDK or PATH) checks the module; tools/gpu/vulkan_host.c, built with `clang` or `gcc` (RYN_CC overrides),
runs the entry point described by NAME.spec and prints the buffers, which must equal NAME.expect. A part that is not
available (no Vulkan driver, no C compiler, no spirv-val) is skipped with its reason and is not a failure.
"""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TESTS = ROOT / "tests" / "gpu"
HOST_SOURCE = ROOT / "tools" / "gpu" / "vulkan_host.c"


def find_tool(name):
    sdk = os.environ.get("VULKAN_SDK")
    if sdk:
        for candidate in (Path(sdk) / "Bin" / (name + ".exe"), Path(sdk) / "bin" / name):
            if candidate.is_file():
                return str(candidate)
    return shutil.which(name)


def build_host(directory):
    compiler = os.environ.get("RYN_CC") or shutil.which("clang") or shutil.which("gcc")
    if not compiler:
        return None, "no C compiler (clang or gcc)"
    executable = Path(directory) / ("vulkan_host.exe" if os.name == "nt" else "vulkan_host")
    command = [compiler, "-O1", "-o", str(executable), str(HOST_SOURCE)]
    if os.name != "nt":
        command.append("-ldl")
    include = os.environ.get("VULKAN_SDK")
    if include:
        command.insert(1, "-I" + str(Path(include) / "Include"))
    built = subprocess.run(command, capture_output=True, text=True)
    if built.returncode != 0:
        return None, "the Vulkan host does not build: " + built.stderr.strip()[:300]
    return str(executable), ""


def run_case(rync, host, validator, source, directory):
    name = source.stem
    module = Path(directory) / (name + ".spv")
    compiled = subprocess.run([rync, "--stdlib", str(ROOT / "stdlib" / "std" / "src"), "--target", "spirv", "-o", str(module), str(source)],
                              capture_output=True, text=True)
    if compiled.returncode != 0:
        return "compilefail", name, compiled.stderr.strip()[:400]
    if validator:
        checked = subprocess.run([validator, str(module)], capture_output=True, text=True)
        if checked.returncode != 0:
            return "invalid", name, checked.stdout.strip() + checked.stderr.strip()
    spec = source.with_suffix(".spec")
    ran = subprocess.run([host, str(module), str(spec)], capture_output=True, text=True, timeout=120)
    if ran.returncode == 2 and "error:" in ran.stderr:
        return "nogpu", name, ran.stderr.strip()[:300]
    if ran.returncode != 0:
        return "mismatch", name, ran.stderr.strip()[:300]
    expected = source.with_suffix(".expect").read_text(encoding="utf-8").strip()
    if ran.stdout.strip() != expected:
        return "mismatch", name, ran.stdout.strip()[:400]
    return "pass", name, ""


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    rync = str(Path(argv[1]).resolve())
    sources = sorted(TESTS.glob("*.ryn"))
    validator = find_tool("spirv-val")
    if not validator:
        print("skip spirv-val not found: validation is skipped (set VULKAN_SDK)")
    with tempfile.TemporaryDirectory(prefix="ryn-spirv-") as directory:
        host, reason = build_host(directory)
        if not host:
            print("skip GPU run: " + reason)
            return 0
        results = [run_case(rync, host, validator, source, directory) for source in sources]
    for status, name, detail in results:
        print(status, name, detail, flush=True)
    failed = [r for r in results if r[0] in ("compilefail", "invalid", "mismatch")]
    if all(r[0] == "nogpu" for r in results):
        print("skip no Vulkan device is available for the GPU run")
        return 0
    print(f"{sum(r[0] == 'pass' for r in results)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
