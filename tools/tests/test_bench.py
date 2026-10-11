"""`ryn bench`: compile timing, warmup, medians, JSON reports, baselines and regressions."""

import json
import unittest

from ryn_harness import IS_WINDOWS, TempDir, run_ryn

WORK = "fun main() {\n    mut sum: i64 = 0\n    for i in 0..200000 { sum += i }\n    echo sum\n}\n"


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def report(result):
    return json.loads(result.stdout)


class BenchTests(unittest.TestCase):
    def test_text_report_shows_compile_and_run_timings(self):
        with TempDir("bench-text") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            result = run_ryn("bench", source, "--iterations", "3", "--warmup", "1")
            self.assertEqual(result.code, 0, result.stderr)
            self.assertIn("work.ryn", result.stdout)
            self.assertIn("compile  median", result.stdout)
            self.assertIn("run      median", result.stdout)
            self.assertIn("(3 runs after 1 warmup)", result.stdout)
            # The program's own output is captured, not shown.
            self.assertNotIn("19999900000", result.stdout)

    def test_json_report_has_the_statistics_of_every_run(self):
        with TempDir("bench-json") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            result = run_ryn("bench", source, "--iterations", "4", "--warmup", "2", "--compile-runs", "2", "--json")
            self.assertEqual(result.code, 0, result.stderr)
            data = report(result)
            self.assertIn("ryn", data)
            self.assertEqual(len(data["benchmarks"]), 1)
            entry = data["benchmarks"][0]
            self.assertTrue(entry["name"].endswith("work.ryn"))
            run = entry["run"]
            self.assertEqual(run["warmup"], 2)
            self.assertEqual(run["runs"], 4)
            self.assertEqual(len(run["samples_ns"]), 4)
            samples = sorted(run["samples_ns"])
            self.assertEqual(run["min_ns"], samples[0])
            self.assertEqual(run["max_ns"], samples[-1])
            # With an even count the median is the mean of the two middle samples.
            self.assertEqual(run["median_ns"], (samples[1] + samples[2]) // 2)
            self.assertEqual(run["mean_ns"], sum(samples) // 4)
            self.assertLessEqual(run["min_ns"], run["median_ns"])
            self.assertLessEqual(run["median_ns"], run["max_ns"])
            compile_stats = entry["compile"]
            self.assertEqual(compile_stats["runs"], 2)
            self.assertEqual(len(compile_stats["samples_ns"]), 2)
            self.assertGreater(compile_stats["median_ns"], 0)

    @unittest.skipIf(IS_WINDOWS, "the peak memory of a child process is not measured on Windows")
    def test_peak_memory_of_the_compiler_and_of_the_program_is_reported(self):
        with TempDir("bench-memory") as temporary:
            small = temporary / "small.ryn"
            write(small, "fun main() {\n    echo 1\n}\n")
            large = temporary / "large.ryn"
            write(large, "fun main() {\n    mut items := Vec<i64>()\n    for i in 0..4000000 { items.push(i) }\n    echo items.len()\n}\n")
            peaks = {}
            for name, source in (("small", small), ("large", large)):
                result = run_ryn("bench", source, "--iterations", "2", "--warmup", "0", "--json")
                self.assertEqual(result.code, 0, result.stderr)
                entry = report(result)["benchmarks"][0]
                self.assertGreater(entry["compile"]["peak_memory_kb"], 1024)
                peaks[name] = entry["run"]["peak_memory_kb"]
            self.assertGreater(peaks["small"], 0)
            # 4 million 8-byte items: far more than the small program, which needs a few hundred kilobytes.
            self.assertGreater(peaks["large"], peaks["small"] + 20000)

    @unittest.skipIf(IS_WINDOWS, "the peak memory of a child process is not measured on Windows")
    def test_memory_growth_past_the_threshold_against_a_baseline_is_a_regression(self):
        with TempDir("bench-memory-baseline") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            saved = temporary / "base.json"
            first = run_ryn("bench", source, "--iterations", "2", "--save-baseline", saved)
            self.assertEqual(first.code, 0, first.stderr)
            data = json.loads(saved.read_text(encoding="utf-8"))
            # Time stays the same; only the memory baseline is made far smaller than any run can use.
            data["benchmarks"][0]["run"]["median_ns"] = 10**15
            data["benchmarks"][0]["run"]["peak_memory_kb"] = 1
            saved.write_text(json.dumps(data), encoding="utf-8")
            result = run_ryn("bench", source, "--iterations", "2", "--baseline", saved)
            self.assertEqual(result.code, 1, result.stdout)
            self.assertIn("baseline memory 1 KB", result.stdout)
            self.assertIn("REGRESSION", result.stdout)

    def test_odd_count_median_is_the_middle_sample(self):
        with TempDir("bench-odd") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            result = run_ryn("bench", source, "--iterations", "5", "--warmup", "0", "--json")
            self.assertEqual(result.code, 0, result.stderr)
            run = report(result)["benchmarks"][0]["run"]
            self.assertEqual(run["median_ns"], sorted(run["samples_ns"])[2])

    def test_baseline_that_matches_passes_and_one_far_below_is_a_regression(self):
        with TempDir("bench-baseline") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            saved = temporary / "baseline" / "base.json"
            first = run_ryn("bench", source, "--iterations", "3", "--save-baseline", saved)
            self.assertEqual(first.code, 0, first.stderr)
            data = json.loads(saved.read_text(encoding="utf-8"))
            self.assertEqual(len(data["benchmarks"]), 1)

            generous = run_ryn("bench", source, "--iterations", "3", "--baseline", saved, "--threshold", "100000")
            self.assertEqual(generous.code, 0, generous.stdout + generous.stderr)
            self.assertIn("baseline median", generous.stdout)
            self.assertNotIn("REGRESSION", generous.stdout)

            data["benchmarks"][0]["run"]["median_ns"] = 1
            saved.write_text(json.dumps(data), encoding="utf-8")
            slower = run_ryn("bench", source, "--iterations", "3", "--baseline", saved)
            self.assertEqual(slower.code, 1)
            self.assertIn("REGRESSION", slower.stdout)
            self.assertIn("1 benchmark(s) regressed", slower.stdout)

    def test_baseline_without_the_program_is_reported_not_failed(self):
        with TempDir("bench-unknown") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            baseline = temporary / "other.json"
            write(baseline, '{"ryn":"0","benchmarks":[]}')
            result = run_ryn("bench", source, "--iterations", "2", "--baseline", baseline)
            self.assertEqual(result.code, 0, result.stderr)
            self.assertIn("baseline none for this program", result.stdout)

    def test_a_benchmark_that_exits_with_a_status_fails(self):
        with TempDir("bench-exit") as temporary:
            source = temporary / "bad.ryn"
            write(source, "use std::system\nfun main() {\n    system::exit(3)\n}\n")
            result = run_ryn("bench", source, "--iterations", "2")
            self.assertEqual(result.code, 1)
            self.assertIn("R0909", result.stderr)
            self.assertIn("status 3", result.stderr)

    def test_a_program_that_does_not_compile_reports_its_diagnostics(self):
        with TempDir("bench-invalid") as temporary:
            source = temporary / "invalid.ryn"
            write(source, "fun main() {\n    echo missing\n}\n")
            result = run_ryn("bench", source)
            self.assertEqual(result.code, 1)
            self.assertIn("R0203", result.stderr)

    def test_options_are_checked(self):
        with TempDir("bench-options") as temporary:
            source = temporary / "work.ryn"
            write(source, WORK)
            zero = run_ryn("bench", source, "--iterations", "0")
            self.assertEqual(zero.code, 1)
            self.assertIn("R0908", zero.stderr)
            word = run_ryn("bench", source, "--warmup", "many")
            self.assertEqual(word.code, 1)
            self.assertIn("takes a whole number", word.stderr)
            missing = run_ryn("bench", temporary / "missing.ryn")
            self.assertEqual(missing.code, 1)
            self.assertIn("no such file or directory", missing.stderr)
            self.assertEqual(run_ryn("bench").code, 1)

    def test_project_benches_directory_times_every_file_by_name(self):
        with TempDir("bench-project") as temporary:
            project = temporary / "app"
            write(project / "ryn.yaml", "name: app\nversion: 0.1.0\n")
            write(project / "src" / "main.ryn", 'fun main() {\n    echo "hi"\n}\n')
            write(project / "benches" / "b_second.ryn", WORK)
            write(project / "benches" / "a_first.ryn", "fun main() {\n    echo 1\n}\n")
            result = run_ryn("bench", project, "--iterations", "2", "--warmup", "0", "--json")
            self.assertEqual(result.code, 0, result.stderr)
            names = [entry["name"] for entry in report(result)["benchmarks"]]
            self.assertEqual(len(names), 2)
            self.assertTrue(names[0].endswith("a_first.ryn"))
            self.assertTrue(names[1].endswith("b_second.ryn"))

    def test_program_arguments_go_after_the_separator(self):
        with TempDir("bench-args") as temporary:
            source = temporary / "args.ryn"
            write(source, "use std::system\nfun main() -> i32 {\n    when arg_count() == 1 {\n        return 0\n    }\n    return 7\n}\n")
            ok = run_ryn("bench", source, "--iterations", "2", "--", "one")
            self.assertEqual(ok.code, 0, ok.stderr)
            failing = run_ryn("bench", source, "--iterations", "2")
            self.assertEqual(failing.code, 1)


if __name__ == "__main__":
    unittest.main()
