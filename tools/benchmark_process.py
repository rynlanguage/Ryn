"""Compare native executables in five paired series of 100 checked process launches.

Example: python tools/benchmark_process.py before.exe after.exe --output report.json
Use --timed-output for startup_arithmetic.ryn (checksum line, then elapsed seconds).
Every stdout, stderr and exit code is checked; raw wall and internal timings are saved.
"""
import argparse
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path


def run(path, timed):
    start = time.perf_counter_ns()
    proc = subprocess.run([str(path)], capture_output=True,
                          creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
                          timeout=30)
    wall_ns = time.perf_counter_ns() - start
    if proc.returncode or proc.stderr:
        raise RuntimeError((path.name, proc.returncode, proc.stdout, proc.stderr))
    internal_ns = None
    checksum = proc.stdout
    if timed:
        lines = proc.stdout.splitlines()
        if len(lines) != 2:
            raise RuntimeError(f"Unexpected timed output: {proc.stdout!r}")
        checksum = lines[0]
        elapsed = float(lines[1])
        if not math.isfinite(elapsed) or elapsed <= 0:
            raise RuntimeError(f"Invalid elapsed seconds: {elapsed}")
        internal_ns = round(elapsed * 1_000_000_000)
    return {"wall_ns": wall_ns, "internal_ns": internal_ns}, checksum


def summary(samples, field):
    values = sorted(sample[field] for sample in samples if sample[field] is not None)
    if not values:
        return None
    return {"median_ns": statistics.median(values), "mean_ns": statistics.mean(values),
            "min_ns": values[0], "p95_ns": values[math.ceil(.95 * len(values)) - 1],
            "max_ns": values[-1]}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("before", type=Path)
    p.add_argument("after", type=Path)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--timed-output", action="store_true")
    p.add_argument("--warmup", type=int, default=10)
    args = p.parse_args()
    paths = {name: getattr(args, name).resolve() for name in ("before", "after")}
    expected = None

    def checked(name):
        nonlocal expected
        sample, stdout = run(paths[name], args.timed_output)
        if expected is None:
            expected = stdout
        if stdout != expected:
            raise RuntimeError(f"Output mismatch: {name}: {stdout!r} != {expected!r}")
        return sample

    for _ in range(args.warmup):
        for name in paths:
            checked(name)
    report = {"platform": platform.platform(), "python": platform.python_version(),
              "warmup_per_binary": args.warmup, "series": [], "executables": {}}
    for name, path in paths.items():
        data = path.read_bytes()
        report["executables"][name] = {"path": str(path), "bytes": len(data),
                                       "sha256": hashlib.sha256(data).hexdigest()}
    for number in range(1, 6):
        series = {"number": number, "before": [], "after": []}
        order = ("before", "after") if number % 2 else ("after", "before")
        for _ in range(100):
            for name in order:
                series[name].append(checked(name))
        series["summary"] = {name: {field: summary(series[name], field)
                                    for field in ("wall_ns", "internal_ns")} for name in paths}
        report["series"].append(series)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"Series {number}: " + "; ".join(
            f"{name} wall {series['summary'][name]['wall_ns']['median_ns'] / 1e6:.3f} ms" +
            (f", computation {series['summary'][name]['internal_ns']['median_ns'] / 1e6:.3f} ms"
             if args.timed_output else "") for name in paths), flush=True)
    report["total"] = {name: {field: summary([sample for series in report["series"] for sample in series[name]], field)
                             for field in ("wall_ns", "internal_ns")} for name in paths}
    report["verified_output"] = expected.decode("utf-8")
    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report["total"], indent=2))


if __name__ == "__main__":
    main()
