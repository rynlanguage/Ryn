//! The Ryn native backend (`selfhost/src/back`, driven by `selfhost/src/native.ryn`) compiles programs of the scalar
//! subset into object files. Each object links with `ld` and prints and exits as the program's `// out:` and `// exit:`
//! lines say; a program outside the subset is declined with the reason.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn temp_dir(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("ryn-native-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

fn repository(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// The IR text that the Ryn semantic analysis writes for a program.
fn ir_of(source: &Path, directory: &Path) -> PathBuf {
    let dump = directory.join("dump");
    fs::create_dir_all(&dump).expect("dump directory");
    let output = Command::new(env!("CARGO_BIN_EXE_ryn"))
        .arg("check")
        .arg(source)
        .env("RYN_DUMP_SEMA", &dump)
        .output()
        .expect("ryn check runs");
    assert!(
        output.status.success(),
        "ryn check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = dump.join("ryn.ir");
    assert!(
        ir.is_file(),
        "the Ryn analysis wrote no IR for {}",
        source.display()
    );
    ir
}

/// The object file of an IR text, or the reason the Ryn backend declines it.
fn object_of(ir: &Path) -> Result<Vec<u8>, String> {
    static DRIVER: OnceLock<PathBuf> = OnceLock::new();
    let driver = DRIVER.get_or_init(|| {
        let path = temp_dir("driver").join("native");
        let build = Command::new(env!("CARGO_BIN_EXE_ryn"))
            .arg("build")
            .arg(repository("selfhost/src/native.ryn"))
            .arg("-o")
            .arg(&path)
            .output()
            .expect("native driver builds");
        assert!(
            build.status.success(),
            "driver build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        path
    });
    let mut output = Command::new(driver)
        .arg(ir)
        .output()
        .expect("the native driver runs");
    if output.status.code() == Some(3)
        && String::from_utf8_lossy(&output.stderr).contains("in a freestanding program")
    {
        output = Command::new(driver)
            .arg(ir)
            .arg("libc")
            .output()
            .expect("libc driver runs");
    }
    if output.status.code() == Some(3) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    assert!(
        output.status.success(),
        "the native driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

/// The expected standard output and exit status of a program's header lines.
fn expectations(source: &Path) -> (String, i32) {
    let text = fs::read_to_string(source).expect("program is readable");
    let mut output = String::new();
    let mut status = 0;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("// out:") {
            output.push_str(value.strip_prefix(' ').unwrap_or(value));
            output.push('\n');
        } else if let Some(value) = line.strip_prefix("// exit:") {
            status = value.trim().parse().expect("exit status is a number");
        }
    }
    (output, status)
}

fn link_object(object: &[u8], directory: &Path) -> PathBuf {
    let object_path = directory.join("program.o");
    fs::write(&object_path, object).expect("object is written");
    let executable = directory.join("program");
    let freestanding = object.windows(8).any(|word| word == b"\0_start\0");
    let mut linker = Command::new(if freestanding { "ld" } else { "cc" });
    if freestanding {
        linker.arg("-static");
    }
    linker.arg("-o").arg(&executable).arg(&object_path);
    if !freestanding {
        linker.arg("-lm");
    }
    let link = linker.output().expect("ld runs");
    assert!(
        link.status.success(),
        "cc failed: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    executable
}

fn link_and_run(object: &[u8], directory: &Path) -> (String, i32) {
    let executable = link_object(object, directory);
    let run = Command::new(&executable)
        .output()
        .expect("the program runs");
    (
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    )
}

#[test]
fn scalar_programs_run_as_objects_built_by_the_ryn_backend() {
    let programs = [
        "tests/suite/collections/for_loop_over_range.ryn",
        "tests/suite/output/print_expression_arguments.ryn",
        "tests/suite/output/print_newline_per_call.ryn",
        "tests/suite/output/echo_escaped_braces.ryn",
        "tests/suite/raii/w3_defer_runs_on_break.ryn",
        "tests/suite/comptime/const_recursive_function.ryn",
        "tests/suite/comptime/w1_comptime_fib_matches_runtime.ryn",
        "tests/suite/comptime/comptime_bit_operations.ryn",
        "tests/suite/comptime/w7_runtime_wrapping_arithmetic.ryn",
        "tests/suite/comptime/const_negative_value.ryn",
        "tests/suite/generics/generic_function_over_two_parameter_structure.ryn",
        "tests/suite/generics/generic_extend_two_parameters.ryn",
        "tests/suite/patterns/r2_j1_trailing_comma_parameters.ryn",
        "tests/suite/contracts/r2_j6_shape_three_default_methods.ryn",
        "tests/suite/ffi/memset_fills_native_memory.ryn",
        "tests/suite/raii/r2_lead_stack_overflow_reports_error.ryn",
        "tests/suite/comptime/w7_runtime_signed_division_signs.ryn",
        "tests/suite/comptime/w7_runtime_shift_count_edges.ryn",
        "tests/suite/comptime/const_owned_string.ryn",
        "tests/suite/guard/move_in_both_branches_accepted.ryn",
        "tests/suite/patterns/choose_as_function_argument.ryn",
        "tests/suite/patterns/nested_pattern_with_literal_inside.ryn",
        "tests/suite/patterns/r2_j6_method_on_pattern_binding.ryn",
        "tests/suite/structs/structure_returned_by_call.ryn",
        "tests/suite/structs/structure_returned_with_mixed_fields.ryn",
        "tests/suite/generics/w2_generic_fn_swaps_pair.ryn",
        "tests/suite/generics/function_takes_generic_structure.ryn",
        "tests/suite/generics/function_returns_generic_structure.ryn",
        "tests/suite/patterns/r2_j4_tuple_pattern_on_call_runs.ryn",
        "tests/suite/patterns/w2_bool_tuple_exhaustive.ryn",
        "tests/suite/result_option/expect_ok.ryn",
        "tests/suite/result_option/is_ok_is_err.ryn",
        "tests/suite/result_option/option_unwrap_or.ryn",
        "tests/suite/result_option/unwrap_or_fallback.ryn",
        "tests/suite/result_option/string_payload_result.ryn",
        "tests/suite/result_option/native_unwrap_none_exits_with_status_one.ryn",
        "tests/suite/result_option/native_expect_error_exits_with_status_one.ryn",
        "tests/suite/result_option/native_unwrap_err_on_ok_exits_with_status_one.ryn",
        "tests/suite/collections/for_loop_over_vec.ryn",
        "tests/suite/collections/vec_growth_keeps_contents.ryn",
        "tests/suite/collections/vec_set_replaces_element.ryn",
        "tests/suite/collections/vec_take_removes_and_shifts.ryn",
        "tests/suite/collections/w4_vec_insert_out_of_bounds_traps.ryn",
        "tests/suite/collections/vec_of_strings_owns_elements.ryn",
        "tests/suite/collections/vec_capacity_and_positional_edits.ryn",
        "tests/suite/collections/w6_stack_vm_jumps.ryn",
        "tests/suite/result_option/option_question_propagates_none.ryn",
        "tests/suite/result_option/question_propagates_err.ryn",
        "tests/suite/result_option/question_runs_scope_cleanup.ryn",
        "tests/suite/result_option/question_unwraps_ok.ryn",
        "tests/suite/result_option/result_widening_with_cast.ryn",
        "tests/suite/result_option/w4_option_question_chain.ryn",
        "tests/suite/output/echo_char_utf8.ryn",
        "tests/suite/comptime/const_char.ryn",
        "tests/suite/raii/native_drop_order_and_moves.ryn",
        "tests/native/nested_result_ownership.ryn",
        "tests/suite/raii/drop_on_early_return.ryn",
        "tests/suite/raii/reassignment_drops_previous.ryn",
        "tests/suite/raii/w8_recursion_100k_frames_drop_on_unwind.ryn",
        "tests/suite/raii/no_double_drop_after_move.ryn",
        "tests/suite/raii/w3_conditional_move_drop_flag.ryn",
        "tests/suite/raii/reverse_declaration_order.ryn",
        "tests/suite/arena/w8_arena_reset_100k_cycles.ryn",
        "tests/suite/output/w6_raii_file_handles.ryn",
        "tests/suite/collections/native_string_char_at_utf8.ryn",
        "tests/suite/collections/native_string_append_grows.ryn",
        "tests/suite/guard/mutable_string_append.ryn",
        "tests/suite/guard/clone_keeps_original_alive.ryn",
        "tests/suite/collections/w6_calc_tokenizer_parser.ryn",
        "tests/suite/output/w6_turnstile_state_machine.ryn",
        "tests/suite/collections/native_string_slice_chars.ryn",
        "tests/suite/collections/w4_string_unicode_indexing.ryn",
        "tests/suite/collections/native_vec_clone_clear_capacity.ryn",
        "tests/suite/collections/native_vec_of_structures.ryn",
        "tests/suite/collections/native_string_append_is_empty_starts_with.ryn",
        "tests/suite/structs/native_nested_field_reads_and_copies.ryn",
        "tests/suite/guard/native_struct_places_through_references.ryn",
        "tests/suite/result_option/native_question_on_structures.ryn",
        "tests/suite/result_option/native_unwrap_structure_payloads.ryn",
        "tests/suite/structs/native_str_fields_of_locals.ryn",
        "tests/suite/collections/native_vec_clone_of_owned_strings.ryn",
        "tests/suite/collections/native_string_push_utf8.ryn",
        "tests/suite/output/native_program_arguments_absent.ryn",
        "tests/suite/collections/native_vec_clone_nested_owned.ryn",
        "tests/suite/collections/native_string_search_and_concat.ryn",
        "tests/suite/collections/native_string_split_pieces.ryn",
        "tests/suite/collections/native_vec_get_option.ryn",
        "tests/suite/collections/native_vec_pop_moves_out.ryn",
        "tests/suite/output/native_file_write_then_read.ryn",
        "tests/suite/collections/vec_clear_and_clone.ryn",
        "tests/suite/raii/exit_releases_live_owners.ryn",
        "tests/suite/output/native_integer_strings_and_eprint.ryn",
        "tests/suite/output/eprint_goes_to_stderr.ryn",
        "tests/suite/output/write_formats_values.ryn",
        "tests/suite/output/write_has_no_newline.ryn",
        "tests/suite/output/write_then_print_interleave.ryn",
        "tests/suite/collections/native_map_growth_remove_strings.ryn",
        "tests/suite/collections/map_insert_get_remove.ryn",
        "tests/suite/collections/map_overwrite_value.ryn",
        "tests/suite/collections/map_contains_key_and_clear.ryn",
        "tests/suite/collections/w4_map_overwrite_and_clear.ryn",
        "tests/suite/collections/native_set_and_get_copy.ryn",
        "tests/suite/collections/map_owned_values.ryn",
        "tests/suite/collections/set_add_contains_remove.ryn",
        "tests/suite/collections/native_vec_sort_scalars_strings_chars.ryn",
        "tests/suite/collections/w4_vec_strings_sort_take.ryn",
        "tests/suite/patterns/native_string_equality_and_null.ryn",
        "tests/suite/patterns/string_literal_patterns.ryn",
        "tests/suite/patterns/owned_string_scrutinee.ryn",
        "tests/suite/ffi/r2_lead_null_literal.ryn",
        "tests/suite/collections/native_string_concat_edges.ryn",
        "tests/suite/generics/generic_struct_fields.ryn",
        "tests/suite/result_option/result_err_choose.ryn",
        "tests/suite/result_option/native_try_parse_integer_bounds.ryn",
        "tests/suite/result_option/w7_try_parse_i32_bad_input.ryn",
        "tests/suite/collections/native_map_clone_is_independent.ryn",
        "tests/suite/collections/set_clone_keeps_set_type.ryn",
        "tests/suite/collections/native_string_byte_slice_boundaries.ryn",
        "tests/suite/result_option/w7_char_count_vs_bytes.ryn",
        "tests/suite/guard/native_struct_references_and_raw_pointers.ryn",
        "tests/suite/arena/w4_arena_struct_roundtrip.ryn",
        "tests/suite/literals/native_float_arithmetic_conversions.ryn",
        "tests/suite/literals/native_float_nan_comparisons.ryn",
        "tests/suite/collections/fixed_array_indexing.ryn",
        "tests/suite/collections/native_fixed_array_values.ryn",
        "tests/suite/collections/native_fixed_array_calls_and_order.ryn",
        "tests/suite/collections/native_fixed_array_element_widths.ryn",
        "tests/suite/collections/native_fixed_array_index_checks_before_value.ryn",
        "tests/suite/collections/native_fixed_array_negative_index.ryn",
        "tests/suite/guard/dynamic_array_index_checked_at_runtime.ryn",
        "tests/suite/comptime/constant_as_array_length.ryn",
        "tests/suite/comptime/constant_array_length_in_signature_and_struct.ryn",
        "tests/suite/literals/native_float_display_shortest.ryn",
        "tests/suite/literals/native_math_functions.ryn",
        "tests/suite/comptime/w7_float_display_specials.ryn",
        "tests/suite/comptime/w7_runtime_float_to_int_edges.ryn",
        "tests/suite/result_option/w7_try_parse_f64_forms.ryn",
        "tests/suite/result_option/native_float_parse_and_text.ryn",
        "tests/suite/collections/native_slices_of_arrays_and_vectors.ryn",
        "tests/suite/collections/native_slice_range_out_of_bounds.ryn",
        "tests/suite/collections/slice_view_of_vec.ryn",
        "tests/suite/generics/generic_slice_parameter.ryn",
        "tests/suite/result_option/native_string_trim_case_find_replace.ryn",
        "tests/suite/result_option/w7_trim_and_case_unicode.ryn",
        "tests/suite/result_option/w7_find_split_replace_scalars.ryn",
        "tests/suite/raii/native_vec_and_map_destructors.ryn",
        "tests/suite/raii/vec_elements_dropped.ryn",
        "tests/suite/raii/w8_map_1e5_values_removed_once.ryn",
        "tests/suite/ffi/native_function_pointers_and_float_calls.ryn",
        "tests/suite/ffi/qsort_calls_back_into_ryn.ryn",
        "tests/suite/ffi/record_returned_by_value_from_c.ryn",
        "tests/suite/ffi/sixteen_byte_float_record_to_and_from_c.ryn",
        "tests/suite/ffi/complex_float_record_to_and_from_c.ryn",
        "tests/suite/ffi/w10_ffi_memcpy_and_memcmp_on_records.ryn",
        "tests/suite/ffi/w10_ffi_memset_local_repr_c_struct.ryn",
        "tests/suite/ffi/w10_ffi_qsort_repr_c_records_by_key.ryn",
        "tests/suite/comptime/const_structure.ryn",
    ];
    let directory = temp_dir("scalar");
    for program in programs {
        let source = repository(program);
        let ir = ir_of(&source, &directory);
        let object =
            object_of(&ir).unwrap_or_else(|reason| panic!("{program} is declined: {reason}"));
        let (output, status) = link_and_run(&object, &directory);
        let (expected_output, expected_status) = expectations(&source);
        assert_eq!(output, expected_output, "{program}: standard output");
        assert_eq!(status, expected_status, "{program}: exit status");
    }
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn a_program_outside_the_subset_is_declined_with_its_reason() {
    let directory = temp_dir("declined");
    let source = directory.join("outside.ryn");
    fs::write(
        &source,
        "fun main() {\n    echo String(\"abc\").replace(\"\", \"-\")\n}\n",
    )
    .expect("program is written");
    let ir = ir_of(&source, &directory);
    let reason = object_of(&ir).expect_err("replacing an empty pattern is outside the subset");
    assert_eq!(reason, "NOT HANDLED: a replace of an empty pattern");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn native_ownership_paths_leave_no_live_allocations() {
    let directory = temp_dir("ownership-paths");
    let ir = ir_of(&repository("tests/native/ownership_paths.ryn"), &directory);
    let object = object_of(&ir).expect("ownership paths compile");
    assert_eq!(link_and_run(&object, &directory), ("0\n0\n".into(), 0));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn thirty_million_temporary_allocations_fit_in_one_gibibyte() {
    let directory = temp_dir("ownership-30m");
    let ir = ir_of(&repository("tests/native/ownership_30m.ryn"), &directory);
    let object = object_of(&ir).expect("allocation stress compiles");
    let executable = link_object(&object, &directory);
    let run = Command::new("bash")
        .args(["-c", "ulimit -v 1048576; exec \"$1\"", "ryn-memory-limit"])
        .arg(executable)
        .output()
        .expect("limited stress runs");
    assert!(
        run.status.success(),
        "allocation stress failed: {:?}: {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, b"480000000\n0\n0\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn freestanding_allocator_traps_on_double_free() {
    use std::os::unix::process::ExitStatusExt;
    let directory = temp_dir("double-free");
    let source = directory.join("double_free.ryn");
    fs::write(&source, "extern \"C\" fun malloc(size: u64) -> *u8;\nextern \"C\" fun free(p: *u8);\nfun main() { p := malloc(16 as u64) free(p) free(p) }\n").unwrap();
    let ir = ir_of(&source, &directory);
    let object = object_of(&ir).expect("allocator detector probe compiles");
    let executable = link_object(&object, &directory);
    let run = Command::new(executable).output().unwrap();
    assert_eq!(
        run.status.signal(),
        Some(4),
        "double free must trap with SIGILL"
    );
    fs::remove_dir_all(directory).unwrap();
}
