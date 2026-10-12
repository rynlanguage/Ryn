#!/usr/bin/env python3
"""Builds and times each program in Ryn, Rust and C++ (median of 5 runs; 3 for `big`) and writes results.json.

    python3 benches/versus/measure.py /absolute/path/to/rync [program ...]

Needs `rustc` and `g++`. The Ryn build is `rync build`, the Rust build `rustc -O`, the C++ build `g++ -O2`. For `big`, run
`gen_big.py` first. `fsum.ryn` is `benches/simd_fused_sum.ryn`; `hello` is timed over 400 starts."""
import json, os, statistics, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RYNC = sys.argv[1]
STDLIB = str(ROOT / "stdlib/std/src")
os.chdir(HERE)
if not Path("fsum.ryn").exists():
    Path("fsum.ryn").write_text((ROOT / "benches/simd_fused_sum.ryn").read_text())
BUILD = {
    "ryn": lambda a: [RYNC, "build", "--stdlib", STDLIB, "-o", f"ryn_{a}", f"{a}.ryn"],
    "rust": lambda a: ["rustc", "-O", "-o", f"rs_{a}", f"{a}.rs"],
    "cpp": lambda a: ["g++", "-O2", "-o", f"cpp_{a}", f"{a}.cpp"],
}
BINARY = {"ryn": "ryn_{}", "rust": "rs_{}", "cpp": "cpp_{}"}


def median_time(command, runs):
    times, output = [], ""
    for _ in range(runs):
        start = time.perf_counter()
        result = subprocess.run(command, capture_output=True, text=True)
        times.append(time.perf_counter() - start)
        assert result.returncode == 0, (command, result.stderr[:300])
        output = result.stdout.strip()
    return statistics.median(times), output


results = {}
for program in sys.argv[2:] or ["fib", "sieve", "collatz", "hashmap", "fsum", "list", "hello"]:
    for language in BUILD:
        build, _ = median_time(BUILD[language](program), 3 if program == "big" else 5)
        binary = "./" + BINARY[language].format(program)
        if program == "hello":
            starts = []
            for _ in range(5):
                start = time.perf_counter()
                for _ in range(400):
                    subprocess.run([binary], stdout=subprocess.DEVNULL)
                starts.append((time.perf_counter() - start) / 400)
            run, output = statistics.median(starts), "hello"
        else:
            run, output = median_time([binary], 3 if program == "big" else 5)
        results[f"{program}/{language}"] = {"build": build, "run": run, "size": os.path.getsize(binary), "output": output}
        print(program, language, f"build {build:.3f} s, run {run:.4f} s, {os.path.getsize(binary)} bytes")
json.dump(results, open("results.json", "w"), indent=1)
