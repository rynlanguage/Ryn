use std::{
    fs,
    hint::black_box,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use ryn::{check, check_recovering, codegen, lexer, parser, source::SourceFile};

const FRONTEND_CORPUS: &str = concat!(
    include_str!("../examples/functions.ryn"),
    "\n",
    include_str!("../examples/variables.ryn"),
    "\n",
    include_str!("../examples/conditions.ryn"),
    "\n",
    include_str!("../examples/if_expression.ryn"),
    "\n",
    include_str!("../examples/evaluation_order.ryn"),
    "\n",
    include_str!("../examples/structs.ryn"),
);
const CHECK_SOURCE: &str = include_str!("../examples/evaluation_order.ryn");
const RECOVERY_SOURCE: &str =
    "fun main() { when ) { mut := 1 echo(2 + ) } for index in 0.. { mut := 3 echo(4 + ) } }";
const SAMPLES: usize = 5;

fn measure_samples(iterations: usize, mut run: impl FnMut()) -> Vec<f64> {
    (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..iterations {
                run();
            }
            start.elapsed().as_secs_f64()
        })
        .collect()
}

fn report_timing(name: &str, samples: &[f64], iterations: usize, bytes: Option<usize>) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let median_per_iteration = median * 1_000_000.0 / iterations as f64;
    let range_per_iteration = (
        min * 1_000_000.0 / iterations as f64,
        max * 1_000_000.0 / iterations as f64,
    );
    if let Some(bytes) = bytes {
        let mib_per_second =
            bytes as f64 * iterations as f64 / median.max(f64::EPSILON) / (1024.0 * 1024.0);
        println!(
            "{name}: median {median_per_iteration:.2} us/iteration, {mib_per_second:.2} MiB/s (range {:.2}-{:.2} us/iteration, {SAMPLES} samples, {iterations} iterations/sample, {bytes} bytes)",
            range_per_iteration.0, range_per_iteration.1
        );
    } else {
        println!(
            "{name}: median {:.2} ms/iteration (range {:.2}-{:.2} ms/iteration, {SAMPLES} samples, {iterations} iterations/sample)",
            median_per_iteration / 1_000.0,
            range_per_iteration.0 / 1_000.0,
            range_per_iteration.1 / 1_000.0
        );
    }
}

fn report_frontend(name: &str, iterations: usize, bytes: usize, run: impl Fn(&str)) {
    let samples = measure_samples(iterations, || run(black_box(FRONTEND_CORPUS)));
    report_timing(name, &samples, iterations, Some(bytes));
}

fn unique_output_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let mut output =
        std::env::temp_dir().join(format!("ryn-benchmark-{}-{unique}", std::process::id()));
    if cfg!(windows) {
        output.set_extension("exe");
    }
    output
}

fn main() {
    let corpus_bytes = FRONTEND_CORPUS.len();
    report_frontend("lexer", 10_000, corpus_bytes, |source| {
        black_box(lexer::lex(source).expect("benchmark corpus lexes"));
    });
    report_frontend("parser", 5_000, corpus_bytes, |source| {
        black_box(parser::parse(source).expect("benchmark corpus parses"));
    });

    let check_bytes = CHECK_SOURCE.len();
    let check_samples = measure_samples(2_000, || {
        black_box(check(black_box(CHECK_SOURCE)).expect("benchmark source checks"));
    });
    report_timing(
        "check (lex + parse + semantic analysis)",
        &check_samples,
        2_000,
        Some(check_bytes),
    );

    let recovering_check_samples = measure_samples(2_000, || {
        black_box(
            check_recovering(black_box(CHECK_SOURCE))
                .expect("valid benchmark source passes recovering check"),
        );
    });
    report_timing(
        "recovering check (valid source)",
        &recovering_check_samples,
        2_000,
        Some(check_bytes),
    );

    let recovery_bytes = RECOVERY_SOURCE.len();
    let recovery_samples = measure_samples(2_000, || {
        let diagnostics = check_recovering(black_box(RECOVERY_SOURCE))
            .expect_err("malformed benchmark source reports diagnostics");
        black_box(diagnostics);
    });
    report_timing(
        "recovering check (malformed source)",
        &recovery_samples,
        2_000,
        Some(recovery_bytes),
    );

    let mut diagnostics_source = String::from("fun main() {\n");
    for _ in 0..100 {
        diagnostics_source.push_str("    echo(1 + )\n");
    }
    diagnostics_source.push_str("}\n");
    let diagnostics = check_recovering(&diagnostics_source)
        .expect_err("diagnostic benchmark source reports parser errors");
    assert_eq!(diagnostics.len(), 100, "benchmark diagnostic count changed");
    let diagnostic_source_file = SourceFile::new("diagnostics-benchmark.ryn", diagnostics_source);
    let diagnostic_samples = measure_samples(100, || {
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(&diagnostic_source_file))
            .collect::<Vec<_>>();
        black_box(rendered);
    });
    report_timing(
        "render 100 independent diagnostics",
        &diagnostic_samples,
        100,
        None,
    );

    let ir = check(CHECK_SOURCE).expect("benchmark source checks before code generation");
    let object = codegen::emit_object(&ir).expect("benchmark object warmup succeeds");
    let object_samples = measure_samples(10, || {
        black_box(codegen::emit_object(black_box(&ir)).expect("object emission succeeds"));
    });
    report_timing(
        &format!("Cranelift object emission ({} object bytes)", object.len()),
        &object_samples,
        10,
        None,
    );

    let output = unique_output_path();
    codegen::link_object(&object, &output).expect("link warmup succeeds");
    let link_samples = measure_samples(10, || {
        codegen::link_object(black_box(&object), black_box(&output))
            .expect("object linking succeeds");
    });
    let _ = fs::remove_file(&output);
    report_timing(
        &format!("linking + host runtime ({} object bytes)", object.len()),
        &link_samples,
        10,
        None,
    );

    codegen::build_native(&ir, &output).expect("full native-build warmup succeeds");
    let build_samples = measure_samples(10, || {
        codegen::build_native(black_box(&ir), black_box(&output))
            .expect("full native build succeeds");
    });
    let _ = fs::remove_file(&output);
    report_timing(
        &format!("full native build ({check_bytes} source bytes)"),
        &build_samples,
        10,
        None,
    );
}
