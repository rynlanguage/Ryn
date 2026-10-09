use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsStr,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use cranelift_codegen::{
    Context,
    ir::{
        AbiParam, BlockArg, FuncRef, InstBuilder, MemFlagsData, Signature, StackSlot,
        StackSlotData, StackSlotKind, TrapCode, Value,
        condcodes::{FloatCC, IntCC},
        types,
    },
    isa::CallConv,
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

const RUNTIME_SHIM_OBJECT: &[u8] = include_bytes!(env!("RYN_RUNTIME_SHIM_OBJECT_PATH"));
/// The runtime with Rust std as one static library: Ryn's own linker uses it on
/// Windows, and the system C compiler driver links it on Unix.
const RUNTIME_LIBRARY: &[u8] = include_bytes!(env!("RYN_RUNTIME_LIBRARY_PATH"));
const LINKER_ENTRY_SOURCE: &str = r#"
mod ryn_entry_ffi {
    unsafe extern "C" {
        pub fn ryn_args_init();
        pub fn ryn_main() -> i32;
    }
}

fn main() {
    // SAFETY: The linked runtime shim owns argument storage for the process lifetime.
    unsafe { ryn_entry_ffi::ryn_args_init() };
    // SAFETY: The Cranelift object always provides this no-argument C ABI entry point.
    let code = unsafe { ryn_entry_ffi::ryn_main() };
    let mut stdout = std::io::stdout();
    let _ = std::io::Write::flush(&mut stdout);
    std::process::exit(code)
}
"#;

use crate::{
    ast::BinaryOp,
    filesystem_ops::FilesystemOp,
    map_ops::MapOp,
    sema::{
        EnumPredicate, IrCallTarget, IrExpression, IrPrintPart, IrStatement, LocalBinding,
        LocalType, RynEnum, RynFunction, RynIr, RynStruct, Type, array_info, map_info,
        storage_slot_width, vec_elem,
    },
    string_ops::StringOp,
    system_ops::SystemOp,
    vector_ops::VecOp,
};

#[derive(Debug)]
pub enum BuildError {
    Internal(String),
    Environment(String),
    Link(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(message) => write!(
                formatter,
                "error[R0900]: internal compiler error during native code generation: {message}"
            ),
            Self::Environment(message) => {
                write!(
                    formatter,
                    "error[R0302]: native build setup failed: {message}"
                )
            }
            Self::Link(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for BuildError {}

/// Compiles typed Ryn IR into a native object file for the current host.
pub fn emit_object(ir: &RynIr) -> Result<Vec<u8>, BuildError> {
    emit_object_with_optimize(ir, "speed")
}

fn emit_object_with_optimize(ir: &RynIr, optimize: &str) -> Result<Vec<u8>, BuildError> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|e| BuildError::Internal(e.to_string()))?;
    flag_builder
        .set("opt_level", optimize)
        .map_err(|e| BuildError::Internal(e.to_string()))?;
    let flags = settings::Flags::new(flag_builder);
    let isa = cranelift_native::builder()
        .map_err(native_backend_setup_error)?
        .finish(flags)
        .map_err(|e| BuildError::Environment(e.to_string()))?;
    let builder = ObjectBuilder::new(isa, "ryn", default_libcall_names())
        .map_err(|e| BuildError::Environment(e.to_string()))?;
    let mut module = ObjectModule::new(builder);
    let pointer_type = module.target_config().pointer_type();

    let print_functions =
        declare_print_functions(&mut module, pointer_type).map_err(BuildError::Internal)?;
    let mut enum_drop_plans = Vec::with_capacity(ir.enums.len());
    for definition in &ir.enums {
        let mut bytes = Vec::new();
        for (variant_index, variant) in definition.variants.iter().enumerate() {
            let mut offset = 0;
            for field in &variant.fields {
                append_enum_drop_entries(&mut bytes, variant_index, *field, offset, &ir.structs);
                offset += value_width(*field, &ir.structs);
            }
        }
        if bytes.is_empty() {
            enum_drop_plans.push(None);
        } else {
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            let id = module
                .declare_anonymous_data(false, false)
                .map_err(|error| BuildError::Internal(error.to_string()))?;
            module
                .define_data(id, &desc)
                .map_err(|error| BuildError::Internal(error.to_string()))?;
            enum_drop_plans.push(Some(id));
        }
    }
    let mut string_data = HashMap::new();
    for value in collect_strings(ir) {
        let mut desc = DataDescription::new();
        desc.define(value.as_bytes().to_vec().into_boxed_slice());
        let id = module
            .declare_anonymous_data(false, false)
            .map_err(|e| BuildError::Internal(e.to_string()))?;
        module
            .define_data(id, &desc)
            .map_err(|e| BuildError::Internal(e.to_string()))?;
        string_data.insert(value, id);
    }

    let mut signatures = Vec::with_capacity(ir.functions.len());
    let mut function_ids = Vec::with_capacity(ir.functions.len());
    let return_types = ir
        .functions
        .iter()
        .map(|function| function.return_type)
        .collect::<Vec<_>>();
    let external_functions = ir
        .functions
        .iter()
        .map(|function| function.external_symbol.is_some())
        .collect::<Vec<_>>();
    let function_parameters = ir
        .functions
        .iter()
        .map(|function| {
            function
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for (index, function) in ir.functions.iter().enumerate() {
        let signature = make_signature(&mut module, function, pointer_type, &ir.structs);
        let symbol = function
            .external_symbol
            .clone()
            .unwrap_or_else(|| format!("ryn_fn_{index}"));
        let linkage = if function.external_symbol.is_some() {
            Linkage::Import
        } else {
            Linkage::Export
        };
        let id = module
            .declare_function(&symbol, linkage, &signature)
            .map_err(|e| BuildError::Internal(e.to_string()))?;
        signatures.push(signature);
        function_ids.push(id);
    }
    let vec_struct_callbacks = declare_vec_struct_callbacks(
        &mut module,
        &ir.structs,
        print_functions,
        pointer_type,
        &function_ids,
    )
    .map_err(BuildError::Internal)?;

    let codegen_env = FunctionCodegenEnv {
        function_ids: &function_ids,
        strings: &string_data,
        print_ids: print_functions,
        vec_struct_callbacks: &vec_struct_callbacks,
        structs: &ir.structs,
        enums: &ir.enums,
        enum_drop_plans: &enum_drop_plans,
        return_types: &return_types,
        external_functions: &external_functions,
        function_parameters: &function_parameters,
    };
    for (index, function) in ir.functions.iter().enumerate() {
        if function.external_symbol.is_some() {
            continue;
        }
        define_function(
            &mut module,
            function,
            function_ids[index],
            &signatures[index],
            &codegen_env,
        )
        .map_err(BuildError::Internal)?;
    }
    define_entrypoint_wrapper(
        &mut module,
        function_ids[ir.main_index],
        ir.functions[ir.main_index].return_type,
    )
    .map_err(BuildError::Internal)?;

    let object = module
        .finish()
        .emit()
        .map_err(|e| BuildError::Internal(e.to_string()))?;
    Ok(object)
}

fn define_entrypoint_wrapper(
    module: &mut ObjectModule,
    main_function: FuncId,
    main_return_type: Option<Type>,
) -> Result<(), String> {
    let mut signature = module.make_signature();
    signature.call_conv = module.isa().default_call_conv();
    signature.returns.push(AbiParam::new(types::I32));
    let wrapper_id = module
        .declare_function("ryn_main", Linkage::Export, &signature)
        .map_err(|error| error.to_string())?;

    let mut context = Context::new();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let main = module.declare_func_in_func(main_function, builder.func);
        let call = builder.ins().call(main, &[]);
        let exit_code = match main_return_type {
            None => builder.ins().iconst(types::I32, 0),
            Some(Type::I32) => builder.inst_results(call)[0],
            Some(_) => {
                return Err("internal error: checked main has an unsupported return type".into());
            }
        };
        builder.ins().return_(&[exit_code]);
        builder.seal_block(entry);
        builder.finalize(module.target_config());
    }
    module
        .define_function(wrapper_id, &mut context)
        .map_err(|error| error.to_string())?;
    module.clear_context(&mut context);
    Ok(())
}

/// Links a Cranelift object file with the small Ryn host runtime.
///
/// The requested output is replaced only after the complete link succeeds.
pub fn link_object(object: &[u8], output: &Path) -> Result<(), BuildError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| BuildError::Environment(e.to_string()))?;
    let temp_dir = BuildTempDir::create().map_err(|error| {
        BuildError::Environment(format!(
            "could not create temporary build directory: {error}"
        ))
    })?;
    let object_path = temp_dir.path().join(if cfg!(windows) {
        "module.obj"
    } else {
        "module.o"
    });
    let wrapper_path = temp_dir.path().join("wrapper.rs");
    fs::write(&object_path, object)
        .map_err(|e| BuildError::Environment(format!("could not write object file: {e}")))?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let runtime_object = temp_dir.path().join(if cfg!(windows) {
        "ryn_runtime_shim.obj"
    } else {
        "ryn_runtime_shim.o"
    });
    fs::write(&runtime_object, RUNTIME_SHIM_OBJECT).map_err(|error| {
        BuildError::Environment(format!("could not write precompiled runtime shim: {error}"))
    })?;
    let staged_output_name = output
        .file_name()
        .ok_or_else(|| BuildError::Environment("output path must name a file".into()))?;
    let output_temp_dir = BuildTempDir::create_in(parent).map_err(|error| {
        BuildError::Environment(format!(
            "could not create temporary output directory: {error}"
        ))
    })?;
    let staged_output = output_temp_dir.path().join(staged_output_name);
    // Windows executables are linked in-process; `RYN_LINKER=rustc` selects the
    // previous rustc-driven link for comparison.
    #[cfg(windows)]
    if std::env::var_os("RYN_LINKER").is_none_or(|linker| linker != "rustc") {
        crate::pe_linker::link(
            &[("module.obj", object)],
            RUNTIME_LIBRARY,
            "ryn_start",
            &staged_output,
        )
        .map_err(|error| BuildError::Link(format!("error[R0300]: link failed: {error}")))?;
        fs::rename(&staged_output, output).map_err(|error| {
            BuildError::Environment(format!(
                "could not install native executable at {}: {error}",
                output.display()
            ))
        })?;
        return Ok(());
    }
    #[cfg(unix)]
    if std::env::var_os("RYN_LINKER").is_none_or(|linker| linker != "rustc") {
        link_with_cc(temp_dir.path(), &object_path, &staged_output)?;
        fs::rename(&staged_output, output).map_err(|error| {
            BuildError::Environment(format!(
                "could not install native executable at {}: {error}",
                output.display()
            ))
        })?;
        return Ok(());
    }
    link_with_rust(
        &rustc,
        &object_path,
        &wrapper_path,
        Some(&runtime_object),
        &staged_output,
    )?;
    fs::rename(&staged_output, output).map_err(|error| {
        BuildError::Environment(format!(
            "could not install native executable at {}: {error}",
            output.display()
        ))
    })?;
    Ok(())
}

/// Emits a native object from Ryn IR and links it into an executable.
pub fn build_native(ir: &RynIr, output: &Path) -> Result<(), BuildError> {
    build_native_with_optimize(ir, output, "speed")
}

pub(crate) fn build_native_with_optimize(
    ir: &RynIr,
    output: &Path,
    optimize: &str,
) -> Result<(), BuildError> {
    if let Some(path) = std::env::var_os("RYN_DUMP_IR") {
        let _ = fs::write(path, crate::ir_codec::encode_ir(ir));
    }
    let object = emit_object_with_optimize(ir, optimize)?;
    link_object(&object, output)
}

fn native_backend_setup_error(reason: &str) -> BuildError {
    BuildError::Environment(format!("Cranelift native backend is unavailable: {reason}"))
}

struct BuildTempDir(PathBuf);

impl BuildTempDir {
    fn create() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    fn create_in(parent: &Path) -> io::Result<Self> {
        static NEXT_BUILD_ID: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let id = NEXT_BUILD_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("ryn-build-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique build directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for BuildTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn declare_print_functions(
    module: &mut ObjectModule,
    pointer_type: types::Type,
) -> Result<PrintFunctionIds, String> {
    let call_conv = module.isa().default_call_conv();
    let string_sig = string_signature(call_conv, pointer_type);
    let string = module
        .declare_function("ryn_print", Linkage::Import, &string_sig)
        .map_err(|e| e.to_string())?;
    let string_equals_sig = string_equals_signature(call_conv, pointer_type);
    let string_equals = module
        .declare_function("ryn_str_equals", Linkage::Import, &string_equals_sig)
        .map_err(|e| e.to_string())?;
    let integers = [
        ("ryn_print_i8", types::I8),
        ("ryn_print_i16", types::I16),
        ("ryn_print_i32", types::I32),
        ("ryn_print_i64", types::I64),
        ("ryn_print_u8", types::I8),
        ("ryn_print_u16", types::I16),
        ("ryn_print_u32", types::I32),
        ("ryn_print_u64", types::I64),
    ]
    .into_iter()
    .map(|(name, ty)| declare_scalar_printer(module, name, ty))
    .collect::<Result<Vec<_>, _>>()?
    .try_into()
    .map_err(|_| "internal error: expected eight integer printers".to_string())?;
    let mut float32_sig = module.make_signature();
    float32_sig.call_conv = module.isa().default_call_conv();
    float32_sig.params.push(AbiParam::new(types::F32));
    let float32 = module
        .declare_function("ryn_print_f32", Linkage::Import, &float32_sig)
        .map_err(|e| e.to_string())?;
    let mut float64_sig = module.make_signature();
    float64_sig.call_conv = module.isa().default_call_conv();
    float64_sig.params.push(AbiParam::new(types::F64));
    let float64 = module
        .declare_function("ryn_print_f64", Linkage::Import, &float64_sig)
        .map_err(|e| e.to_string())?;
    let mut bool_sig = module.make_signature();
    bool_sig.call_conv = module.isa().default_call_conv();
    bool_sig.params.push(AbiParam::new(types::I8));
    let boolean = module
        .declare_function("ryn_print_bool", Linkage::Import, &bool_sig)
        .map_err(|e| e.to_string())?;
    let mut newline_sig = module.make_signature();
    newline_sig.call_conv = module.isa().default_call_conv();
    let newline = module
        .declare_function("ryn_print_newline", Linkage::Import, &newline_sig)
        .map_err(|e| e.to_string())?;
    let mut args_count_sig = module.make_signature();
    args_count_sig.call_conv = call_conv;
    args_count_sig.returns.push(AbiParam::new(types::I32));
    let args_count = module
        .declare_function("ryn_args_count", Linkage::Import, &args_count_sig)
        .map_err(|e| e.to_string())?;
    let mut arg_pointer_sig = module.make_signature();
    arg_pointer_sig.call_conv = call_conv;
    arg_pointer_sig.params.push(AbiParam::new(types::I32));
    arg_pointer_sig.returns.push(AbiParam::new(pointer_type));
    let arg_pointer = module
        .declare_function("ryn_arg_ptr", Linkage::Import, &arg_pointer_sig)
        .map_err(|e| e.to_string())?;
    let mut arg_length_sig = module.make_signature();
    arg_length_sig.call_conv = call_conv;
    arg_length_sig.params.push(AbiParam::new(types::I32));
    arg_length_sig.returns.push(AbiParam::new(types::I64));
    let arg_length = module
        .declare_function("ryn_arg_len", Linkage::Import, &arg_length_sig)
        .map_err(|e| e.to_string())?;
    let mut integer_division_by_zero_sig = module.make_signature();
    integer_division_by_zero_sig.call_conv = call_conv;
    let integer_division_by_zero = module
        .declare_function(
            "ryn_integer_division_by_zero",
            Linkage::Import,
            &integer_division_by_zero_sig,
        )
        .map_err(|e| e.to_string())?;
    let mut array_index_out_of_bounds_sig = module.make_signature();
    array_index_out_of_bounds_sig.call_conv = call_conv;
    let array_index_out_of_bounds = module
        .declare_function(
            "ryn_array_index_out_of_bounds",
            Linkage::Import,
            &array_index_out_of_bounds_sig,
        )
        .map_err(|e| e.to_string())?;
    let mut try_read_file_sig = module.make_signature();
    try_read_file_sig.call_conv = call_conv;
    try_read_file_sig
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]);
    try_read_file_sig.returns.push(AbiParam::new(pointer_type));
    let try_read_file = module
        .declare_function("ryn_fs_try_read_file", Linkage::Import, &try_read_file_sig)
        .map_err(|e| e.to_string())?;
    let mut read_file_result_sig = module.make_signature();
    read_file_result_sig.call_conv = call_conv;
    read_file_result_sig
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]);
    read_file_result_sig
        .returns
        .push(AbiParam::new(pointer_type));
    let read_file_result = module
        .declare_function(
            "ryn_fs_read_file_result",
            Linkage::Import,
            &read_file_result_sig,
        )
        .map_err(|error| error.to_string())?;
    let mut owned_strings = Vec::new();
    for operation in StringOp::ALL {
        let mut signature = module.make_signature();
        for ty in operation.parameters() {
            append_type(&mut signature.params, ty, pointer_type, &[]);
        }
        if let Some(ty) = operation.result() {
            append_type(&mut signature.returns, ty, pointer_type, &[]);
        }
        owned_strings.push(
            module
                .declare_function(operation.symbol(), Linkage::Import, &signature)
                .map_err(|error| error.to_string())?,
        );
    }
    let mut filesystem = Vec::new();
    for operation in FilesystemOp::ALL {
        let mut signature = module.make_signature();
        for ty in operation.parameters() {
            append_type(&mut signature.params, ty, pointer_type, &[]);
        }
        append_type(
            &mut signature.returns,
            operation.result(),
            pointer_type,
            &[],
        );
        filesystem.push(
            module
                .declare_function(operation.symbol(), Linkage::Import, &signature)
                .map_err(|error| error.to_string())?,
        );
    }
    let mut system = Vec::new();
    for operation in SystemOp::ALL {
        let mut signature = module.make_signature();
        for ty in operation.parameters() {
            append_type(&mut signature.params, ty, pointer_type, &[]);
        }
        if let Some(ty) = operation.result() {
            append_type(&mut signature.returns, ty, pointer_type, &[]);
        }
        system.push(
            module
                .declare_function(operation.symbol(), Linkage::Import, &signature)
                .map_err(|error| error.to_string())?,
        );
    }
    let mut owned_vecs = Vec::new();
    for operation in VecOp::ALL {
        let mut signature = module.make_signature();
        match operation {
            VecOp::New => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(pointer_type));
            }
            VecOp::Clone => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(pointer_type));
            }
            VecOp::Drop | VecOp::Clear | VecOp::Len | VecOp::Capacity | VecOp::Reverse => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                if matches!(operation, VecOp::Len | VecOp::Capacity) {
                    signature.returns.push(AbiParam::new(types::I64));
                }
            }
            VecOp::IsEmpty => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(types::I8));
            }
            VecOp::Sort => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
            }
            VecOp::Contains => {
                signature.call_conv = call_conv;
                signature.params.extend([
                    AbiParam::new(pointer_type),
                    AbiParam::new(pointer_type),
                    AbiParam::new(types::I64),
                ]);
                signature.returns.push(AbiParam::new(types::I8));
            }
            VecOp::GetOption => {
                signature.call_conv = call_conv;
                signature.params.extend([
                    AbiParam::new(pointer_type),
                    AbiParam::new(types::I64),
                    AbiParam::new(pointer_type),
                ]);
                signature.returns.push(AbiParam::new(types::I8));
            }
            VecOp::PopOption => {
                signature.call_conv = call_conv;
                signature
                    .params
                    .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
                signature.returns.push(AbiParam::new(types::I8));
            }
            VecOp::Reserve => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
            }
            VecOp::Push => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(pointer_type));
            }
            VecOp::Take | VecOp::Extract => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(pointer_type));
            }
            VecOp::Index => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(pointer_type));
            }
            VecOp::Set => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(pointer_type));
            }
            VecOp::Insert => {
                signature.call_conv = call_conv;
                signature.params.push(AbiParam::new(pointer_type));
                signature.params.push(AbiParam::new(types::I64));
                signature.params.push(AbiParam::new(pointer_type));
            }
        }
        owned_vecs.push(
            module
                .declare_function(operation.symbol(), Linkage::Import, &signature)
                .map_err(|error| error.to_string())?,
        );
    }
    let mut vec_slice_signature = module.make_signature();
    vec_slice_signature.call_conv = call_conv;
    vec_slice_signature.params.extend([
        AbiParam::new(pointer_type),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
        AbiParam::new(pointer_type),
    ]);
    vec_slice_signature
        .returns
        .push(AbiParam::new(pointer_type));
    let vec_slice = module
        .declare_function("ryn_vec_slice", Linkage::Import, &vec_slice_signature)
        .map_err(|error| error.to_string())?;
    let mut maps = Vec::new();
    for operation in MapOp::ALL {
        let mut signature = module.make_signature();
        signature.call_conv = call_conv;
        match operation {
            MapOp::New => {
                signature
                    .params
                    .extend((0..4).map(|_| AbiParam::new(types::I64)));
                signature
                    .params
                    .extend((0..2).map(|_| AbiParam::new(pointer_type)));
                signature.returns.push(AbiParam::new(pointer_type));
            }
            MapOp::Drop | MapOp::Clear => {
                signature.params.push(AbiParam::new(pointer_type));
            }
            MapOp::Clone => {
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(pointer_type));
            }
            MapOp::Len => {
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(types::I64));
            }
            MapOp::IsEmpty => {
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(types::I8));
            }
            MapOp::ContainsKey | MapOp::Remove => {
                signature
                    .params
                    .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
                signature.returns.push(AbiParam::new(types::I8));
            }
            MapOp::Insert => {
                signature
                    .params
                    .extend((0..3).map(|_| AbiParam::new(pointer_type)));
                signature.returns.push(AbiParam::new(types::I8));
            }
            MapOp::Get => {
                signature
                    .params
                    .extend((0..3).map(|_| AbiParam::new(pointer_type)));
                signature.returns.push(AbiParam::new(types::I8));
            }
            MapOp::Keys | MapOp::Values => {
                signature.params.push(AbiParam::new(pointer_type));
                signature.returns.push(AbiParam::new(pointer_type));
            }
        }
        maps.push(
            module
                .declare_function(operation.symbol(), Linkage::Import, &signature)
                .map_err(|error| error.to_string())?,
        );
    }
    let maps: [FuncId; 12] = maps
        .try_into()
        .map_err(|_| "internal error: expected twelve Map runtime functions")?;
    let mut vec_string_element_signature = module.make_signature();
    vec_string_element_signature.call_conv = call_conv;
    vec_string_element_signature
        .params
        .push(AbiParam::new(pointer_type));
    vec_string_element_signature
        .params
        .push(AbiParam::new(pointer_type));
    let vec_string_element = module
        .declare_function(
            "ryn_vec_string_element",
            Linkage::Import,
            &vec_string_element_signature,
        )
        .map_err(|error| error.to_string())?;
    let vec_map_element = module
        .declare_function(
            "ryn_vec_map_element",
            Linkage::Import,
            &vec_string_element_signature,
        )
        .map_err(|error| error.to_string())?;
    let vec_enum_element = module
        .declare_function(
            "ryn_vec_enum_element",
            Linkage::Import,
            &vec_string_element_signature,
        )
        .map_err(|error| error.to_string())?;
    let mut enum_drop_signature = module.make_signature();
    enum_drop_signature.call_conv = call_conv;
    enum_drop_signature.params.push(AbiParam::new(pointer_type));
    let enum_drop = module
        .declare_function("ryn_enum_drop", Linkage::Import, &enum_drop_signature)
        .map_err(|error| error.to_string())?;
    let mut enum_clone_signature = module.make_signature();
    enum_clone_signature.call_conv = call_conv;
    enum_clone_signature
        .params
        .push(AbiParam::new(pointer_type));
    enum_clone_signature
        .returns
        .push(AbiParam::new(pointer_type));
    let enum_clone = module
        .declare_function("ryn_enum_clone", Linkage::Import, &enum_clone_signature)
        .map_err(|error| error.to_string())?;
    let mut enum_new_signature = module.make_signature();
    enum_new_signature.call_conv = call_conv;
    enum_new_signature.params.extend([
        AbiParam::new(types::I64),
        AbiParam::new(pointer_type),
        AbiParam::new(types::I64),
        AbiParam::new(pointer_type),
        AbiParam::new(types::I64),
    ]);
    enum_new_signature.returns.push(AbiParam::new(pointer_type));
    let enum_new = module
        .declare_function("ryn_enum_new", Linkage::Import, &enum_new_signature)
        .map_err(|error| error.to_string())?;
    let mut enum_tag_signature = module.make_signature();
    enum_tag_signature.call_conv = call_conv;
    enum_tag_signature.params.push(AbiParam::new(pointer_type));
    enum_tag_signature.returns.push(AbiParam::new(types::I64));
    let enum_tag = module
        .declare_function("ryn_enum_tag", Linkage::Import, &enum_tag_signature)
        .map_err(|error| error.to_string())?;
    let mut enum_word_signature = module.make_signature();
    enum_word_signature.call_conv = call_conv;
    enum_word_signature
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]);
    enum_word_signature.returns.push(AbiParam::new(types::I64));
    let enum_word = module
        .declare_function("ryn_enum_word", Linkage::Import, &enum_word_signature)
        .map_err(|error| error.to_string())?;
    let mut enum_clear_word_signature = module.make_signature();
    enum_clear_word_signature.call_conv = call_conv;
    enum_clear_word_signature
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]);
    let enum_clear_word = module
        .declare_function(
            "ryn_enum_clear_word",
            Linkage::Import,
            &enum_clear_word_signature,
        )
        .map_err(|error| error.to_string())?;
    Ok(PrintFunctionIds {
        owned_strings: owned_strings
            .try_into()
            .map_err(|_| "invalid String runtime table")?,
        filesystem: filesystem
            .try_into()
            .map_err(|_| "invalid filesystem runtime table")?,
        system: system
            .try_into()
            .map_err(|_| "invalid system runtime table")?,
        owned_vecs: owned_vecs
            .try_into()
            .map_err(|_| "invalid Vec runtime table")?,
        vec_slice,
        maps,
        enum_drop,
        enum_clone,
        enum_new,
        enum_tag,
        enum_word,
        enum_clear_word,
        vec_string_element,
        vec_map_element,
        vec_enum_element,
        string,
        string_equals,
        integers,
        float32,
        float64,
        boolean,
        newline,
        args_count,
        arg_pointer,
        arg_length,
        integer_division_by_zero,
        array_index_out_of_bounds,
        try_read_file,
        read_file_result,
    })
}

fn declare_vec_struct_callbacks(
    module: &mut ObjectModule,
    structs: &[RynStruct],
    runtime: PrintFunctionIds,
    pointer_type: types::Type,
    function_ids: &[FuncId],
) -> Result<Vec<Option<(FuncId, FuncId)>>, String> {
    let mut callbacks = Vec::with_capacity(structs.len());
    for (index, definition) in structs.iter().enumerate() {
        let custom_drop = definition.drop_function.is_some();
        let nested_custom_drop = definition
            .fields
            .iter()
            .any(|field| type_has_custom_drop(field.ty, structs));
        if !custom_drop
            && !nested_custom_drop
            && (!structure_clone_supported(definition, structs)
                || !definition
                    .fields
                    .iter()
                    .any(|field| type_has_owned_data(field.ty, structs)))
        {
            callbacks.push(None);
            continue;
        }
        let call_conv = module.isa().default_call_conv();
        let mut drop_signature = module.make_signature();
        drop_signature.call_conv = call_conv;
        drop_signature.params.push(AbiParam::new(pointer_type));
        let mut clone_signature = module.make_signature();
        clone_signature.call_conv = call_conv;
        clone_signature
            .params
            .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
        let drop_id = module
            .declare_function(
                &format!("ryn_vec_struct_{index}_drop"),
                Linkage::Local,
                &drop_signature,
            )
            .map_err(|error| error.to_string())?;
        let clone_id = module
            .declare_function(
                &format!("ryn_vec_struct_{index}_clone"),
                Linkage::Local,
                &clone_signature,
            )
            .map_err(|error| error.to_string())?;
        callbacks.push(Some((drop_id, clone_id)));
    }
    for (index, definition) in structs.iter().enumerate() {
        let Some((drop_id, clone_id)) = callbacks[index] else {
            continue;
        };
        let call_conv = module.isa().default_call_conv();
        let mut drop_signature = module.make_signature();
        drop_signature.call_conv = call_conv;
        drop_signature.params.push(AbiParam::new(pointer_type));
        let mut clone_signature = module.make_signature();
        clone_signature.call_conv = call_conv;
        clone_signature
            .params
            .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
        define_vec_struct_callback(
            module,
            drop_id,
            drop_signature,
            definition,
            structs,
            &callbacks,
            runtime,
            pointer_type,
            function_ids,
            false,
        )?;
        define_vec_struct_callback(
            module,
            clone_id,
            clone_signature,
            definition,
            structs,
            &callbacks,
            runtime,
            pointer_type,
            function_ids,
            true,
        )?;
    }
    Ok(callbacks)
}

fn structure_clone_supported(definition: &RynStruct, structs: &[RynStruct]) -> bool {
    definition.drop_function.is_none()
        && definition
            .fields
            .iter()
            .all(|field| type_clone_supported(field.ty, structs))
}

fn type_clone_supported(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::Slice(_) => false,
        Type::Struct(id) => structure_clone_supported(&structs[id], structs),
        Type::Vec(id) => type_clone_supported(vec_elem(id), structs),
        Type::Array(id) => type_clone_supported(array_info(id).0, structs),
        _ => true,
    }
}

fn type_has_owned_data(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_) => true,
        Type::Struct(id) => {
            structs[id].drop_function.is_some()
                || structs[id]
                    .fields
                    .iter()
                    .any(|field| type_has_owned_data(field.ty, structs))
        }
        Type::Array(id) => type_has_owned_data(array_info(id).0, structs),
        _ => false,
    }
}

fn type_has_custom_drop(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::Struct(id) => {
            structs[id].drop_function.is_some()
                || structs[id]
                    .fields
                    .iter()
                    .any(|field| type_has_custom_drop(field.ty, structs))
        }
        Type::Array(id) => type_has_custom_drop(array_info(id).0, structs),
        Type::Vec(id) => type_has_custom_drop(vec_elem(id), structs),
        Type::Map(id) => type_has_custom_drop(map_info(id).1, structs),
        Type::Set(id) => type_has_custom_drop(map_info(id).0, structs),
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn define_vec_struct_callback(
    module: &mut ObjectModule,
    function_id: FuncId,
    signature: Signature,
    definition: &RynStruct,
    structs: &[RynStruct],
    callbacks: &[Option<(FuncId, FuncId)>],
    runtime: PrintFunctionIds,
    pointer_type: types::Type,
    function_ids: &[FuncId],
    clone: bool,
) -> Result<(), String> {
    let mut context = Context::new();
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let source = builder.block_params(entry)[0];
        let destination = if clone {
            Some(builder.block_params(entry)[1])
        } else {
            None
        };
        let destination = destination.unwrap_or(source);
        if clone && definition.drop_function.is_some() {
            builder.ins().trap(TrapCode::unwrap_user(1));
        } else if let Some(drop_function) = definition.drop_function {
            let handle_address = callback_field_pointer(&mut builder, source, 0);
            let handle = builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), handle_address, 0);
            let is_null = builder.ins().icmp_imm_s(IntCC::Equal, handle, 0);
            let skip = builder.create_block();
            let invoke = builder.create_block();
            let merge = builder.create_block();
            builder.ins().brif(is_null, skip, &[], invoke, &[]);
            builder.switch_to_block(invoke);
            let mut arguments = Vec::new();
            for field in &definition.fields {
                let address = callback_field_pointer(&mut builder, source, field.slot_offset);
                let field_types = clif_types(field.ty, pointer_type, structs);
                let [field_type] = field_types.as_slice() else {
                    return Err("custom destructor field must occupy one ABI value".into());
                };
                arguments.push(
                    builder
                        .ins()
                        .load(*field_type, MemFlagsData::new(), address, 0),
                );
            }
            let function = module.declare_func_in_func(function_ids[drop_function], builder.func);
            builder.ins().call(function, &arguments);
            builder.ins().jump(merge, &[]);
            builder.seal_block(invoke);
            builder.switch_to_block(skip);
            builder.ins().jump(merge, &[]);
            builder.seal_block(skip);
            builder.switch_to_block(merge);
            builder.seal_block(merge);
            builder.ins().return_(&[]);
        } else if clone {
            for field in &definition.fields {
                emit_vec_struct_field_callback(
                    &mut builder,
                    module,
                    field.ty,
                    source,
                    destination,
                    field.slot_offset,
                    structs,
                    callbacks,
                    runtime,
                    pointer_type,
                    true,
                )?;
            }
        } else {
            for field in definition.fields.iter().rev() {
                emit_vec_struct_field_callback(
                    &mut builder,
                    module,
                    field.ty,
                    source,
                    destination,
                    field.slot_offset,
                    structs,
                    callbacks,
                    runtime,
                    pointer_type,
                    false,
                )?;
            }
        }
        if !(clone && definition.drop_function.is_some()) && definition.drop_function.is_none() {
            builder.ins().return_(&[]);
        }
        builder.seal_block(entry);
        builder.finalize(module.target_config());
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|error| error.to_string())?;
    module.clear_context(&mut context);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_vec_struct_field_callback(
    builder: &mut FunctionBuilder<'_>,
    module: &mut ObjectModule,
    ty: Type,
    source: Value,
    destination: Value,
    slot_offset: usize,
    structs: &[RynStruct],
    callbacks: &[Option<(FuncId, FuncId)>],
    runtime: PrintFunctionIds,
    pointer_type: types::Type,
    clone: bool,
) -> Result<(), String> {
    match ty {
        // Pointer-sized scalars (references, raw and function pointers) were
        // copied with the rest of the record and own nothing to release.
        Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_) => {
            let function_id = match ty {
                Type::OwnedString => {
                    runtime.owned_strings[if clone {
                        StringOp::Clone as usize
                    } else {
                        StringOp::Drop as usize
                    }]
                }
                Type::Vec(_) => {
                    runtime.owned_vecs[if clone {
                        VecOp::Clone as usize
                    } else {
                        VecOp::Drop as usize
                    }]
                }
                Type::Map(_) | Type::Set(_) => {
                    runtime.maps[if clone {
                        MapOp::Clone as usize
                    } else {
                        MapOp::Drop as usize
                    }]
                }
                Type::Enum(_) => {
                    if clone {
                        runtime.enum_clone
                    } else {
                        runtime.enum_drop
                    }
                }
                _ => unreachable!(),
            };
            let source_field = callback_field_pointer(builder, source, slot_offset);
            let value = builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), source_field, 0);
            let function = module.declare_func_in_func(function_id, builder.func);
            let call = builder.ins().call(function, &[value]);
            if clone {
                let cloned = builder.func.dfg.inst_results(call)[0];
                let destination_field = callback_field_pointer(builder, destination, slot_offset);
                builder
                    .ins()
                    .store(MemFlagsData::new(), cloned, destination_field, 0);
            }
        }
        Type::Struct(struct_id) => {
            if let Some((drop_id, clone_id)) = callbacks[struct_id] {
                let function_id = if clone { clone_id } else { drop_id };
                let function = module.declare_func_in_func(function_id, builder.func);
                let source_field = callback_field_pointer(builder, source, slot_offset);
                if clone {
                    let destination_field =
                        callback_field_pointer(builder, destination, slot_offset);
                    builder
                        .ins()
                        .call(function, &[source_field, destination_field]);
                } else {
                    builder.ins().call(function, &[source_field]);
                }
            }
        }
        Type::Array(array_id) => {
            let (element, length) = array_info(array_id);
            let stride = storage_slot_width(element, structs);
            let indices: Box<dyn Iterator<Item = usize>> = if clone {
                Box::new(0..length)
            } else {
                Box::new((0..length).rev())
            };
            for index in indices {
                emit_vec_struct_field_callback(
                    builder,
                    module,
                    element,
                    source,
                    destination,
                    slot_offset + index * stride,
                    structs,
                    callbacks,
                    runtime,
                    pointer_type,
                    clone,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn callback_field_pointer(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    slot_offset: usize,
) -> Value {
    if slot_offset == 0 {
        base
    } else {
        builder.ins().iadd_imm_s(base, (slot_offset * 8) as i64)
    }
}

fn string_signature(call_conv: CallConv, pointer_type: types::Type) -> Signature {
    let mut signature = Signature::new(call_conv);
    signature
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]);
    signature
}

fn string_equals_signature(call_conv: CallConv, pointer_type: types::Type) -> Signature {
    let mut signature = Signature::new(call_conv);
    signature.params.extend([
        AbiParam::new(pointer_type),
        AbiParam::new(types::I64),
        AbiParam::new(pointer_type),
        AbiParam::new(types::I64),
    ]);
    signature.returns.push(AbiParam::new(types::I8));
    signature
}

fn declare_scalar_printer(
    module: &mut ObjectModule,
    name: &str,
    ty: types::Type,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.call_conv = module.isa().default_call_conv();
    signature.params.push(AbiParam::new(ty));
    module
        .declare_function(name, Linkage::Import, &signature)
        .map_err(|error| error.to_string())
}

fn make_signature(
    module: &mut ObjectModule,
    function: &RynFunction,
    pointer_type: types::Type,
    structs: &[RynStruct],
) -> cranelift_codegen::ir::Signature {
    let mut signature = module.make_signature();
    signature.call_conv = module.isa().default_call_conv();
    for parameter in &function.parameters {
        if function.external_symbol.is_some()
            && let Some((size, _)) = c_abi_packed_record_layout(parameter.ty, structs)
        {
            let (size, fields) = (
                size,
                c_abi_packed_record_layout(parameter.ty, structs)
                    .map(|l| l.1)
                    .unwrap_or_default(),
            );
            for ty in c_abi_param_types(size, &fields) {
                signature.params.push(AbiParam::new(ty));
            }
        } else {
            append_type(&mut signature.params, parameter.ty, pointer_type, structs);
        }
    }
    if let Some(ty) = function.return_type {
        let direct_c_record =
            function.external_symbol.is_some() && is_direct_c_abi_record(ty, structs);
        if matches!(ty, Type::Struct(_) | Type::Array(_)) && !direct_c_record {
            // Aggregate returns use a caller-provided stack buffer because the
            // native ABI may not support the record's flattened result count.
            signature.params.insert(0, AbiParam::new(pointer_type));
        } else if function.external_symbol.is_some()
            && let Some((size, fields)) = c_abi_packed_record_layout(ty, structs)
        {
            for ty in c_abi_param_types(size, &fields) {
                signature.returns.push(AbiParam::new(ty));
            }
        } else {
            append_type(&mut signature.returns, ty, pointer_type, structs);
        }
    }
    signature
}

fn c_abi_packed_record_layout(
    ty: Type,
    structs: &[RynStruct],
) -> Option<(usize, Vec<(usize, Type)>)> {
    let Type::Struct(id) = ty else {
        return None;
    };
    if !structs[id].repr_c || structs[id].fields.is_empty() {
        return None;
    }
    let mut offset = 0_usize;
    let mut aggregate_align = 1_usize;
    let mut fields = Vec::with_capacity(structs[id].fields.len());
    for field in &structs[id].fields {
        let (field_size, field_align) = match field.ty {
            Type::I8 | Type::U8 | Type::Bool => (1, 1),
            Type::I16 | Type::U16 => (2, 2),
            Type::I32 | Type::U32 | Type::Char => (4, 4),
            Type::F32 => (4, 4),
            Type::I64 | Type::U64 | Type::F64 => (8, 8),
            _ => return None,
        };
        offset = offset.saturating_add(field_align - 1) / field_align * field_align;
        fields.push((offset, field.ty));
        offset = offset.checked_add(field_size)?;
        aggregate_align = aggregate_align.max(field_align);
    }
    offset = offset.saturating_add(aggregate_align - 1) / aggregate_align * aggregate_align;
    (matches!(offset, 1 | 2 | 4 | 8) || (cfg!(not(windows)) && (9..=16).contains(&offset)))
        .then_some((offset, fields))
}

fn is_direct_c_abi_record(ty: Type, structs: &[RynStruct]) -> bool {
    matches!(ty, Type::Struct(id)
        if structs[id].repr_c
            && (structs[id].fields.len() == 1
                || c_abi_packed_record_layout(ty, structs).is_some()))
}

fn c_abi_integer_type(size: usize) -> types::Type {
    match size {
        1 => types::I8,
        2 => types::I16,
        4 => types::I32,
        8 => types::I64,
        _ => unreachable!("unsupported packed C aggregate size"),
    }
}

fn c_abi_field_clif_type(ty: Type) -> types::Type {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => types::I8,
        Type::I16 | Type::U16 => types::I16,
        Type::I32 | Type::U32 | Type::Char => types::I32,
        Type::I64 | Type::U64 => types::I64,
        Type::F32 => types::I32,
        Type::F64 => types::I64,
        _ => unreachable!("unsupported packed C aggregate field"),
    }
}

/// The ABI value types a packed C record travels in. System V classifies each
/// eightbyte separately: it is SSE (a float register) only when it holds at least one
/// field and every field in it is a float, otherwise INTEGER (an integer register).
/// A record of up to 8 bytes is one eightbyte; a 4-byte float record is a single f32.
fn c_abi_param_types(size: usize, fields: &[(usize, Type)]) -> Vec<types::Type> {
    let sse_eightbyte = |eightbyte: usize| {
        let mut any = false;
        for (_, ty) in fields.iter().filter(|(offset, _)| offset / 8 == eightbyte) {
            if !matches!(ty, Type::F32 | Type::F64) {
                return false;
            }
            any = true;
        }
        any
    };
    if size <= 8 {
        return vec![if sse_eightbyte(0) {
            if size == 4 { types::F32 } else { types::F64 }
        } else {
            c_abi_integer_type(size)
        }];
    }
    (0..2)
        .map(|eightbyte| {
            if sse_eightbyte(eightbyte) {
                types::F64
            } else {
                types::I64
            }
        })
        .collect()
}

fn pack_c_abi_record(
    b: &mut FunctionBuilder<'_>,
    values: &[Value],
    size: usize,
    fields: &[(usize, Type)],
) -> Result<Vec<Value>, String> {
    if values.len() != fields.len() {
        return Err("internal error: C aggregate field count mismatch".into());
    }
    let words = if size > 8 { 2 } else { 1 };
    let mut packed: Vec<Value> = (0..words).map(|_| b.ins().iconst(types::I64, 0)).collect();
    for (value, (offset, ty)) in values.iter().zip(fields) {
        let bits = match ty {
            Type::F32 => b.ins().bitcast(types::I32, MemFlagsData::new(), *value),
            Type::F64 => b.ins().bitcast(types::I64, MemFlagsData::new(), *value),
            _ => *value,
        };
        let mut part = if b.func.dfg.value_type(bits) == types::I64 {
            bits
        } else {
            b.ins().uextend(types::I64, bits)
        };
        let word = offset / 8;
        let shift = offset % 8;
        if shift != 0 {
            part = b.ins().ishl_imm_u(part, (shift * 8) as i64);
        }
        packed[word] = b.ins().bor(packed[word], part);
    }
    if size <= 8 {
        let value = match c_abi_param_types(size, fields)[0] {
            types::F64 => b.ins().bitcast(types::F64, MemFlagsData::new(), packed[0]),
            types::F32 => {
                let bits = b.ins().ireduce(types::I32, packed[0]);
                b.ins().bitcast(types::F32, MemFlagsData::new(), bits)
            }
            types::I64 => packed[0],
            ty => b.ins().ireduce(ty, packed[0]),
        };
        return Ok(vec![value]);
    }
    let types = c_abi_param_types(size, fields);
    Ok(packed
        .into_iter()
        .zip(types)
        .map(|(word, ty)| {
            if ty == types::F64 {
                b.ins().bitcast(types::F64, MemFlagsData::new(), word)
            } else {
                word
            }
        })
        .collect())
}

fn unpack_c_abi_record(
    b: &mut FunctionBuilder<'_>,
    values: &[Value],
    size: usize,
    fields: &[(usize, Type)],
) -> Vec<Value> {
    let words: Vec<Value> = values
        .iter()
        .map(|value| match b.func.dfg.value_type(*value) {
            types::F64 => b.ins().bitcast(types::I64, MemFlagsData::new(), *value),
            types::I64 => *value,
            types::F32 => {
                let bits = b.ins().bitcast(types::I32, MemFlagsData::new(), *value);
                b.ins().uextend(types::I64, bits)
            }
            _ => b.ins().uextend(types::I64, *value),
        })
        .collect();
    let _ = size;
    fields
        .iter()
        .map(|(offset, ty)| {
            let mut field = words[offset / 8];
            if offset % 8 != 0 {
                field = b.ins().ushr_imm_u(field, ((offset % 8) * 8) as i64);
            }
            let width = match ty {
                Type::I8 | Type::U8 | Type::Bool => 8,
                Type::I16 | Type::U16 => 16,
                Type::I32 | Type::U32 | Type::Char => 32,
                Type::I64 | Type::U64 => 64,
                Type::F64 => 64,
                Type::F32 => 32,
                _ => unreachable!("unsupported packed C aggregate field"),
            };
            let field = if width == 64 {
                field
            } else {
                let mask = (1_u64 << width) - 1;
                b.ins().band_imm_u(field, mask as i64)
            };
            let field_type = c_abi_field_clif_type(*ty);
            let bits = if field_type == types::I64 {
                field
            } else {
                b.ins().ireduce(field_type, field)
            };
            match ty {
                Type::F32 => b.ins().bitcast(types::F32, MemFlagsData::new(), bits),
                Type::F64 => b.ins().bitcast(types::F64, MemFlagsData::new(), bits),
                _ => bits,
            }
        })
        .collect()
}

fn append_type(
    params: &mut Vec<AbiParam>,
    ty: Type,
    pointer_type: types::Type,
    structs: &[RynStruct],
) {
    match ty {
        Type::I8 | Type::U8 => params.push(AbiParam::new(types::I8)),
        Type::I16 | Type::U16 => params.push(AbiParam::new(types::I16)),
        Type::I32 | Type::U32 | Type::Char => params.push(AbiParam::new(types::I32)),
        Type::I64 | Type::U64 => params.push(AbiParam::new(types::I64)),
        Type::F32 => params.push(AbiParam::new(types::F32)),
        Type::F64 => params.push(AbiParam::new(types::F64)),
        Type::Bool => params.push(AbiParam::new(types::I8)),
        Type::Str | Type::Slice(_) => {
            params.extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)])
        }
        Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Reference(_, _)
        | Type::RawPointer(_)
        | Type::FunctionPointer(_) => params.push(AbiParam::new(pointer_type)),
        Type::Struct(struct_id) => {
            for field in &structs[struct_id].fields {
                append_type(params, field.ty, pointer_type, structs);
            }
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            for _ in 0..length {
                append_type(params, element, pointer_type, structs);
            }
        }
    }
}

fn clif_integer_type(ty: Type) -> Option<types::Type> {
    match ty {
        Type::I8 | Type::U8 => Some(types::I8),
        Type::I16 | Type::U16 => Some(types::I16),
        Type::I32 | Type::U32 | Type::Char => Some(types::I32),
        Type::I64 | Type::U64 => Some(types::I64),
        _ => None,
    }
}

fn integer_width(ty: Type) -> Option<u16> {
    match ty {
        Type::I8 | Type::U8 => Some(8),
        Type::I16 | Type::U16 => Some(16),
        Type::I32 | Type::U32 => Some(32),
        Type::I64 | Type::U64 => Some(64),
        _ => None,
    }
}

fn is_signed_integer_type(ty: Type) -> bool {
    matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64)
}

fn integer_print_index(ty: Type) -> Option<usize> {
    match ty {
        Type::I8 => Some(0),
        Type::I16 => Some(1),
        Type::I32 => Some(2),
        Type::I64 => Some(3),
        Type::U8 => Some(4),
        Type::U16 => Some(5),
        Type::U32 => Some(6),
        Type::U64 => Some(7),
        _ => None,
    }
}

fn define_function(
    module: &mut ObjectModule,
    function: &RynFunction,
    function_id: FuncId,
    signature: &cranelift_codegen::ir::Signature,
    codegen_env: &FunctionCodegenEnv<'_>,
) -> Result<(), String> {
    let mut context = Context::new();
    context.func.signature = signature.clone();
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let pointer_type = module.target_config().pointer_type();
        for local in &function.local_types {
            let ty = match local {
                LocalType::I32 => types::I32,
                LocalType::I64 => types::I64,
                LocalType::F32 => types::F32,
                LocalType::F64 => types::F64,
                LocalType::Ptr | LocalType::OwnedPtr => pointer_type,
                LocalType::I8 => types::I8,
                LocalType::I16 => types::I16,
            };
            b.declare_var(ty);
        }
        let address_slots = function
            .addressed_slot_types
            .iter()
            .map(|ty| {
                ty.map(|ty| {
                    let (size, align) = reference_storage_layout(ty, codegen_env.structs);
                    b.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size,
                        align.trailing_zeros() as u8,
                    ))
                })
            })
            .collect::<Vec<_>>();
        let owned_slots = function
            .owned_slot_types
            .iter()
            .enumerate()
            .filter_map(|(slot, ty)| ty.map(|ty| (slot, ty)))
            .rev()
            .collect::<Vec<_>>();
        for (slot, _) in &owned_slots {
            let null = b.ins().iconst(pointer_type, 0);
            b.def_var(Variable::from_u32(*slot as u32), null);
        }
        bind_parameters(
            &mut b,
            &function.parameters,
            entry,
            codegen_env.structs,
            usize::from(matches!(
                function.return_type,
                Some(Type::Struct(_) | Type::Array(_))
            )),
        );
        for parameter in &function.parameters {
            if let Some(Some(slot)) = address_slots.get(parameter.slot) {
                let c_layout = matches!(
                    parameter.ty,
                    Type::Struct(id) if codegen_env.structs[id].repr_c
                );
                store_local_variables_to_stack_slot(
                    &mut b,
                    parameter.slot,
                    parameter.ty,
                    *slot,
                    0,
                    pointer_type,
                    codegen_env.structs,
                    c_layout,
                )?;
            }
        }
        let sret_pointer = if matches!(function.return_type, Some(Type::Struct(_) | Type::Array(_)))
        {
            Some(b.block_params(entry)[0])
        } else {
            None
        };
        let print_functions = PrintFunctions {
            owned_strings: codegen_env
                .print_ids
                .owned_strings
                .map(|id| module.declare_func_in_func(id, b.func)),
            filesystem: codegen_env
                .print_ids
                .filesystem
                .map(|id| module.declare_func_in_func(id, b.func)),
            system: codegen_env
                .print_ids
                .system
                .map(|id| module.declare_func_in_func(id, b.func)),
            owned_vecs: codegen_env
                .print_ids
                .owned_vecs
                .map(|id| module.declare_func_in_func(id, b.func)),
            vec_slice: module.declare_func_in_func(codegen_env.print_ids.vec_slice, b.func),
            maps: codegen_env
                .print_ids
                .maps
                .map(|id| module.declare_func_in_func(id, b.func)),
            enum_drop: module.declare_func_in_func(codegen_env.print_ids.enum_drop, b.func),
            enum_clone: module.declare_func_in_func(codegen_env.print_ids.enum_clone, b.func),
            enum_new: module.declare_func_in_func(codegen_env.print_ids.enum_new, b.func),
            enum_tag: module.declare_func_in_func(codegen_env.print_ids.enum_tag, b.func),
            enum_word: module.declare_func_in_func(codegen_env.print_ids.enum_word, b.func),
            enum_clear_word: module
                .declare_func_in_func(codegen_env.print_ids.enum_clear_word, b.func),
            vec_string_element: module
                .declare_func_in_func(codegen_env.print_ids.vec_string_element, b.func),
            vec_map_element: module
                .declare_func_in_func(codegen_env.print_ids.vec_map_element, b.func),
            vec_enum_element: module
                .declare_func_in_func(codegen_env.print_ids.vec_enum_element, b.func),
            string: module.declare_func_in_func(codegen_env.print_ids.string, b.func),
            string_equals: module.declare_func_in_func(codegen_env.print_ids.string_equals, b.func),
            integers: codegen_env
                .print_ids
                .integers
                .map(|id| module.declare_func_in_func(id, b.func)),
            float32: module.declare_func_in_func(codegen_env.print_ids.float32, b.func),
            float64: module.declare_func_in_func(codegen_env.print_ids.float64, b.func),
            boolean: module.declare_func_in_func(codegen_env.print_ids.boolean, b.func),
            newline: module.declare_func_in_func(codegen_env.print_ids.newline, b.func),
            args_count: module.declare_func_in_func(codegen_env.print_ids.args_count, b.func),
            arg_pointer: module.declare_func_in_func(codegen_env.print_ids.arg_pointer, b.func),
            arg_length: module.declare_func_in_func(codegen_env.print_ids.arg_length, b.func),
            integer_division_by_zero: module
                .declare_func_in_func(codegen_env.print_ids.integer_division_by_zero, b.func),
            array_index_out_of_bounds: module
                .declare_func_in_func(codegen_env.print_ids.array_index_out_of_bounds, b.func),
            try_read_file: module.declare_func_in_func(codegen_env.print_ids.try_read_file, b.func),
            read_file_result: module
                .declare_func_in_func(codegen_env.print_ids.read_file_result, b.func),
        };
        let calls = codegen_env
            .function_ids
            .iter()
            .map(|id| module.declare_func_in_func(*id, b.func))
            .collect::<Vec<_>>();
        let vec_struct_callbacks = codegen_env
            .vec_struct_callbacks
            .iter()
            .map(|callbacks| {
                callbacks.map(|(drop, clone)| {
                    (
                        module.declare_func_in_func(drop, b.func),
                        module.declare_func_in_func(clone, b.func),
                    )
                })
            })
            .collect::<Vec<_>>();
        let env = ExprEnv {
            owned_slots: &owned_slots,
            strings: codegen_env.strings,
            calls: &calls,
            vec_struct_callbacks: &vec_struct_callbacks,
            print_functions,
            structs: codegen_env.structs,
            enums: codegen_env.enums,
            enum_drop_plans: codegen_env.enum_drop_plans,
            function_returns: codegen_env.return_types,
            external_functions: codegen_env.external_functions,
            function_parameters: codegen_env.function_parameters,
            function_return: function.return_type,
            sret_pointer,
            address_slots: &address_slots,
            addressed_slot_types: &function.addressed_slot_types,
        };
        let mut seal_state = BlockSealState::default();
        let mut loops = Vec::new();
        if emit_statements(
            &mut b,
            module,
            &env,
            &function.statements,
            &mut seal_state,
            &mut loops,
        )? {
            let returns = if let Some(value) = &function.return_value {
                let compiled = emit_expr(&mut b, module, value, &env, &mut seal_state)?;
                if let Some(return_type @ (Type::Struct(_) | Type::Array(_))) = function.return_type
                {
                    store_aggregate_return(
                        &mut b,
                        return_type,
                        compiled,
                        sret_pointer.ok_or_else(|| {
                            "internal error: missing structure return buffer".to_string()
                        })?,
                        codegen_env.structs,
                    )?;
                    Vec::new()
                } else {
                    flatten_value(compiled)
                }
            } else {
                Vec::new()
            };
            drop_slots(&mut b, &env, env.owned_slots, &mut seal_state);
            b.ins().return_(&returns);
        }
        seal_ready(&mut b, entry, &mut seal_state);
        b.finalize(module.target_config());
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|e| format!("{e:?}"))?;
    module.clear_context(&mut context);
    Ok(())
}

fn bind_parameters(
    b: &mut FunctionBuilder<'_>,
    parameters: &[LocalBinding],
    entry: cranelift_codegen::ir::Block,
    structs: &[RynStruct],
    mut offset: usize,
) {
    let values = b.block_params(entry).to_vec();
    for parameter in parameters {
        bind_parameter_value(
            b,
            parameter.ty,
            parameter.slot,
            &values,
            &mut offset,
            structs,
        );
    }
}

fn reference_storage_layout(ty: Type, structs: &[RynStruct]) -> (u32, u32) {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => (1, 1),
        Type::I16 | Type::U16 => (2, 2),
        Type::I32 | Type::U32 | Type::F32 | Type::Char => (4, 4),
        Type::I64 | Type::U64 | Type::F64 | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            (8, 8)
        }
        Type::Struct(id) if structs.get(id).is_some_and(|definition| definition.repr_c) => {
            let (size, align) = crate::sema::type_layout(ty, structs);
            (size as u32, align as u32)
        }
        Type::Struct(id) => {
            let slot_count = structs
                .get(id)
                .map(|definition| definition.slot_count)
                .unwrap_or(1);
            ((slot_count * 8) as u32, 8)
        }
        // Owning handles are pointer-sized stack homes.
        Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_) => (8, 8),
        _ => unreachable!("reference target was validated as scalar or structure"),
    }
}

fn align_up(offset: usize, alignment: usize) -> usize {
    offset.saturating_add(alignment - 1) / alignment * alignment
}

/// Byte offset of one field in a pointer to a structure.
///
/// An addressed `#[repr(C)]` value is stored with C packing. Every other
/// structure reference keeps the native 8-byte slot stride.
fn field_memory_offset(
    structs: &[RynStruct],
    struct_id: usize,
    field_index: usize,
    c_layout: bool,
) -> Result<i32, String> {
    let definition = structs
        .get(struct_id)
        .ok_or("internal error: reference field structure is missing")?;
    if !c_layout {
        let field = definition
            .fields
            .get(field_index)
            .ok_or("internal error: reference field index is out of range")?;
        return Ok((field.slot_offset * 8) as i32);
    }
    let mut cursor = 0usize;
    for (index, field) in definition.fields.iter().enumerate() {
        let (field_size, field_align) = crate::sema::type_layout(field.ty, structs);
        cursor = align_up(cursor, field_align);
        if index == field_index {
            return Ok(cursor as i32);
        }
        cursor = cursor.saturating_add(field_size);
    }
    Err("internal error: reference field index is out of range".into())
}

/// Stores every leaf variable of a local into its stack home.
#[allow(clippy::too_many_arguments)]
fn store_local_variables_to_stack_slot(
    b: &mut FunctionBuilder<'_>,
    root: usize,
    ty: Type,
    slot: StackSlot,
    base_offset: i32,
    pointer_type: types::Type,
    structs: &[RynStruct],
    c_layout: bool,
) -> Result<(), String> {
    match ty {
        Type::Struct(struct_id) => {
            let mut c_offset = 0usize;
            for field in &structs[struct_id].fields {
                let field_offset = if c_layout {
                    let (field_size, field_align) = crate::sema::type_layout(field.ty, structs);
                    c_offset = align_up(c_offset, field_align);
                    let offset = c_offset;
                    c_offset = c_offset.saturating_add(field_size);
                    offset
                } else {
                    field.slot_offset * 8
                };
                store_local_variables_to_stack_slot(
                    b,
                    root + field.slot_offset,
                    field.ty,
                    slot,
                    base_offset + field_offset as i32,
                    pointer_type,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        Type::Array(array_id) => {
            let (element, length) = array_info(array_id);
            let stride = storage_slot_width(element, structs);
            let (element_size, _) = crate::sema::type_layout(element, structs);
            let byte_stride = if c_layout { element_size } else { stride * 8 };
            for index in 0..length {
                store_local_variables_to_stack_slot(
                    b,
                    root + index * stride,
                    element,
                    slot,
                    base_offset + (index * byte_stride) as i32,
                    pointer_type,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        other => {
            let clif_ty = clif_scalar_type(other, pointer_type)?;
            let value = b.use_var(Variable::from_u32(root as u32));
            let address = b.ins().stack_addr(pointer_type, slot, 0);
            let expected = b.func.dfg.value_type(value);
            if expected != clif_ty {
                return Err("internal error: reference sync variable type mismatch".into());
            }
            b.ins()
                .store(MemFlagsData::new(), value, address, base_offset);
            Ok(())
        }
    }
}

/// Loads every leaf of a local's stack home back into its variables after a
/// mutable method call wrote through the receiver reference.
#[allow(clippy::too_many_arguments)]
fn sync_stack_slot_to_variables(
    b: &mut FunctionBuilder<'_>,
    root: usize,
    ty: Type,
    slot: StackSlot,
    base_offset: i32,
    pointer_type: types::Type,
    structs: &[RynStruct],
    c_layout: bool,
) -> Result<(), String> {
    match ty {
        Type::Struct(struct_id) => {
            let mut c_offset = 0usize;
            for field in &structs[struct_id].fields {
                let field_offset = if c_layout {
                    let (field_size, field_align) = crate::sema::type_layout(field.ty, structs);
                    c_offset = align_up(c_offset, field_align);
                    let offset = c_offset;
                    c_offset = c_offset.saturating_add(field_size);
                    offset
                } else {
                    field.slot_offset * 8
                };
                sync_stack_slot_to_variables(
                    b,
                    root + field.slot_offset,
                    field.ty,
                    slot,
                    base_offset + field_offset as i32,
                    pointer_type,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        Type::Array(array_id) => {
            let (element, length) = array_info(array_id);
            let stride = storage_slot_width(element, structs);
            let (element_size, _) = crate::sema::type_layout(element, structs);
            let byte_stride = if c_layout { element_size } else { stride * 8 };
            for index in 0..length {
                sync_stack_slot_to_variables(
                    b,
                    root + index * stride,
                    element,
                    slot,
                    base_offset + (index * byte_stride) as i32,
                    pointer_type,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        other => {
            let clif_ty = clif_scalar_type(other, pointer_type)?;
            let address = b.ins().stack_addr(pointer_type, slot, 0);
            let value = b
                .ins()
                .load(clif_ty, MemFlagsData::new(), address, base_offset);
            b.def_var(Variable::from_u32(root as u32), value);
            Ok(())
        }
    }
}

fn load_addressed_local(
    b: &mut FunctionBuilder<'_>,
    slot: usize,
    ty: Type,
    address_slots: &[Option<StackSlot>],
    pointer_type: types::Type,
    addressed_types: &[Option<Type>],
    structs: &[RynStruct],
) -> Result<Option<Value>, String> {
    for (root, root_type) in addressed_types.iter().enumerate() {
        let Some(root_type) = root_type else { continue };
        let Some(Some(stack_slot)) = address_slots.get(root) else {
            continue;
        };
        if slot < root || slot >= root + storage_slot_width(*root_type, structs) {
            continue;
        }
        let c_layout = matches!(root_type, Type::Struct(id) if structs[*id].repr_c);
        if let Some(offset) = addressed_leaf_offset(*root_type, slot - root, structs, c_layout) {
            let address = b.ins().stack_addr(pointer_type, *stack_slot, 0);
            return Ok(Some(b.ins().load(
                clif_scalar_type(ty, pointer_type)?,
                MemFlagsData::new(),
                address,
                offset,
            )));
        }
    }
    Ok(None)
}

fn addressed_leaf_offset(
    ty: Type,
    slot: usize,
    structs: &[RynStruct],
    c_layout: bool,
) -> Option<i32> {
    match ty {
        Type::Struct(id) => {
            let mut cursor = 0usize;
            for field in &structs[id].fields {
                let offset = if c_layout {
                    let (size, align) = crate::sema::type_layout(field.ty, structs);
                    cursor = align_up(cursor, align);
                    let offset = cursor;
                    cursor += size;
                    offset
                } else {
                    field.slot_offset * 8
                };
                if slot >= field.slot_offset
                    && slot < field.slot_offset + storage_slot_width(field.ty, structs)
                {
                    return addressed_leaf_offset(
                        field.ty,
                        slot - field.slot_offset,
                        structs,
                        c_layout,
                    )
                    .map(|leaf| offset as i32 + leaf);
                }
            }
            None
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let width = storage_slot_width(element, structs);
            let index = slot / width;
            if index >= length {
                return None;
            }
            let stride = if c_layout {
                let (size, align) = crate::sema::type_layout(element, structs);
                align_up(size, align)
            } else {
                width * 8
            };
            addressed_leaf_offset(element, slot % width, structs, c_layout)
                .map(|leaf| (index * stride) as i32 + leaf)
        }
        _ => (slot == 0).then_some(0),
    }
}

fn bind_parameter_value(
    b: &mut FunctionBuilder<'_>,
    ty: Type,
    slot: usize,
    values: &[Value],
    offset: &mut usize,
    structs: &[RynStruct],
) {
    match ty {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::F32
        | Type::F64
        | Type::Bool
        | Type::Char
        | Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_) => {
            b.def_var(Variable::from_u32(slot as u32), values[*offset]);
            *offset += 1;
        }
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            b.def_var(Variable::from_u32(slot as u32), values[*offset]);
            *offset += 1;
        }
        Type::Str => {
            b.def_var(Variable::from_u32(slot as u32), values[*offset]);
            b.def_var(Variable::from_u32(slot as u32 + 1), values[*offset + 1]);
            *offset += 2;
        }
        Type::Slice(_) => {
            b.def_var(Variable::from_u32(slot as u32), values[*offset]);
            b.def_var(Variable::from_u32(slot as u32 + 1), values[*offset + 1]);
            *offset += 2;
        }
        Type::Struct(struct_id) => {
            for field in &structs[struct_id].fields {
                bind_parameter_value(
                    b,
                    field.ty,
                    slot + field.slot_offset,
                    values,
                    offset,
                    structs,
                );
            }
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            for index in 0..length {
                bind_parameter_value(
                    b,
                    element,
                    slot + index * storage_slot_width(element, structs),
                    values,
                    offset,
                    structs,
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
struct PrintFunctionIds {
    owned_strings: [FuncId; 65],
    filesystem: [FuncId; 29],
    system: [FuncId; 95],
    owned_vecs: [FuncId; 19],
    vec_slice: FuncId,
    maps: [FuncId; 12],
    enum_drop: FuncId,
    enum_clone: FuncId,
    enum_new: FuncId,
    enum_tag: FuncId,
    enum_word: FuncId,
    enum_clear_word: FuncId,
    vec_string_element: FuncId,
    vec_map_element: FuncId,
    vec_enum_element: FuncId,
    string: FuncId,
    string_equals: FuncId,
    integers: [FuncId; 8],
    float32: FuncId,
    float64: FuncId,
    boolean: FuncId,
    newline: FuncId,
    args_count: FuncId,
    arg_pointer: FuncId,
    arg_length: FuncId,
    integer_division_by_zero: FuncId,
    array_index_out_of_bounds: FuncId,
    try_read_file: FuncId,
    read_file_result: FuncId,
}

struct FunctionCodegenEnv<'a> {
    function_ids: &'a [FuncId],
    strings: &'a HashMap<String, DataId>,
    print_ids: PrintFunctionIds,
    vec_struct_callbacks: &'a [Option<(FuncId, FuncId)>],
    structs: &'a [RynStruct],
    enums: &'a [RynEnum],
    enum_drop_plans: &'a [Option<DataId>],
    return_types: &'a [Option<Type>],
    external_functions: &'a [bool],
    function_parameters: &'a [Vec<Type>],
}

#[derive(Clone, Copy)]
struct PrintFunctions {
    owned_strings: [FuncRef; 65],
    filesystem: [FuncRef; 29],
    system: [FuncRef; 95],
    owned_vecs: [FuncRef; 19],
    vec_slice: FuncRef,
    maps: [FuncRef; 12],
    enum_drop: FuncRef,
    enum_clone: FuncRef,
    enum_new: FuncRef,
    enum_tag: FuncRef,
    enum_word: FuncRef,
    enum_clear_word: FuncRef,
    vec_string_element: FuncRef,
    vec_map_element: FuncRef,
    vec_enum_element: FuncRef,
    string: FuncRef,
    string_equals: FuncRef,
    integers: [FuncRef; 8],
    float32: FuncRef,
    float64: FuncRef,
    boolean: FuncRef,
    newline: FuncRef,
    args_count: FuncRef,
    arg_pointer: FuncRef,
    arg_length: FuncRef,
    integer_division_by_zero: FuncRef,
    array_index_out_of_bounds: FuncRef,
    try_read_file: FuncRef,
    read_file_result: FuncRef,
}

fn drop_slots(
    b: &mut FunctionBuilder<'_>,
    env: &ExprEnv<'_>,
    slots: &[(usize, Type)],
    seal_state: &mut BlockSealState,
) {
    for (slot, ty) in slots {
        let pointer = b.use_var(Variable::from_u32(*slot as u32));
        match ty {
            Type::Struct(struct_id) if env.structs[*struct_id].drop_function.is_some() => {
                let drop_function = env.structs[*struct_id].drop_function.unwrap();
                let is_null = b.ins().icmp_imm_s(IntCC::Equal, pointer, 0);
                let skip = b.create_block();
                let invoke = b.create_block();
                let merge = b.create_block();
                let from = b.current_block().expect("drop has a current block");
                b.ins().brif(is_null, skip, &[], invoke, &[]);
                seal_ready(b, from, seal_state);
                b.switch_to_block(invoke);
                let mut arguments = Vec::new();
                for field in &env.structs[*struct_id].fields {
                    for component in 0..crate::sema::storage_slot_width(field.ty, env.structs) {
                        arguments.push(b.use_var(Variable::from_u32(
                            (*slot + field.slot_offset + component) as u32,
                        )));
                    }
                }
                b.ins().call(env.calls[drop_function], &arguments);
                b.ins().jump(merge, &[]);
                seal_ready(b, invoke, seal_state);
                b.switch_to_block(skip);
                b.ins().jump(merge, &[]);
                seal_ready(b, skip, seal_state);
                b.switch_to_block(merge);
                seal_ready(b, merge, seal_state);
                let pointer_type = b.func.dfg.value_type(pointer);
                let null = b.ins().iconst(pointer_type, 0);
                b.def_var(Variable::from_u32(*slot as u32), null);
                continue;
            }
            Type::Vec(_) => {
                b.ins().call(
                    env.print_functions.owned_vecs[VecOp::Drop as usize],
                    &[pointer],
                );
            }
            Type::Map(_) => {
                b.ins()
                    .call(env.print_functions.maps[MapOp::Drop as usize], &[pointer]);
            }
            Type::Set(_) => {
                b.ins()
                    .call(env.print_functions.maps[MapOp::Drop as usize], &[pointer]);
            }
            Type::Enum(_) => {
                b.ins().call(env.print_functions.enum_drop, &[pointer]);
            }
            _ => {
                b.ins().call(
                    env.print_functions.owned_strings[StringOp::Drop as usize],
                    &[pointer],
                );
            }
        }
        let pointer_type = b.func.dfg.value_type(pointer);
        let null = b.ins().iconst(pointer_type, 0);
        b.def_var(Variable::from_u32(*slot as u32), null);
    }
}

fn drop_binding(
    b: &mut FunctionBuilder<'_>,
    env: &ExprEnv<'_>,
    slot: usize,
    ty: Type,
    seal_state: &mut BlockSealState,
) {
    let mut slots = crate::guard::owned_slots(ty, slot, env.structs);
    slots.reverse();
    drop_slots(b, env, &slots, seal_state);
}

fn drop_temporary(b: &mut FunctionBuilder<'_>, runtime: PrintFunctions, value: CompiledValue) {
    match value {
        CompiledValue::StrViewOwned {
            owner,
            temporary: true,
            ..
        } => {
            b.ins()
                .call(runtime.owned_strings[StringOp::Drop as usize], &[owner]);
        }
        CompiledValue::OwnedString {
            ptr,
            temporary: true,
        } => {
            b.ins()
                .call(runtime.owned_strings[StringOp::Drop as usize], &[ptr]);
        }
        CompiledValue::Vec {
            ptr,
            temporary: true,
        } => {
            b.ins()
                .call(runtime.owned_vecs[VecOp::Drop as usize], &[ptr]);
        }
        CompiledValue::Map {
            ptr,
            temporary: true,
        } => {
            b.ins().call(runtime.maps[MapOp::Drop as usize], &[ptr]);
        }
        CompiledValue::Enum {
            ptr,
            temporary: true,
        } => {
            b.ins().call(runtime.enum_drop, &[ptr]);
        }
        CompiledValue::Struct { fields, .. } => {
            for value in fields.into_iter().rev() {
                drop_temporary(b, runtime, value);
            }
        }
        CompiledValue::Array(values) => {
            for value in values.into_iter().rev() {
                drop_temporary(b, runtime, value);
            }
        }
        _ => {}
    }
}

/// A discarded value with a custom destructor is dropped at the end of its statement, the way
/// a named one is: its destructor receives the whole value, unless the handle is null.
fn drop_discarded_destructor(
    b: &mut FunctionBuilder<'_>,
    env: &ExprEnv<'_>,
    drop_function: usize,
    value: CompiledValue,
    seal_state: &mut BlockSealState,
) {
    let arguments = flatten_value(value);
    let Some(&handle) = arguments.first() else {
        return;
    };
    let is_null = b.ins().icmp_imm_s(IntCC::Equal, handle, 0);
    let skip = b.create_block();
    let invoke = b.create_block();
    let merge = b.create_block();
    let from = b.current_block().expect("drop has a current block");
    b.ins().brif(is_null, skip, &[], invoke, &[]);
    seal_ready(b, from, seal_state);
    b.switch_to_block(invoke);
    b.ins().call(env.calls[drop_function], &arguments);
    b.ins().jump(merge, &[]);
    seal_ready(b, invoke, seal_state);
    b.switch_to_block(skip);
    b.ins().jump(merge, &[]);
    seal_ready(b, skip, seal_state);
    b.switch_to_block(merge);
    seal_ready(b, merge, seal_state);
}

fn make_temporary(value: &mut CompiledValue) {
    match value {
        CompiledValue::OwnedString { temporary, .. } => *temporary = true,
        CompiledValue::Vec { temporary, .. } => *temporary = true,
        CompiledValue::Map { temporary, .. } => *temporary = true,
        CompiledValue::Enum { temporary, .. } => *temporary = true,
        CompiledValue::Struct { fields, .. } => {
            for field in fields {
                make_temporary(field);
            }
        }
        CompiledValue::Array(values) => {
            for value in values {
                make_temporary(value);
            }
        }
        _ => {}
    }
}

fn call_result(target: IrCallTarget, env: &ExprEnv<'_>) -> Option<Type> {
    match target {
        IrCallTarget::Function(index) => env.function_returns[index],
        IrCallTarget::IndirectFunctionPointer(id) => crate::sema::function_pointer_info(id).result,
        IrCallTarget::String(operation) => operation.result(),
        IrCallTarget::Filesystem(operation) => Some(operation.result()),
        IrCallTarget::TryReadFile(enum_id) => Some(Type::Enum(enum_id)),
        IrCallTarget::ReadFileResult(enum_id) => Some(Type::Enum(enum_id)),
        IrCallTarget::System(operation) => operation.result(),
        IrCallTarget::Vec(op, elem_id) => op.result(vec_elem(elem_id)),
        IrCallTarget::VecSlice(elem_id) => Some(Type::Slice(elem_id)),
        IrCallTarget::SliceLen => Some(Type::U64),
        IrCallTarget::Map(op, map_id) => {
            let (key, value) = map_info(map_id);
            if op == MapOp::Get {
                env.enums
                    .iter()
                    .position(|definition| {
                        definition.name.starts_with("$RynOption#")
                            && definition.variants.len() == 2
                            && definition.variants[0].name == "Some"
                            && definition.variants[0].fields == [value]
                            && definition.variants[1].name == "None"
                            && definition.variants[1].fields.is_empty()
                    })
                    .map(Type::Enum)
            } else {
                op.result(key, value)
            }
        }
        IrCallTarget::Set(op, map_id) => {
            let (key, _) = map_info(map_id);
            match op {
                MapOp::New | MapOp::Clone => Some(Type::Set(map_id)),
                MapOp::Insert | MapOp::Remove => Some(Type::Bool),
                _ => op.result(key, Type::Bool),
            }
        }
        IrCallTarget::Argument => Some(Type::Str),
        IrCallTarget::ArgumentCount => Some(Type::U32),
        IrCallTarget::EnumNew { enum_id, .. } => Some(Type::Enum(enum_id)),
        IrCallTarget::EnumPredicate(_, _) => Some(Type::Bool),
        IrCallTarget::EnumUnwrapOr { value_type, .. } => Some(value_type),
        IrCallTarget::EnumUnwrap { value_type, .. } => Some(value_type),
        IrCallTarget::VecGetOption { option_id, .. } => Some(Type::Enum(option_id)),
    }
}

fn emit_statements(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    env: &ExprEnv<'_>,
    statements: &[IrStatement],
    seal_state: &mut BlockSealState,
    loops: &mut Vec<LoopBlocks>,
) -> Result<bool, String> {
    for statement in statements {
        let falls_through = match statement {
            IrStatement::Block(statements) => {
                emit_statements(b, module, env, statements, seal_state, loops)?
            }
            IrStatement::Let {
                slot, ty, value, ..
            } => {
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                store_local(b, *slot, *ty, compiled, env.structs)?;
                if let Some(Some(stack_slot)) = env.address_slots.get(*slot) {
                    store_local_variables_to_stack_slot(
                        b,
                        *slot,
                        *ty,
                        *stack_slot,
                        0,
                        module.target_config().pointer_type(),
                        env.structs,
                        matches!(*ty, Type::Struct(id) if env.structs[id].repr_c),
                    )?;
                }
                true
            }
            IrStatement::Assign {
                slot, ty, value, ..
            } => {
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                drop_binding(b, env, *slot, *ty, seal_state);
                store_local(b, *slot, *ty, compiled, env.structs)?;
                if let Some(Some(stack_slot)) = env.address_slots.get(*slot) {
                    store_local_variables_to_stack_slot(
                        b,
                        *slot,
                        *ty,
                        *stack_slot,
                        0,
                        module.target_config().pointer_type(),
                        env.structs,
                        matches!(*ty, Type::Struct(id) if env.structs[id].repr_c),
                    )?;
                }
                true
            }
            IrStatement::FieldAssign { slot, ty, value } => {
                // A raw-pointer alias may have changed other fields since the last call.
                for (root, root_type) in env.addressed_slot_types.iter().enumerate() {
                    let Some(root_type) = root_type else { continue };
                    if *slot < root || *slot >= root + storage_slot_width(*root_type, env.structs) {
                        continue;
                    }
                    if let Some(Some(stack_slot)) = env.address_slots.get(root) {
                        sync_stack_slot_to_variables(
                            b,
                            root,
                            *root_type,
                            *stack_slot,
                            0,
                            module.target_config().pointer_type(),
                            env.structs,
                            matches!(*root_type, Type::Struct(id) if env.structs[id].repr_c),
                        )?;
                    }
                }
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                drop_binding(b, env, *slot, *ty, seal_state);
                store_local(b, *slot, *ty, compiled, env.structs)?;
                // A borrowed receiver reads the structure from its stack home;
                // refresh that copy so it never holds the released field.
                for (root, root_type) in env.addressed_slot_types.iter().enumerate() {
                    let Some(root_type) = root_type else {
                        continue;
                    };
                    let width = storage_slot_width(*root_type, env.structs);
                    if *slot < root || *slot >= root + width {
                        continue;
                    }
                    if let Some(Some(stack_slot)) = env.address_slots.get(root) {
                        store_local_variables_to_stack_slot(
                            b,
                            root,
                            *root_type,
                            *stack_slot,
                            0,
                            module.target_config().pointer_type(),
                            env.structs,
                            matches!(*root_type, Type::Struct(id) if env.structs[id].repr_c),
                        )?;
                    }
                }
                true
            }
            IrStatement::ReferenceFieldAssign {
                pointer,
                struct_id,
                field_index,
                ty: _,
                value,
            } => {
                let pointer = emit_expr(b, module, pointer, env, seal_state)?;
                let CompiledValue::Integer(pointer, _) = pointer else {
                    return Err(
                        "internal error: reference field assignment base is not a pointer".into(),
                    );
                };
                let field = env.structs[*struct_id]
                    .fields
                    .get(*field_index)
                    .ok_or("internal error: reference field index is out of range")?;
                let c_layout = env.structs[*struct_id].repr_c;
                let offset = field_memory_offset(env.structs, *struct_id, *field_index, c_layout)?;
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                // Release the previous field owner before overwriting the handle.
                if matches!(
                    field.ty,
                    Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_)
                ) {
                    // Borrow the stored handle so the old owner itself is
                    // released, not a fresh clone of it.
                    let previous = load_reference_field(
                        b,
                        field.ty,
                        pointer,
                        offset,
                        c_layout,
                        module.target_config().pointer_type(),
                        env.structs,
                        env.print_functions,
                        true,
                    )?;
                    let owned = match field.ty {
                        Type::OwnedString => CompiledValue::OwnedString {
                            ptr: flatten_value(previous)[0],
                            temporary: true,
                        },
                        Type::Vec(_) => CompiledValue::Vec {
                            ptr: flatten_value(previous)[0],
                            temporary: true,
                        },
                        Type::Map(_) | Type::Set(_) => CompiledValue::Map {
                            ptr: flatten_value(previous)[0],
                            temporary: true,
                        },
                        _ => CompiledValue::Enum {
                            ptr: flatten_value(previous)[0],
                            temporary: true,
                        },
                    };
                    drop_temporary(b, env.print_functions, owned);
                }
                store_reference_value(
                    b,
                    field.ty,
                    compiled,
                    pointer,
                    offset,
                    env.structs,
                    c_layout,
                )?;
                true
            }
            IrStatement::DereferenceAssign { pointer, ty, value } => {
                let pointer = emit_expr(b, module, pointer, env, seal_state)?;
                let CompiledValue::Integer(pointer, _) = pointer else {
                    return Err(
                        "internal error: dereference assignment target is not a pointer".into(),
                    );
                };
                let value = emit_expr(b, module, value, env, seal_state)?;
                let c_layout = matches!(ty, Type::Struct(id) if env.structs[*id].repr_c);
                store_reference_value(b, *ty, value, pointer, 0, env.structs, c_layout)?;
                true
            }
            IrStatement::ArrayAssign {
                slot,
                element,
                length,
                index,
                value,
                ..
            } => {
                let index = emit_expr(b, module, index, env, seal_state)?;
                let CompiledValue::Integer(index, index_ty) = index else {
                    return Err(
                        "internal error: checked array assignment index is not integer".into(),
                    );
                };
                let index64 = match index_ty {
                    Type::I8 | Type::I16 | Type::I32 => b.ins().sextend(types::I64, index),
                    Type::U8 | Type::U16 | Type::U32 => b.ins().uextend(types::I64, index),
                    Type::I64 | Type::U64 => index,
                    _ => return Err("internal error: invalid array assignment index type".into()),
                };
                let too_large =
                    b.ins()
                        .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, index64, *length as i64);
                let out_of_bounds =
                    if matches!(index_ty, Type::I8 | Type::I16 | Type::I32 | Type::I64) {
                        let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index64, 0);
                        b.ins().bor(negative, too_large)
                    } else {
                        too_large
                    };
                let error_block = b.create_block();
                let valid_block = b.create_block();
                let from = b
                    .current_block()
                    .ok_or("internal error: array write has no source block")?;
                b.ins()
                    .brif(out_of_bounds, error_block, &[], valid_block, &[]);
                seal_ready(b, from, seal_state);
                b.switch_to_block(error_block);
                b.ins()
                    .call(env.print_functions.array_index_out_of_bounds, &[]);
                b.ins().trap(TrapCode::unwrap_user(1));
                seal_ready(b, error_block, seal_state);
                b.switch_to_block(valid_block);
                let assigned = flatten_value(emit_expr(b, module, value, env, seal_state)?);
                let expected =
                    clif_types(*element, module.target_config().pointer_type(), env.structs);
                if assigned.len() != expected.len() {
                    return Err(
                        "internal error: array assignment value has an incompatible layout".into(),
                    );
                };
                let owned_components = crate::guard::owned_slots(*element, 0, env.structs);
                for element_index in 0..*length {
                    let select = b
                        .ins()
                        .icmp_imm_u(IntCC::Equal, index64, element_index as i64);
                    let base = *slot + element_index * storage_slot_width(*element, env.structs);
                    for (component_index, assigned) in assigned.iter().enumerate() {
                        let component_slot = base + component_index;
                        let current = b.use_var(Variable::from_u32(component_slot as u32));
                        if let Some((_, owner_ty)) = owned_components
                            .iter()
                            .find(|(owner_slot, _)| *owner_slot == component_index)
                        {
                            let pointer_type = b.func.dfg.value_type(current);
                            let null = b.ins().iconst(pointer_type, 0);
                            let old_if_selected = b.ins().select(select, current, null);
                            let drop_op = match owner_ty {
                                Type::Vec(_) => {
                                    env.print_functions.owned_vecs[VecOp::Drop as usize]
                                }
                                Type::Enum(_) => env.print_functions.enum_drop,
                                Type::Map(_) => env.print_functions.maps[MapOp::Drop as usize],
                                Type::Set(_) => env.print_functions.maps[MapOp::Drop as usize],
                                _ => env.print_functions.owned_strings[StringOp::Drop as usize],
                            };
                            b.ins().call(drop_op, &[old_if_selected]);
                        }
                        let updated = b.ins().select(select, *assigned, current);
                        b.def_var(Variable::from_u32(component_slot as u32), updated);
                    }
                }
                seal_ready(b, valid_block, seal_state);
                true
            }
            IrStatement::Print { value, ty } => {
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                emit_print_value(
                    b,
                    module,
                    env.print_functions,
                    env.strings,
                    env.structs,
                    env.enums,
                    *ty,
                    compiled,
                    seal_state,
                )?;
                b.ins().call(env.print_functions.newline, &[]);
                true
            }
            IrStatement::PrintTemplate(parts) => {
                for part in parts {
                    let (value, ty) = match part {
                        IrPrintPart::Text(text) => (IrExpression::String(text.clone()), Type::Str),
                        IrPrintPart::Value { value, ty } => (value.clone(), *ty),
                    };
                    let compiled = emit_expr(b, module, &value, env, seal_state)?;
                    emit_print_value(
                        b,
                        module,
                        env.print_functions,
                        env.strings,
                        env.structs,
                        env.enums,
                        ty,
                        compiled,
                        seal_state,
                    )?;
                }
                b.ins().call(env.print_functions.newline, &[]);
                true
            }
            IrStatement::Call { target, arguments } => {
                if matches!(target, IrCallTarget::System(SystemOp::Exit)) {
                    let [argument] = arguments.as_slice() else {
                        return Err("internal error: exit requires one argument".into());
                    };
                    let CompiledValue::Integer(code, Type::I32) =
                        emit_expr(b, module, argument, env, seal_state)?
                    else {
                        return Err("internal error: exit code is not i32".into());
                    };
                    // Evaluate the exit code first, then release every live compiler-known
                    // owner before the runtime terminates the process.
                    drop_slots(b, env, env.owned_slots, seal_state);
                    b.ins()
                        .call(env.print_functions.system[SystemOp::Exit as usize], &[code]);
                    b.ins().trap(TrapCode::unwrap_user(1));
                    false
                } else if matches!(target, IrCallTarget::System(SystemOp::Panic)) {
                    emit_call(b, module, *target, arguments, env, seal_state)?;
                    drop_slots(b, env, env.owned_slots, seal_state);
                    let failure_code = b.ins().iconst(types::I32, 1);
                    b.ins().call(
                        env.print_functions.system[SystemOp::Exit as usize],
                        &[failure_code],
                    );
                    b.ins().trap(TrapCode::unwrap_user(1));
                    false
                } else if matches!(
                    target,
                    IrCallTarget::System(SystemOp::Assert | SystemOp::AssertMessage)
                ) {
                    emit_assertion(b, module, *target, arguments, env, seal_state)?;
                    true
                } else {
                    let result = emit_call(b, module, *target, arguments, env, seal_state)?;
                    if let Some(ty) = call_result(*target, env) {
                        let mut offset = 0;
                        let result =
                            compiled_value_from_type(ty, &result, &mut offset, env.structs)?;
                        match ty {
                            Type::Struct(struct_id)
                                if env.structs[struct_id].drop_function.is_some() =>
                            {
                                let drop_function = env.structs[struct_id].drop_function.unwrap();
                                drop_discarded_destructor(
                                    b,
                                    env,
                                    drop_function,
                                    result,
                                    seal_state,
                                );
                            }
                            _ => drop_temporary(b, env.print_functions, result),
                        }
                    }
                    true
                }
            }
            IrStatement::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = as_bool(emit_expr(b, module, condition, env, seal_state)?)?;
                let then_block = b.create_block();
                let merge_block = b.create_block();
                let else_block = if else_body.is_empty() {
                    merge_block
                } else {
                    b.create_block()
                };
                let from = b
                    .current_block()
                    .ok_or_else(|| "internal error: missing current block".to_string())?;
                b.ins().brif(condition, then_block, &[], else_block, &[]);
                seal_ready(b, from, seal_state);
                b.switch_to_block(then_block);
                let then_falls_through =
                    emit_statements(b, module, env, then_body, seal_state, loops)?;
                if then_falls_through {
                    b.ins().jump(merge_block, &[]);
                }
                seal_ready(b, then_block, seal_state);
                let else_falls_through = if !else_body.is_empty() {
                    b.switch_to_block(else_block);
                    let falls_through =
                        emit_statements(b, module, env, else_body, seal_state, loops)?;
                    if falls_through {
                        b.ins().jump(merge_block, &[]);
                    }
                    seal_ready(b, else_block, seal_state);
                    falls_through
                } else {
                    true
                };
                if then_falls_through || else_falls_through {
                    b.switch_to_block(merge_block);
                    seal_ready(b, merge_block, seal_state);
                    true
                } else {
                    seal_ready(b, merge_block, seal_state);
                    false
                }
            }
            IrStatement::While {
                setup,
                condition,
                body,
            } => {
                let header = b.create_block();
                let loop_body = b.create_block();
                let exit = b.create_block();
                let from = b
                    .current_block()
                    .ok_or_else(|| "internal error: missing loop predecessor".to_string())?;
                b.ins().jump(header, &[]);
                seal_ready(b, from, seal_state);
                b.switch_to_block(header);
                seal_state.deferred.push(header);
                let _ = emit_statements(b, module, env, setup, seal_state, loops)?;
                let condition = as_bool(emit_expr(b, module, condition, env, seal_state)?)?;
                b.ins().brif(condition, loop_body, &[], exit, &[]);
                b.switch_to_block(loop_body);
                loops.push(LoopBlocks {
                    continue_target: header,
                    break_target: exit,
                });
                let body_falls_through = emit_statements(b, module, env, body, seal_state, loops)?;
                loops.pop();
                if body_falls_through {
                    b.ins().jump(header, &[]);
                }
                seal_ready(b, loop_body, seal_state);
                let deferred_header = seal_state.deferred.pop();
                debug_assert_eq!(deferred_header, Some(header));
                seal_ready(b, header, seal_state);
                b.switch_to_block(exit);
                seal_ready(b, exit, seal_state);
                true
            }
            IrStatement::For {
                slot,
                end_slot,
                ty,
                start,
                end,
                inclusive,
                body,
            } => {
                let start = emit_expr(b, module, start, env, seal_state)?;
                store_local(b, *slot, *ty, start, env.structs)?;
                let end = emit_expr(b, module, end, env, seal_state)?;
                store_local(b, *end_slot, *ty, end, env.structs)?;

                let header = b.create_block();
                let loop_body = b.create_block();
                let increment = b.create_block();
                let exit = b.create_block();
                let unsigned = matches!(ty, Type::U8 | Type::U16 | Type::U32 | Type::U64);
                let from = b
                    .current_block()
                    .ok_or_else(|| "internal error: missing for-loop predecessor".to_string())?;
                b.ins().jump(header, &[]);
                seal_ready(b, from, seal_state);

                b.switch_to_block(header);
                seal_state.deferred.push(header);
                let current = b.use_var(Variable::from_u32(*slot as u32));
                let end = b.use_var(Variable::from_u32(*end_slot as u32));
                // An inclusive range keeps going while `current <= end`; the increment below
                // leaves before stepping past `end`, so the counter never wraps at the type's limit.
                let condition = match (unsigned, *inclusive) {
                    (true, false) => b.ins().icmp(IntCC::UnsignedLessThan, current, end),
                    (true, true) => b.ins().icmp(IntCC::UnsignedLessThanOrEqual, current, end),
                    (false, false) => b.ins().icmp(IntCC::SignedLessThan, current, end),
                    (false, true) => b.ins().icmp(IntCC::SignedLessThanOrEqual, current, end),
                };
                b.ins().brif(condition, loop_body, &[], exit, &[]);

                b.switch_to_block(loop_body);
                loops.push(LoopBlocks {
                    continue_target: increment,
                    break_target: exit,
                });
                let body_falls_through = emit_statements(b, module, env, body, seal_state, loops)?;
                loops.pop();
                if body_falls_through {
                    b.ins().jump(increment, &[]);
                }
                seal_ready(b, loop_body, seal_state);

                b.switch_to_block(increment);
                let step = if *inclusive {
                    // The last value of an inclusive range is `end` itself: leave without stepping.
                    let current = b.use_var(Variable::from_u32(*slot as u32));
                    let last = b.use_var(Variable::from_u32(*end_slot as u32));
                    let at_last = b.ins().icmp(IntCC::Equal, current, last);
                    let step = b.create_block();
                    b.ins().brif(at_last, exit, &[], step, &[]);
                    seal_ready(b, increment, seal_state);
                    b.switch_to_block(step);
                    step
                } else {
                    increment
                };
                let current = b.use_var(Variable::from_u32(*slot as u32));
                let value_type = clif_integer_type(*ty)
                    .ok_or_else(|| "internal error: for-loop has a non-integer type".to_string())?;
                let one = b.ins().iconst(value_type, 1);
                let next = b.ins().iadd(current, one);
                b.def_var(Variable::from_u32(*slot as u32), next);
                b.ins().jump(header, &[]);
                seal_ready(b, step, seal_state);

                let deferred_header = seal_state.deferred.pop();
                debug_assert_eq!(deferred_header, Some(header));
                seal_ready(b, header, seal_state);
                b.switch_to_block(exit);
                seal_ready(b, exit, seal_state);
                true
            }
            IrStatement::Break => {
                let loop_blocks = loops
                    .last()
                    .ok_or_else(|| "internal error: `break` has no enclosing loop".to_string())?;
                b.ins().jump(loop_blocks.break_target, &[]);
                false
            }
            IrStatement::Continue => {
                let loop_blocks = loops.last().ok_or_else(|| {
                    "internal error: `continue` has no enclosing loop".to_string()
                })?;
                b.ins().jump(loop_blocks.continue_target, &[]);
                false
            }
            IrStatement::Return { value } => {
                let values = if let Some(value) = value {
                    let compiled = emit_expr(b, module, value, env, seal_state)?;
                    if let Some(return_type @ (Type::Struct(_) | Type::Array(_))) =
                        env.function_return
                    {
                        let ptr = env.sret_pointer.ok_or_else(|| {
                            "internal error: missing structure return buffer".to_string()
                        })?;
                        store_aggregate_return(b, return_type, compiled, ptr, env.structs)?;
                        Vec::new()
                    } else {
                        flatten_value(compiled)
                    }
                } else {
                    Vec::new()
                };
                drop_slots(b, env, env.owned_slots, seal_state);
                b.ins().return_(&values);
                false
            }
            IrStatement::Drop { slots } => {
                drop_slots(b, env, slots, seal_state);
                true
            }
        };
        if !falls_through {
            return Ok(false);
        }
    }
    Ok(true)
}

fn emit_assertion(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    target: IrCallTarget,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<(), String> {
    let IrCallTarget::System(operation @ (SystemOp::Assert | SystemOp::AssertMessage)) = target
    else {
        return Err("internal error: assertion helper received a non-assert operation".into());
    };
    let mut borrowed = Vec::with_capacity(arguments.len());
    let mut values = Vec::new();
    for argument in arguments {
        let value = emit_expr(b, module, argument, env, seal_state)?;
        values.extend(flatten_value(value.clone()));
        borrowed.push(value);
    }
    let Some(CompiledValue::Bool(condition)) = borrowed.first() else {
        return Err("internal error: assertion condition is not bool".into());
    };
    let condition = *condition;
    b.ins()
        .call(env.print_functions.system[operation as usize], &values);
    for value in borrowed.into_iter().rev() {
        drop_temporary(b, env.print_functions, value);
    }

    let success = b.create_block();
    let failure = b.create_block();
    let from = b
        .current_block()
        .ok_or_else(|| "internal error: assertion has no current block".to_string())?;
    b.ins().brif(condition, success, &[], failure, &[]);
    seal_ready(b, from, seal_state);

    b.switch_to_block(failure);
    drop_slots(b, env, env.owned_slots, seal_state);
    let failure_code = b.ins().iconst(types::I32, 1);
    b.ins().call(
        env.print_functions.system[SystemOp::Exit as usize],
        &[failure_code],
    );
    b.ins().trap(TrapCode::unwrap_user(1));
    seal_ready(b, failure, seal_state);

    b.switch_to_block(success);
    seal_ready(b, success, seal_state);
    Ok(())
}

#[derive(Clone, Copy)]
struct LoopBlocks {
    continue_target: cranelift_codegen::ir::Block,
    break_target: cranelift_codegen::ir::Block,
}

#[allow(clippy::too_many_arguments)]
fn emit_print_value(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    structs: &[RynStruct],
    enums: &[RynEnum],
    ty: Type,
    compiled: CompiledValue,
    seal_state: &mut BlockSealState,
) -> Result<(), String> {
    match (ty, compiled) {
        (Type::Char, CompiledValue::Integer(value, Type::Char)) => {
            b.ins().call(
                print_functions.owned_strings[StringOp::PrintChar as usize],
                &[value],
            );
        }
        (Type::OwnedString, value @ CompiledValue::OwnedString { .. }) => {
            let CompiledValue::OwnedString { ptr, .. } = value else {
                unreachable!()
            };
            b.ins().call(
                print_functions.owned_strings[StringOp::Print as usize],
                &[ptr],
            );
            drop_temporary(b, print_functions, value);
        }
        (Type::Enum(enum_id), value @ CompiledValue::Enum { ptr, temporary }) => {
            emit_print_enum(
                b,
                module,
                print_functions,
                strings,
                structs,
                enums,
                enum_id,
                ptr,
                seal_state,
            )?;
            if temporary {
                drop_temporary(b, print_functions, value);
            }
        }
        (ty, CompiledValue::Integer(value, actual)) if ty == actual => {
            let index = integer_print_index(ty)
                .ok_or_else(|| "internal error: missing integer printer".to_string())?;
            b.ins().call(print_functions.integers[index], &[value]);
        }
        (Type::F32, CompiledValue::F32(value)) => {
            b.ins().call(print_functions.float32, &[value]);
        }
        (Type::F64, CompiledValue::F64(value)) => {
            b.ins().call(print_functions.float64, &[value]);
        }
        (Type::Str, CompiledValue::Str { ptr, len }) => {
            b.ins().call(print_functions.string, &[ptr, len]);
        }
        (Type::Bool, CompiledValue::Bool(value)) => {
            b.ins().call(print_functions.boolean, &[value]);
        }
        (
            Type::Struct(struct_id),
            CompiledValue::Struct {
                struct_id: actual,
                fields,
            },
        ) if struct_id == actual => {
            emit_print_struct(
                b,
                module,
                print_functions,
                strings,
                structs,
                enums,
                struct_id,
                &fields,
                seal_state,
            )?;
        }
        _ => return Err("internal error: print value differs from checked type".into()),
    }
    Ok(())
}

fn emit_struct_equality(
    b: &mut FunctionBuilder<'_>,
    print_functions: PrintFunctions,
    structs: &[RynStruct],
    struct_id: usize,
    left: CompiledValue,
    right: CompiledValue,
) -> Result<Value, String> {
    let CompiledValue::Struct {
        struct_id: left_id,
        fields: left_fields,
    } = left
    else {
        return Err("internal error: left structure equality operand is not a structure".into());
    };
    let CompiledValue::Struct {
        struct_id: right_id,
        fields: right_fields,
    } = right
    else {
        return Err("internal error: right structure equality operand is not a structure".into());
    };
    if left_id != struct_id || right_id != struct_id {
        return Err("internal error: structure equality operand type mismatch".into());
    }
    let definition = structs
        .get(struct_id)
        .ok_or_else(|| "internal error: structure equality type is out of range".to_string())?;
    if left_fields.len() != definition.fields.len() || right_fields.len() != definition.fields.len()
    {
        return Err("internal error: structure equality field count mismatch".into());
    }

    let mut equal = None;
    for (field, (left, right)) in definition
        .fields
        .iter()
        .zip(left_fields.into_iter().zip(right_fields))
    {
        let field_equal = emit_equality_value(b, print_functions, structs, field.ty, left, right)?;
        equal = Some(if let Some(previous) = equal {
            b.ins().band(previous, field_equal)
        } else {
            field_equal
        });
    }
    equal.ok_or_else(|| "internal error: empty structures cannot be compared".into())
}

fn emit_equality_value(
    b: &mut FunctionBuilder<'_>,
    print_functions: PrintFunctions,
    structs: &[RynStruct],
    ty: Type,
    left: CompiledValue,
    right: CompiledValue,
) -> Result<Value, String> {
    match (ty, left, right) {
        (
            Type::OwnedString,
            left @ CompiledValue::OwnedString { .. },
            right @ CompiledValue::OwnedString { .. },
        ) => {
            let CompiledValue::OwnedString { ptr: left_ptr, .. } = left else {
                unreachable!()
            };
            let CompiledValue::OwnedString { ptr: right_ptr, .. } = right else {
                unreachable!()
            };
            let call = b.ins().call(
                print_functions.owned_strings[StringOp::Equals as usize],
                &[left_ptr, right_ptr],
            );
            let result = b.func.dfg.inst_results(call)[0];
            drop_temporary(b, print_functions, left);
            drop_temporary(b, print_functions, right);
            Ok(result)
        }
        (ty, CompiledValue::Integer(left, left_ty), CompiledValue::Integer(right, right_ty))
            if ty == left_ty && ty == right_ty =>
        {
            Ok(b.ins().icmp(IntCC::Equal, left, right))
        }
        (Type::F32, CompiledValue::F32(left), CompiledValue::F32(right)) => {
            Ok(b.ins().fcmp(FloatCC::Equal, left, right))
        }
        (Type::F64, CompiledValue::F64(left), CompiledValue::F64(right)) => {
            Ok(b.ins().fcmp(FloatCC::Equal, left, right))
        }
        (Type::Bool, CompiledValue::Bool(left), CompiledValue::Bool(right)) => {
            Ok(b.ins().icmp(IntCC::Equal, left, right))
        }
        (
            Type::Str,
            CompiledValue::Str {
                ptr: left_ptr,
                len: left_len,
            },
            CompiledValue::Str {
                ptr: right_ptr,
                len: right_len,
            },
        ) => {
            let call = b.ins().call(
                print_functions.string_equals,
                &[left_ptr, left_len, right_ptr, right_len],
            );
            Ok(b.func.dfg.inst_results(call)[0])
        }
        (
            Type::Struct(struct_id),
            left @ CompiledValue::Struct { .. },
            right @ CompiledValue::Struct { .. },
        ) => emit_struct_equality(b, print_functions, structs, struct_id, left, right),
        _ => Err("internal error: structure field equality value type mismatch".into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_print_struct(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    structs: &[RynStruct],
    enums: &[RynEnum],
    struct_id: usize,
    fields: &[CompiledValue],
    seal_state: &mut BlockSealState,
) -> Result<(), String> {
    let definition = structs
        .get(struct_id)
        .ok_or_else(|| "internal error: printed structure type is out of range".to_string())?;
    if fields.len() != definition.fields.len() {
        return Err("internal error: printed structure has the wrong field count".into());
    }

    emit_print_text(b, module, print_functions, strings, &definition.name)?;
    emit_print_text(b, module, print_functions, strings, " { ")?;
    for (index, (field, value)) in definition.fields.iter().zip(fields).enumerate() {
        if index != 0 {
            emit_print_text(b, module, print_functions, strings, ", ")?;
        }
        emit_print_text(b, module, print_functions, strings, &field.name)?;
        emit_print_text(b, module, print_functions, strings, ": ")?;
        emit_print_value(
            b,
            module,
            print_functions,
            strings,
            structs,
            enums,
            field.ty,
            value.clone(),
            seal_state,
        )?;
    }
    emit_print_text(b, module, print_functions, strings, " }")
}

#[allow(clippy::too_many_arguments)]
fn emit_print_enum(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    structs: &[RynStruct],
    enums: &[RynEnum],
    enum_id: usize,
    ptr: Value,
    seal_state: &mut BlockSealState,
) -> Result<(), String> {
    let definition = enums
        .get(enum_id)
        .ok_or_else(|| "internal error: printed enum type is out of range".to_string())?;
    let pointer_type = module.target_config().pointer_type();
    let tag_call = b.ins().call(print_functions.enum_tag, &[ptr]);
    let tag = b.func.dfg.inst_results(tag_call)[0];

    let variant_blocks = (0..definition.variants.len())
        .map(|_| b.create_block())
        .collect::<Vec<_>>();
    let merge_block = b.create_block();
    let mut test_block = b
        .current_block()
        .ok_or_else(|| "internal error: enum print has no source block".to_string())?;
    for (index, block) in variant_blocks.iter().enumerate() {
        if index > 0 {
            b.switch_to_block(test_block);
        }
        if index + 1 == variant_blocks.len() {
            b.ins().jump(*block, &[]);
            seal_ready(b, test_block, seal_state);
        } else {
            let next_test = b.create_block();
            let expected = b.ins().iconst(types::I64, index as i64);
            let matches = b.ins().icmp(IntCC::Equal, tag, expected);
            b.ins().brif(matches, *block, &[], next_test, &[]);
            seal_ready(b, test_block, seal_state);
            test_block = next_test;
        }
    }

    for (index, variant) in definition.variants.iter().enumerate() {
        b.switch_to_block(variant_blocks[index]);
        emit_print_text(b, module, print_functions, strings, &variant.name)?;
        if !variant.fields.is_empty() {
            emit_print_text(b, module, print_functions, strings, "(")?;
            let mut word_offset = 0usize;
            for (field_index, field_ty) in variant.fields.iter().enumerate() {
                if field_index != 0 {
                    emit_print_text(b, module, print_functions, strings, ", ")?;
                }
                let value = decode_enum_value_payload(
                    b,
                    ptr,
                    word_offset,
                    *field_ty,
                    pointer_type,
                    structs,
                    print_functions,
                    false,
                )?;
                emit_print_value(
                    b,
                    module,
                    print_functions,
                    strings,
                    structs,
                    enums,
                    *field_ty,
                    value,
                    seal_state,
                )?;
                word_offset += value_width(*field_ty, structs);
            }
            emit_print_text(b, module, print_functions, strings, ")")?;
        }
        b.ins().jump(merge_block, &[]);
        seal_ready(b, variant_blocks[index], seal_state);
    }
    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    Ok(())
}

fn emit_print_text(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    text: &str,
) -> Result<(), String> {
    let id = strings
        .get(text)
        .ok_or_else(|| "internal error: missing static structure-print text".to_string())?;
    let global = module.declare_data_in_func(*id, b.func);
    let ptr = b
        .ins()
        .symbol_value(module.target_config().pointer_type(), global);
    let len = b.ins().iconst(types::I64, text.len() as i64);
    b.ins().call(print_functions.string, &[ptr, len]);
    Ok(())
}

fn store_reference_value(
    b: &mut FunctionBuilder<'_>,
    ty: Type,
    value: CompiledValue,
    pointer: Value,
    offset: i32,
    structs: &[RynStruct],
    c_layout: bool,
) -> Result<(), String> {
    match ty {
        Type::Struct(struct_id) => {
            let CompiledValue::Struct {
                struct_id: actual,
                fields,
            } = value
            else {
                return Err("internal error: reference field value is not a structure".into());
            };
            if actual != struct_id || fields.len() != structs[struct_id].fields.len() {
                return Err(
                    "internal error: reference field structure layout does not match".into(),
                );
            }
            let mut cursor = 0usize;
            for (field, field_value) in structs[struct_id].fields.iter().zip(fields) {
                let field_offset = if c_layout {
                    let (field_size, field_align) = crate::sema::type_layout(field.ty, structs);
                    cursor = align_up(cursor, field_align);
                    let at = cursor as i32;
                    cursor = cursor.saturating_add(field_size);
                    at
                } else {
                    (field.slot_offset * 8) as i32
                };
                store_reference_value(
                    b,
                    field.ty,
                    field_value,
                    pointer,
                    offset + field_offset,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        Type::Array(array_id) => {
            let CompiledValue::Array(values) = value else {
                return Err("internal error: reference field value is not an array".into());
            };
            let (element, length) = array_info(array_id);
            if values.len() != length {
                return Err("internal error: reference field array has the wrong length".into());
            }
            let stride = if c_layout {
                let (element_size, element_align) = crate::sema::type_layout(element, structs);
                align_up(element_size, element_align)
            } else {
                storage_slot_width(element, structs) * 8
            };
            for (index, element_value) in values.into_iter().enumerate() {
                store_reference_value(
                    b,
                    element,
                    element_value,
                    pointer,
                    offset + (index * stride) as i32,
                    structs,
                    c_layout,
                )?;
            }
            Ok(())
        }
        _ => {
            let values = flatten_value(value);
            for (index, word) in values.iter().enumerate() {
                b.ins().store(
                    MemFlagsData::new(),
                    *word,
                    pointer,
                    offset + (index * 8) as i32,
                );
            }
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_reference_field(
    b: &mut FunctionBuilder<'_>,
    ty: Type,
    pointer: Value,
    offset: i32,
    c_layout: bool,
    pointer_type: types::Type,
    structs: &[RynStruct],
    runtime: PrintFunctions,
    borrow: bool,
) -> Result<CompiledValue, String> {
    match ty {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Char => {
            let clif_ty = clif_scalar_type(ty, pointer_type)?;
            Ok(CompiledValue::Integer(
                b.ins().load(clif_ty, MemFlagsData::new(), pointer, offset),
                ty,
            ))
        }
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            Ok(CompiledValue::Integer(
                b.ins()
                    .load(pointer_type, MemFlagsData::new(), pointer, offset),
                ty,
            ))
        }
        Type::F32 => Ok(CompiledValue::F32(b.ins().load(
            types::F32,
            MemFlagsData::new(),
            pointer,
            offset,
        ))),
        Type::F64 => Ok(CompiledValue::F64(b.ins().load(
            types::F64,
            MemFlagsData::new(),
            pointer,
            offset,
        ))),
        Type::Bool => Ok(CompiledValue::Bool(b.ins().load(
            types::I8,
            MemFlagsData::new(),
            pointer,
            offset,
        ))),
        Type::OwnedString => {
            let handle = b
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, offset);
            if borrow {
                return Ok(CompiledValue::OwnedString {
                    ptr: handle,
                    temporary: false,
                });
            }
            let clone = b
                .ins()
                .call(runtime.owned_strings[StringOp::Clone as usize], &[handle]);
            Ok(CompiledValue::OwnedString {
                ptr: b.func.dfg.inst_results(clone)[0],
                temporary: true,
            })
        }
        Type::Vec(_) => {
            let handle = b
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, offset);
            if borrow {
                return Ok(CompiledValue::Vec {
                    ptr: handle,
                    temporary: false,
                });
            }
            let clone = b
                .ins()
                .call(runtime.owned_vecs[VecOp::Clone as usize], &[handle]);
            Ok(CompiledValue::Vec {
                ptr: b.func.dfg.inst_results(clone)[0],
                temporary: true,
            })
        }
        Type::Map(_) | Type::Set(_) => {
            let handle = b
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, offset);
            if borrow {
                return Ok(CompiledValue::Map {
                    ptr: handle,
                    temporary: false,
                });
            }
            let clone = b.ins().call(runtime.maps[MapOp::Clone as usize], &[handle]);
            Ok(CompiledValue::Map {
                ptr: b.func.dfg.inst_results(clone)[0],
                temporary: true,
            })
        }
        Type::Enum(_) => {
            let handle = b
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, offset);
            if borrow {
                return Ok(CompiledValue::Enum {
                    ptr: handle,
                    temporary: false,
                });
            }
            let clone = b.ins().call(runtime.enum_clone, &[handle]);
            Ok(CompiledValue::Enum {
                ptr: b.func.dfg.inst_results(clone)[0],
                temporary: true,
            })
        }
        Type::Struct(struct_id) => {
            let mut fields = Vec::new();
            let mut cursor = 0usize;
            for field in &structs[struct_id].fields {
                let field_offset = if c_layout {
                    let (field_size, field_align) = crate::sema::type_layout(field.ty, structs);
                    cursor = align_up(cursor, field_align);
                    let at = cursor as i32;
                    cursor = cursor.saturating_add(field_size);
                    at
                } else {
                    (field.slot_offset * 8) as i32
                };
                fields.push(load_reference_field(
                    b,
                    field.ty,
                    pointer,
                    offset + field_offset,
                    c_layout,
                    pointer_type,
                    structs,
                    runtime,
                    borrow,
                )?);
            }
            Ok(CompiledValue::Struct { struct_id, fields })
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let stride = if c_layout {
                let (size, align) = crate::sema::type_layout(element, structs);
                align_up(size, align)
            } else {
                storage_slot_width(element, structs) * 8
            };
            let mut values = Vec::with_capacity(length);
            for index in 0..length {
                values.push(load_reference_field(
                    b,
                    element,
                    pointer,
                    offset + (index * stride) as i32,
                    c_layout,
                    pointer_type,
                    structs,
                    runtime,
                    borrow,
                )?);
            }
            Ok(CompiledValue::Array(values))
        }
        _ => Err("internal error: unsupported reference field type".into()),
    }
}

fn store_local(
    b: &mut FunctionBuilder<'_>,
    slot: usize,
    ty: Type,
    value: CompiledValue,
    structs: &[RynStruct],
) -> Result<(), String> {
    match (ty, value) {
        (ty, CompiledValue::Integer(v, actual)) if ty == actual => {
            b.def_var(Variable::from_u32(slot as u32), v)
        }
        (Type::F32, CompiledValue::F32(v))
        | (Type::F64, CompiledValue::F64(v))
        | (Type::Bool, CompiledValue::Bool(v)) => b.def_var(Variable::from_u32(slot as u32), v),
        (Type::Reference(_, _) | Type::RawPointer(_), CompiledValue::Integer(v, _)) => {
            b.def_var(Variable::from_u32(slot as u32), v)
        }
        (Type::Str, CompiledValue::Str { ptr, len }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr);
            b.def_var(Variable::from_u32(slot as u32 + 1), len);
        }
        (Type::Slice(_), CompiledValue::Slice { ptr, len }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr);
            b.def_var(Variable::from_u32(slot as u32 + 1), len);
        }
        (Type::OwnedString, CompiledValue::OwnedString { ptr, .. }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr)
        }
        (Type::Vec(_), CompiledValue::Vec { ptr, .. }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr)
        }
        (Type::Map(_) | Type::Set(_), CompiledValue::Map { ptr, .. }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr)
        }
        (Type::Enum(_), CompiledValue::Enum { ptr, .. }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr)
        }
        (Type::Array(id), CompiledValue::Array(values)) => {
            let (element, length) = array_info(id);
            if values.len() != length {
                return Err("internal error: array value has the wrong element count".into());
            }
            for (index, value) in values.into_iter().enumerate() {
                store_local(
                    b,
                    slot + index * storage_slot_width(element, structs),
                    element,
                    value,
                    structs,
                )?;
            }
        }
        (
            Type::Struct(struct_id),
            CompiledValue::Struct {
                struct_id: actual,
                fields,
            },
        ) if struct_id == actual => {
            let definition = &structs[struct_id];
            if fields.len() != definition.fields.len() {
                return Err("internal error: structure value has the wrong field count".into());
            }
            for (field, value) in definition.fields.iter().zip(fields) {
                store_local(b, slot + field.slot_offset, field.ty, value, structs)?;
            }
        }
        _ => return Err("internal error: local value differs from checked type".into()),
    }
    Ok(())
}

fn load_local(
    b: &mut FunctionBuilder<'_>,
    slot: usize,
    ty: Type,
    structs: &[RynStruct],
) -> Result<CompiledValue, String> {
    let first = b.use_var(Variable::from_u32(slot as u32));
    Ok(match ty {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Char => CompiledValue::Integer(first, ty),
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            CompiledValue::Integer(first, ty)
        }
        Type::F32 => CompiledValue::F32(first),
        Type::F64 => CompiledValue::F64(first),
        Type::Bool => CompiledValue::Bool(first),
        Type::Str => CompiledValue::Str {
            ptr: first,
            len: b.use_var(Variable::from_u32(slot as u32 + 1)),
        },
        Type::Slice(_) => CompiledValue::Slice {
            ptr: first,
            len: b.use_var(Variable::from_u32(slot as u32 + 1)),
        },
        Type::OwnedString => CompiledValue::OwnedString {
            ptr: first,
            temporary: false,
        },
        Type::Vec(_) => CompiledValue::Vec {
            ptr: first,
            temporary: false,
        },
        Type::Map(_) | Type::Set(_) => CompiledValue::Map {
            ptr: first,
            temporary: false,
        },
        Type::Enum(_) => CompiledValue::Enum {
            ptr: first,
            temporary: false,
        },
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                fields.push(load_local(b, slot + field.slot_offset, field.ty, structs)?);
            }
            CompiledValue::Struct { struct_id, fields }
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let mut values = Vec::with_capacity(length);
            for index in 0..length {
                values.push(load_local(
                    b,
                    slot + index * storage_slot_width(element, structs),
                    element,
                    structs,
                )?);
            }
            CompiledValue::Array(values)
        }
    })
}

#[derive(Default)]
struct BlockSealState {
    sealed: Vec<cranelift_codegen::ir::Block>,
    deferred: Vec<cranelift_codegen::ir::Block>,
}

fn seal_ready(
    b: &mut FunctionBuilder<'_>,
    block: cranelift_codegen::ir::Block,
    state: &mut BlockSealState,
) {
    if !state.deferred.contains(&block) && !state.sealed.contains(&block) {
        b.seal_block(block);
        state.sealed.push(block);
    }
}

fn collect_strings(ir: &RynIr) -> BTreeSet<String> {
    fn print_type(ty: Type, structs: &[RynStruct], enums: &[RynEnum], out: &mut BTreeSet<String>) {
        match ty {
            Type::Struct(struct_id) => {
                let definition = &structs[struct_id];
                out.insert(definition.name.clone());
                out.insert(" { ".into());
                out.insert(", ".into());
                out.insert(": ".into());
                out.insert(" }".into());
                for field in &definition.fields {
                    out.insert(field.name.clone());
                    print_type(field.ty, structs, enums, out);
                }
            }
            Type::Enum(enum_id) => {
                let definition = &enums[enum_id];
                out.insert("(".into());
                out.insert(", ".into());
                out.insert(")".into());
                for variant in &definition.variants {
                    out.insert(variant.name.clone());
                }
            }
            _ => {}
        }
    }

    fn expr(value: &IrExpression, out: &mut BTreeSet<String>) {
        match value {
            IrExpression::String(s) => {
                out.insert(s.clone());
            }
            IrExpression::Call { arguments, .. } => {
                for arg in arguments {
                    expr(arg, out);
                }
            }
            IrExpression::ArrayValue(values) => {
                for value in values {
                    expr(value, out);
                }
            }
            IrExpression::ArrayRepeat { value, .. } => expr(value, out),
            IrExpression::ArrayAsSlice { array, .. } => expr(array, out),
            IrExpression::ArrayIndex { array, index, .. } => {
                expr(array, out);
                expr(index, out);
            }
            IrExpression::SliceIndex { slice, index, .. } => {
                expr(slice, out);
                expr(index, out);
            }
            IrExpression::SliceElementAddress { slice, index, .. } => {
                expr(slice, out);
                expr(index, out);
            }
            IrExpression::Dereference { pointer, .. } => expr(pointer, out),
            IrExpression::ReferenceField { pointer, .. } => expr(pointer, out),
            IrExpression::ValueAddress { value, .. } => expr(value, out),
            IrExpression::StringFindOption { value, .. } | IrExpression::StringAsStr(value) => {
                expr(value, out)
            }
            IrExpression::If {
                condition,
                then_value,
                else_value,
                ..
            } => {
                expr(condition, out);
                expr(then_value, out);
                expr(else_value, out);
            }
            IrExpression::Binary { left, right, .. } => {
                expr(left, out);
                expr(right, out);
            }
            IrExpression::Negate(inner, _)
            | IrExpression::Not(inner)
            | IrExpression::BitNot(inner, _) => expr(inner, out),
            IrExpression::Cast { value, .. } => expr(value, out),
            IrExpression::StructValue { fields, .. } => {
                for (_, field) in fields {
                    expr(field, out);
                }
            }
            IrExpression::Field { value, .. } => expr(value, out),
            IrExpression::Integer(..)
            | IrExpression::Character(_)
            | IrExpression::Move { .. }
            | IrExpression::Float(..)
            | IrExpression::Boolean(_)
            | IrExpression::Local { .. }
            | IrExpression::AddressOf { .. }
            | IrExpression::FunctionAddress { .. } => {}
            IrExpression::EnumMatch { value, arms, .. } => {
                expr(value, out);
                for arm in arms {
                    expr(&arm.body, out);
                }
            }
            IrExpression::Propagate {
                value, deferred, ..
            } => {
                expr(value, out);
                stmts(deferred, &[], &[], out);
            }
        }
    }
    fn stmts(
        values: &[IrStatement],
        structs: &[RynStruct],
        enums: &[RynEnum],
        out: &mut BTreeSet<String>,
    ) {
        for stmt in values {
            match stmt {
                IrStatement::Block(statements) => stmts(statements, structs, enums, out),
                IrStatement::Let { value, .. }
                | IrStatement::Assign { value, .. }
                | IrStatement::FieldAssign { value, .. } => expr(value, out),
                IrStatement::ArrayAssign { index, value, .. } => {
                    expr(index, out);
                    expr(value, out);
                }
                IrStatement::DereferenceAssign { pointer, value, .. } => {
                    expr(pointer, out);
                    expr(value, out);
                }
                IrStatement::ReferenceFieldAssign { pointer, value, .. } => {
                    expr(pointer, out);
                    expr(value, out);
                }
                IrStatement::Print { value, ty } => {
                    expr(value, out);
                    print_type(*ty, structs, enums, out);
                }
                IrStatement::PrintTemplate(parts) => {
                    for part in parts {
                        match part {
                            IrPrintPart::Text(text) => {
                                out.insert(text.clone());
                            }
                            IrPrintPart::Value { value, ty } => {
                                expr(value, out);
                                print_type(*ty, structs, enums, out);
                            }
                        }
                    }
                }
                IrStatement::Call { arguments, .. } => {
                    for argument in arguments {
                        expr(argument, out);
                    }
                }
                IrStatement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    expr(condition, out);
                    stmts(then_body, structs, enums, out);
                    stmts(else_body, structs, enums, out);
                }
                IrStatement::While {
                    setup,
                    condition,
                    body,
                } => {
                    stmts(setup, structs, enums, out);
                    expr(condition, out);
                    stmts(body, structs, enums, out);
                }
                IrStatement::For {
                    start, end, body, ..
                } => {
                    expr(start, out);
                    expr(end, out);
                    stmts(body, structs, enums, out);
                }
                IrStatement::Return { value } => {
                    if let Some(value) = value {
                        expr(value, out);
                    }
                }
                IrStatement::Break | IrStatement::Continue | IrStatement::Drop { .. } => {}
            }
        }
    }
    let mut strings = BTreeSet::new();
    for function in &ir.functions {
        stmts(&function.statements, &ir.structs, &ir.enums, &mut strings);
        if let Some(ret) = &function.return_value {
            expr(ret, &mut strings);
        }
    }
    strings
}

#[derive(Clone)]
enum CompiledValue {
    Integer(Value, Type),
    F32(Value),
    F64(Value),
    Bool(Value),
    OwnedString {
        ptr: Value,
        temporary: bool,
    },
    Vec {
        ptr: Value,
        temporary: bool,
    },
    Map {
        ptr: Value,
        temporary: bool,
    },
    Enum {
        ptr: Value,
        temporary: bool,
    },
    Str {
        ptr: Value,
        len: Value,
    },
    StrViewOwned {
        ptr: Value,
        len: Value,
        owner: Value,
        temporary: bool,
    },
    Struct {
        struct_id: usize,
        fields: Vec<CompiledValue>,
    },
    Array(Vec<CompiledValue>),
    Slice {
        ptr: Value,
        len: Value,
    },
}

fn flatten_value(value: CompiledValue) -> Vec<Value> {
    match value {
        CompiledValue::Integer(v, _)
        | CompiledValue::F32(v)
        | CompiledValue::F64(v)
        | CompiledValue::Bool(v)
        | CompiledValue::OwnedString { ptr: v, .. }
        | CompiledValue::Vec { ptr: v, .. }
        | CompiledValue::Map { ptr: v, .. }
        | CompiledValue::Enum { ptr: v, .. } => vec![v],
        CompiledValue::Str { ptr, len } => vec![ptr, len],
        CompiledValue::StrViewOwned { ptr, len, .. } => vec![ptr, len],
        CompiledValue::Slice { ptr, len } => vec![ptr, len],
        CompiledValue::Struct { fields, .. } => {
            fields.into_iter().flat_map(flatten_value).collect()
        }
        CompiledValue::Array(values) => values.into_iter().flat_map(flatten_value).collect(),
    }
}

fn zero_clif_value(b: &mut FunctionBuilder<'_>, ty: types::Type) -> Value {
    if ty == types::F32 {
        b.ins().f32const(0.0)
    } else if ty == types::F64 {
        b.ins().f64const(0.0)
    } else {
        b.ins().iconst(ty, 0)
    }
}

fn clone_array_element(
    b: &mut FunctionBuilder<'_>,
    runtime: PrintFunctions,
    value: CompiledValue,
) -> Result<CompiledValue, String> {
    Ok(match value {
        CompiledValue::OwnedString { ptr, .. } => {
            let call = b
                .ins()
                .call(runtime.owned_strings[StringOp::Clone as usize], &[ptr]);
            CompiledValue::OwnedString {
                ptr: b.func.dfg.inst_results(call)[0],
                temporary: true,
            }
        }
        CompiledValue::Vec { ptr, .. } => {
            let call = b
                .ins()
                .call(runtime.owned_vecs[VecOp::Clone as usize], &[ptr]);
            CompiledValue::Vec {
                ptr: b.func.dfg.inst_results(call)[0],
                temporary: true,
            }
        }
        CompiledValue::Map { ptr, .. } => {
            let call = b.ins().call(runtime.maps[MapOp::Clone as usize], &[ptr]);
            CompiledValue::Map {
                ptr: b.func.dfg.inst_results(call)[0],
                temporary: true,
            }
        }
        CompiledValue::Enum { ptr, .. } => {
            let call = b.ins().call(runtime.enum_clone, &[ptr]);
            CompiledValue::Enum {
                ptr: b.func.dfg.inst_results(call)[0],
                temporary: true,
            }
        }
        CompiledValue::Struct { struct_id, fields } => CompiledValue::Struct {
            struct_id,
            fields: fields
                .into_iter()
                .map(|field| clone_array_element(b, runtime, field))
                .collect::<Result<Vec<_>, _>>()?,
        },
        CompiledValue::Array(values) => CompiledValue::Array(
            values
                .into_iter()
                .map(|element| clone_array_element(b, runtime, element))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        other => other,
    })
}

fn emit_expr(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    expr: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<CompiledValue, String> {
    Ok(match expr {
        IrExpression::Character(character) => {
            CompiledValue::Integer(b.ins().iconst(types::I32, *character as i64), Type::Char)
        }
        IrExpression::Move { slot, ty } => {
            let mut value = load_local(b, *slot, *ty, env.structs)?;
            make_temporary(&mut value);
            for (owner, _) in crate::guard::owned_slots(*ty, *slot, env.structs) {
                let pointer = b.use_var(Variable::from_u32(owner as u32));
                let pointer_type = b.func.dfg.value_type(pointer);
                let null = b.ins().iconst(pointer_type, 0);
                b.def_var(Variable::from_u32(owner as u32), null);
            }
            value
        }
        IrExpression::Integer(v, ty) => {
            let clif_ty = clif_integer_type(*ty)
                .ok_or_else(|| "internal error: integer IR has a non-integer type".to_string())?;
            CompiledValue::Integer(b.ins().iconst(clif_ty, *v as i64), *ty)
        }
        IrExpression::Float(v, Type::F32) => CompiledValue::F32(b.ins().f32const(*v as f32)),
        IrExpression::Float(v, Type::F64) => CompiledValue::F64(b.ins().f64const(*v)),
        IrExpression::Float(_, _) => {
            return Err("internal error: float IR has a non-float type".into());
        }
        IrExpression::Boolean(v) => CompiledValue::Bool(b.ins().iconst(types::I8, i64::from(*v))),
        IrExpression::AddressOf {
            slot, pointer_type, ..
        } => {
            let Some(Some(address_slot)) = env.address_slots.get(*slot) else {
                return Err("internal error: reference target has no stack storage".into());
            };
            let pointer =
                b.ins()
                    .stack_addr(module.target_config().pointer_type(), *address_slot, 0);
            CompiledValue::Integer(pointer, *pointer_type)
        }
        IrExpression::Dereference { pointer, ty } => {
            let pointer = emit_expr(b, module, pointer, env, seal_state)?;
            let CompiledValue::Integer(pointer, _) = pointer else {
                return Err("internal error: dereference operand is not a pointer".into());
            };
            if matches!(ty, Type::Struct(_) | Type::Array(_)) {
                let c_layout = matches!(ty, Type::Struct(id) if env.structs[*id].repr_c);
                return load_reference_field(
                    b,
                    *ty,
                    pointer,
                    0,
                    c_layout,
                    module.target_config().pointer_type(),
                    env.structs,
                    env.print_functions,
                    true,
                );
            }
            // Owning handles load as borrowed views; the reference's home keeps
            // ownership, so the view must never be stored or consumed.
            if matches!(
                ty,
                Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_)
            ) {
                let handle = b.ins().load(
                    module.target_config().pointer_type(),
                    MemFlagsData::new(),
                    pointer,
                    0,
                );
                return Ok(match ty {
                    Type::OwnedString => CompiledValue::OwnedString {
                        ptr: handle,
                        temporary: false,
                    },
                    Type::Vec(_) => CompiledValue::Vec {
                        ptr: handle,
                        temporary: false,
                    },
                    Type::Map(_) | Type::Set(_) => CompiledValue::Map {
                        ptr: handle,
                        temporary: false,
                    },
                    _ => CompiledValue::Enum {
                        ptr: handle,
                        temporary: false,
                    },
                });
            }
            let value = b.ins().load(
                clif_scalar_type(*ty, module.target_config().pointer_type())?,
                MemFlagsData::new(),
                pointer,
                0,
            );
            match ty {
                Type::F32 => CompiledValue::F32(value),
                Type::F64 => CompiledValue::F64(value),
                Type::Bool => CompiledValue::Bool(value),
                _ => CompiledValue::Integer(value, *ty),
            }
        }
        IrExpression::String(value) => {
            let id = *env
                .strings
                .get(value)
                .ok_or_else(|| "internal error: missing collected string".to_string())?;
            let global = module.declare_data_in_func(id, b.func);
            let ptr = b
                .ins()
                .symbol_value(module.target_config().pointer_type(), global);
            let len = b.ins().iconst(types::I64, value.len() as i64);
            CompiledValue::Str { ptr, len }
        }
        IrExpression::Local { slot, ty, .. }
            if matches!(
                ty,
                Type::I8
                    | Type::I16
                    | Type::I32
                    | Type::I64
                    | Type::U8
                    | Type::U16
                    | Type::U32
                    | Type::U64
                    | Type::Char
                    | Type::Reference(_, _)
                    | Type::RawPointer(_)
                    | Type::FunctionPointer(_)
            ) =>
        {
            CompiledValue::Integer(
                load_addressed_local(
                    b,
                    *slot,
                    *ty,
                    env.address_slots,
                    module.target_config().pointer_type(),
                    env.addressed_slot_types,
                    env.structs,
                )?
                .unwrap_or_else(|| b.use_var(Variable::from_u32(*slot as u32))),
                *ty,
            )
        }
        IrExpression::Local {
            slot,
            ty: Type::F32,
            ..
        } => CompiledValue::F32(
            load_addressed_local(
                b,
                *slot,
                Type::F32,
                env.address_slots,
                module.target_config().pointer_type(),
                env.addressed_slot_types,
                env.structs,
            )?
            .unwrap_or_else(|| b.use_var(Variable::from_u32(*slot as u32))),
        ),
        IrExpression::Local {
            slot,
            ty: Type::F64,
            ..
        } => CompiledValue::F64(
            load_addressed_local(
                b,
                *slot,
                Type::F64,
                env.address_slots,
                module.target_config().pointer_type(),
                env.addressed_slot_types,
                env.structs,
            )?
            .unwrap_or_else(|| b.use_var(Variable::from_u32(*slot as u32))),
        ),
        IrExpression::Local {
            slot,
            ty: Type::Bool,
            ..
        } => CompiledValue::Bool(
            load_addressed_local(
                b,
                *slot,
                Type::Bool,
                env.address_slots,
                module.target_config().pointer_type(),
                env.addressed_slot_types,
                env.structs,
            )?
            .unwrap_or_else(|| b.use_var(Variable::from_u32(*slot as u32))),
        ),
        IrExpression::Local { slot, ty, .. }
            if matches!(ty, Type::Reference(_, _) | Type::RawPointer(_)) =>
        {
            CompiledValue::Integer(b.use_var(Variable::from_u32(*slot as u32)), *ty)
        }
        IrExpression::Local {
            slot,
            ty: Type::Str,
            ..
        } => CompiledValue::Str {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            len: b.use_var(Variable::from_u32(*slot as u32 + 1)),
        },
        IrExpression::Local {
            slot,
            ty: Type::Struct(struct_id),
            ..
        } => {
            if let Some(Some(home)) = env.address_slots.get(*slot) {
                let pointer = b
                    .ins()
                    .stack_addr(module.target_config().pointer_type(), *home, 0);
                return load_reference_field(
                    b,
                    Type::Struct(*struct_id),
                    pointer,
                    0,
                    env.structs[*struct_id].repr_c,
                    module.target_config().pointer_type(),
                    env.structs,
                    env.print_functions,
                    true,
                );
            }
            let mut fields = Vec::new();
            for field in &env.structs[*struct_id].fields {
                let field_slot = *slot + field.slot_offset;
                fields.push(load_local(b, field_slot, field.ty, env.structs)?);
            }
            CompiledValue::Struct {
                struct_id: *struct_id,
                fields,
            }
        }
        IrExpression::Local {
            slot,
            ty: Type::OwnedString,
            ..
        } => CompiledValue::OwnedString {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            temporary: false,
        },
        IrExpression::Local {
            slot,
            ty: Type::Vec(_),
            ..
        } => CompiledValue::Vec {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            temporary: false,
        },
        IrExpression::Local {
            slot,
            ty: Type::Map(_),
            ..
        } => CompiledValue::Map {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            temporary: false,
        },
        IrExpression::Local {
            slot,
            ty: Type::Set(_),
            ..
        } => CompiledValue::Map {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            temporary: false,
        },
        IrExpression::Local {
            slot,
            ty: Type::Enum(_),
            ..
        } => CompiledValue::Enum {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            temporary: false,
        },
        IrExpression::Local {
            slot,
            ty: slice_ty @ Type::Slice(_),
            ..
        } => load_local(b, *slot, *slice_ty, env.structs)?,
        IrExpression::Local {
            slot,
            ty: array_ty @ Type::Array(_),
            ..
        } => load_local(b, *slot, *array_ty, env.structs)?,
        IrExpression::ArrayValue(values) => {
            let mut compiled = Vec::with_capacity(values.len());
            for value in values {
                compiled.push(emit_expr(b, module, value, env, seal_state)?);
            }
            CompiledValue::Array(compiled)
        }
        IrExpression::ArrayRepeat { value, length } => {
            let element = emit_expr(b, module, value, env, seal_state)?;
            let mut compiled = Vec::with_capacity(*length);
            for _ in 0..*length {
                compiled.push(element.clone());
            }
            CompiledValue::Array(compiled)
        }
        IrExpression::ArrayAsSlice {
            array,
            slice_id,
            length,
        } => {
            let array = emit_expr(b, module, array, env, seal_state)?;
            let CompiledValue::Array(values) = array else {
                return Err("internal error: array-to-slice source is not an array".into());
            };
            if values.len() != *length {
                return Err("internal error: array-to-slice source has the wrong length".into());
            }
            let element = vec_elem(*slice_id);
            let scalar_types =
                clif_types(element, module.target_config().pointer_type(), env.structs);
            let stride = storage_slot_width(element, env.structs);
            if scalar_types.len() != stride {
                return Err(
                    "internal error: array slice element has an invalid storage layout".into(),
                );
            }
            let slot_size = (*length).max(1) * stride * 8;
            let slot = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size as u32,
                3,
            ));
            for (index, value) in values.into_iter().enumerate() {
                let components = flatten_value(value);
                if components.len() != stride {
                    return Err(
                        "internal error: array slice element has an invalid value layout".into(),
                    );
                }
                for (component_index, component) in components.into_iter().enumerate() {
                    b.ins().stack_store(
                        module.target_config().pointer_type(),
                        component,
                        slot,
                        ((index * stride + component_index) * 8) as i32,
                    );
                }
            }
            let pointer = b
                .ins()
                .stack_addr(module.target_config().pointer_type(), slot, 0);
            CompiledValue::Slice {
                ptr: pointer,
                len: b.ins().iconst(types::I64, *length as i64),
            }
        }
        IrExpression::ArrayIndex {
            array,
            index,
            ty,
            length,
        } => {
            let array = emit_expr(b, module, array, env, seal_state)?;
            let CompiledValue::Array(values) = array else {
                return Err("internal error: array index base is not an array".into());
            };
            let temporary_elements = values.clone();
            let index_value = emit_expr(b, module, index, env, seal_state)?;
            let CompiledValue::Integer(index_value, index_ty) = index_value else {
                return Err("internal error: checked array index is not an integer".into());
            };
            let index64 = match index_ty {
                Type::I8 | Type::I16 | Type::I32 => b.ins().sextend(types::I64, index_value),
                Type::U8 | Type::U16 | Type::U32 => b.ins().uextend(types::I64, index_value),
                Type::I64 | Type::U64 => index_value,
                _ => return Err("internal error: invalid array index type".into()),
            };
            let too_large =
                b.ins()
                    .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, index64, *length as i64);
            let out_of_bounds = if matches!(index_ty, Type::I8 | Type::I16 | Type::I32 | Type::I64)
            {
                let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index64, 0);
                b.ins().bor(negative, too_large)
            } else {
                too_large
            };
            let error_block = b.create_block();
            let valid_block = b.create_block();
            let from = b
                .current_block()
                .ok_or("internal error: array index has no source block")?;
            b.ins()
                .brif(out_of_bounds, error_block, &[], valid_block, &[]);
            seal_ready(b, from, seal_state);
            b.switch_to_block(error_block);
            b.ins()
                .call(env.print_functions.array_index_out_of_bounds, &[]);
            b.ins().trap(TrapCode::unwrap_user(1));
            seal_ready(b, error_block, seal_state);
            b.switch_to_block(valid_block);
            let components = values.into_iter().map(flatten_value).collect::<Vec<_>>();
            let component_types =
                clif_types(*ty, module.target_config().pointer_type(), env.structs);
            if components
                .iter()
                .any(|value| value.len() != component_types.len())
            {
                return Err("internal error: array element has an incompatible layout".into());
            }
            let mut selected = Vec::with_capacity(component_types.len());
            for (component_index, component_type) in component_types.iter().enumerate() {
                let mut value = if let Some(first) = components.first() {
                    first[component_index]
                } else {
                    zero_clif_value(b, *component_type)
                };
                for (element_index, element) in components.iter().enumerate().skip(1) {
                    let is_element =
                        b.ins()
                            .icmp_imm_u(IntCC::Equal, index64, element_index as i64);
                    value = b.ins().select(is_element, element[component_index], value);
                }
                selected.push(value);
            }
            let mut offset = 0;
            let selected = compiled_value_from_type(*ty, &selected, &mut offset, env.structs)?;
            let result = clone_array_element(b, env.print_functions, selected)?;
            for value in temporary_elements {
                drop_temporary(b, env.print_functions, value);
            }
            seal_ready(b, valid_block, seal_state);
            result
        }
        IrExpression::SliceIndex { slice, index, ty } => {
            let CompiledValue::Slice { ptr, len } = emit_expr(b, module, slice, env, seal_state)?
            else {
                return Err("internal error: slice index base is not a slice".into());
            };
            let CompiledValue::Integer(index, index_ty) =
                emit_expr(b, module, index, env, seal_state)?
            else {
                return Err("internal error: checked slice index is not an integer".into());
            };
            let index64 = match index_ty {
                Type::I8 | Type::I16 | Type::I32 => b.ins().sextend(types::I64, index),
                Type::U8 | Type::U16 | Type::U32 => b.ins().uextend(types::I64, index),
                Type::I64 | Type::U64 => index,
                _ => return Err("internal error: invalid slice index type".into()),
            };
            let too_large = b
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, index64, len);
            let out_of_bounds = if matches!(index_ty, Type::I8 | Type::I16 | Type::I32 | Type::I64)
            {
                let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index64, 0);
                b.ins().bor(negative, too_large)
            } else {
                too_large
            };
            let error_block = b.create_block();
            let valid_block = b.create_block();
            let from = b
                .current_block()
                .ok_or("internal error: slice index has no source block")?;
            b.ins()
                .brif(out_of_bounds, error_block, &[], valid_block, &[]);
            seal_ready(b, from, seal_state);
            b.switch_to_block(error_block);
            b.ins()
                .call(env.print_functions.array_index_out_of_bounds, &[]);
            b.ins().trap(TrapCode::unwrap_user(1));
            seal_ready(b, error_block, seal_state);
            b.switch_to_block(valid_block);
            let pointer_type = module.target_config().pointer_type();
            let byte_offset = b
                .ins()
                .imul_imm_s(index64, (storage_slot_width(*ty, env.structs) * 8) as i64);
            let byte_offset = if pointer_type == types::I64 {
                byte_offset
            } else {
                b.ins().ireduce(pointer_type, byte_offset)
            };
            let address = b.ins().iadd(ptr, byte_offset);
            let component_types = clif_types(*ty, pointer_type, env.structs);
            let stride = storage_slot_width(*ty, env.structs);
            if component_types.len() != stride {
                return Err("internal error: slice element has an invalid storage layout".into());
            }
            let mut components = Vec::with_capacity(stride);
            for (component_index, component_type) in component_types.into_iter().enumerate() {
                let component_address = if component_index == 0 {
                    address
                } else {
                    b.ins().iadd_imm_s(address, (component_index * 8) as i64)
                };
                components.push(b.ins().load(
                    component_type,
                    MemFlagsData::new(),
                    component_address,
                    0,
                ));
            }
            let mut component_offset = 0;
            let selected =
                compiled_value_from_type(*ty, &components, &mut component_offset, env.structs)?;
            let result = clone_array_element(b, env.print_functions, selected)?;
            seal_ready(b, valid_block, seal_state);
            result
        }
        IrExpression::SliceElementAddress {
            slice, index, ty, ..
        } => {
            let CompiledValue::Slice { ptr, len } = emit_expr(b, module, slice, env, seal_state)?
            else {
                return Err("internal error: slice address base is not a slice".into());
            };
            let CompiledValue::Integer(index, index_ty) =
                emit_expr(b, module, index, env, seal_state)?
            else {
                return Err("internal error: checked slice index is not an integer".into());
            };
            let index64 = match index_ty {
                Type::I8 | Type::I16 | Type::I32 => b.ins().sextend(types::I64, index),
                Type::U8 | Type::U16 | Type::U32 => b.ins().uextend(types::I64, index),
                Type::I64 | Type::U64 => index,
                _ => return Err("internal error: invalid slice index type".into()),
            };
            let too_large = b
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, index64, len);
            let out_of_bounds = if matches!(index_ty, Type::I8 | Type::I16 | Type::I32 | Type::I64)
            {
                let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index64, 0);
                b.ins().bor(negative, too_large)
            } else {
                too_large
            };
            let error_block = b.create_block();
            let valid_block = b.create_block();
            let from = b
                .current_block()
                .ok_or("internal error: slice address has no source block")?;
            b.ins()
                .brif(out_of_bounds, error_block, &[], valid_block, &[]);
            seal_ready(b, from, seal_state);
            b.switch_to_block(error_block);
            b.ins()
                .call(env.print_functions.array_index_out_of_bounds, &[]);
            b.ins().trap(TrapCode::unwrap_user(1));
            seal_ready(b, error_block, seal_state);
            b.switch_to_block(valid_block);
            let pointer_type = module.target_config().pointer_type();
            let byte_offset = b
                .ins()
                .imul_imm_s(index64, (storage_slot_width(*ty, env.structs) * 8) as i64);
            let byte_offset = if pointer_type == types::I64 {
                byte_offset
            } else {
                b.ins().ireduce(pointer_type, byte_offset)
            };
            let address = b.ins().iadd(ptr, byte_offset);
            seal_ready(b, valid_block, seal_state);
            CompiledValue::Integer(
                address,
                Type::Reference(crate::sema::intern_pointer_target(*ty), false),
            )
        }
        IrExpression::StringAsStr(value) => {
            let CompiledValue::OwnedString { ptr, temporary } =
                emit_expr(b, module, value, env, seal_state)?
            else {
                return Err("internal error: String borrow is not an owning String".into());
            };
            let data_call = b.ins().call(
                env.print_functions.owned_strings[StringOp::Data as usize],
                &[ptr],
            );
            let length_call = b.ins().call(
                env.print_functions.owned_strings[StringOp::Len as usize],
                &[ptr],
            );
            CompiledValue::StrViewOwned {
                ptr: b.func.dfg.inst_results(data_call)[0],
                len: b.func.dfg.inst_results(length_call)[0],
                owner: ptr,
                temporary,
            }
        }
        IrExpression::StringFindOption { value, enum_id } => {
            let option = env.enums.get(*enum_id).ok_or_else(|| {
                "internal error: String.find Option type is out of range".to_string()
            })?;
            let some_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "Some")
                .ok_or_else(|| "internal error: Option<u64> has no Some variant".to_string())?;
            let none_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "None")
                .ok_or_else(|| "internal error: Option<u64> has no None variant".to_string())?;
            let CompiledValue::Integer(index, Type::I64) =
                emit_expr(b, module, value, env, seal_state)?
            else {
                return Err("internal error: String.find runtime result is not i64".into());
            };
            let pointer_type = module.target_config().pointer_type();
            let some_block = b.create_block();
            let none_block = b.create_block();
            let merge_block = b.create_block();
            b.append_block_param(merge_block, pointer_type);
            let found = b
                .ins()
                .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, index, 0);
            let from = b
                .current_block()
                .ok_or("internal error: String.find has no source block")?;
            b.ins().brif(found, some_block, &[], none_block, &[]);
            seal_ready(b, from, seal_state);

            b.switch_to_block(some_block);
            let payload_slot =
                b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
            b.ins().stack_store(types::I64, index, payload_slot, 0);
            let payload = b.ins().stack_addr(pointer_type, payload_slot, 0);
            let tag = b.ins().iconst(types::I64, some_tag as i64);
            let payload_len = b.ins().iconst(types::I64, 8);
            let no_drops = b.ins().iconst(pointer_type, 0);
            let no_drop_len = b.ins().iconst(types::I64, 0);
            let some_value = b.ins().call(
                env.print_functions.enum_new,
                &[tag, payload, payload_len, no_drops, no_drop_len],
            );
            let some_ptr = b.func.dfg.inst_results(some_value)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(some_ptr)]);
            let from = b
                .current_block()
                .ok_or("internal error: Some block is missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(none_block);
            let tag = b.ins().iconst(types::I64, none_tag as i64);
            let no_payload = b.ins().iconst(pointer_type, 0);
            let no_payload_len = b.ins().iconst(types::I64, 0);
            let no_drops = b.ins().iconst(pointer_type, 0);
            let no_drop_len = b.ins().iconst(types::I64, 0);
            let none_value = b.ins().call(
                env.print_functions.enum_new,
                &[tag, no_payload, no_payload_len, no_drops, no_drop_len],
            );
            let none_ptr = b.func.dfg.inst_results(none_value)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(none_ptr)]);
            let from = b
                .current_block()
                .ok_or("internal error: None block is missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(merge_block);
            seal_ready(b, merge_block, seal_state);
            CompiledValue::Enum {
                ptr: b.block_params(merge_block)[0],
                temporary: true,
            }
        }
        IrExpression::StructValue { struct_id, fields } => {
            let definition = env.structs.get(*struct_id).ok_or_else(|| {
                "internal error: structure literal type is out of range".to_string()
            })?;
            let mut compiled_fields = (0..definition.fields.len())
                .map(|_| None)
                .collect::<Vec<Option<CompiledValue>>>();
            for (field_index, value) in fields {
                let slot = compiled_fields.get_mut(*field_index).ok_or_else(|| {
                    "internal error: structure literal field index is out of range".to_string()
                })?;
                if slot.is_some() {
                    return Err(
                        "internal error: structure literal initializes a field twice".into(),
                    );
                }
                *slot = Some(emit_expr(b, module, value, env, seal_state)?);
            }
            let compiled_fields = compiled_fields
                .into_iter()
                .map(|field| {
                    field.ok_or_else(|| {
                        "internal error: structure literal is missing a field".to_string()
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            CompiledValue::Struct {
                struct_id: *struct_id,
                fields: compiled_fields,
            }
        }
        IrExpression::Field {
            value,
            struct_id,
            field_index,
            ..
        } => {
            let value = emit_expr(b, module, value, env, seal_state)?;
            let CompiledValue::Struct {
                struct_id: actual,
                fields,
            } = value
            else {
                return Err("internal error: field access base is not a structure".into());
            };
            if actual != *struct_id {
                return Err("internal error: field access structure type mismatch".into());
            }
            let mut selected = None;
            for (index, field) in fields.into_iter().enumerate() {
                if index == *field_index {
                    selected = Some(field);
                } else {
                    drop_temporary(b, env.print_functions, field);
                }
            }
            selected.ok_or_else(|| "internal error: field index is out of range".to_string())?
        }
        IrExpression::ValueAddress {
            value,
            ty,
            pointer_type,
            ..
        } => {
            let compiled = emit_expr(b, module, value, env, seal_state)?;
            let flattened = flatten_value(compiled);
            let bytes = (storage_slot_width(*ty, env.structs) * 8) as u32;
            let slot = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                bytes.max(8),
                3,
            ));
            let address = b
                .ins()
                .stack_addr(module.target_config().pointer_type(), slot, 0);
            for (index, word) in flattened.iter().enumerate() {
                b.ins()
                    .store(MemFlagsData::new(), *word, address, (index * 8) as i32);
            }
            let _ = ty;
            CompiledValue::Integer(address, *pointer_type)
        }
        IrExpression::ReferenceField {
            pointer,
            struct_id,
            field_index,
            borrowed,
            ..
        } => {
            let pointer = emit_expr(b, module, pointer, env, seal_state)?;
            let CompiledValue::Integer(pointer, _) = pointer else {
                return Err("internal error: reference field base is not a pointer".into());
            };
            let field = env.structs[*struct_id]
                .fields
                .get(*field_index)
                .ok_or("internal error: reference field index is out of range")?;
            let c_layout = env.structs[*struct_id].repr_c;
            let offset = field_memory_offset(env.structs, *struct_id, *field_index, c_layout)?;
            load_reference_field(
                b,
                field.ty,
                pointer,
                offset,
                c_layout,
                module.target_config().pointer_type(),
                env.structs,
                env.print_functions,
                *borrowed,
            )?
        }
        IrExpression::Local { .. } => {
            return Err("internal error: local IR has an unsupported type".into());
        }
        IrExpression::Call {
            target,
            arguments,
            return_type,
        } => {
            let values = emit_call(b, module, *target, arguments, env, seal_state)?;
            match return_type {
                Type::I8
                | Type::I16
                | Type::I32
                | Type::I64
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Char => CompiledValue::Integer(values[0], *return_type),
                Type::F32 => CompiledValue::F32(values[0]),
                Type::F64 => CompiledValue::F64(values[0]),
                Type::Bool => CompiledValue::Bool(values[0]),
                Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
                    CompiledValue::Integer(values[0], *return_type)
                }
                Type::OwnedString => CompiledValue::OwnedString {
                    ptr: values[0],
                    temporary: true,
                },
                Type::Vec(_) => CompiledValue::Vec {
                    ptr: values[0],
                    temporary: true,
                },
                Type::Map(_) | Type::Set(_) => CompiledValue::Map {
                    ptr: values[0],
                    temporary: true,
                },
                Type::Enum(_) => CompiledValue::Enum {
                    ptr: values[0],
                    temporary: true,
                },
                Type::Str => CompiledValue::Str {
                    ptr: values[0],
                    len: values[1],
                },
                Type::Slice(_) => CompiledValue::Slice {
                    ptr: values[0],
                    len: values[1],
                },
                Type::Struct(struct_id) => {
                    let mut offset = 0;
                    let fields = env.structs[*struct_id]
                        .fields
                        .iter()
                        .map(|field| {
                            compiled_value_from_type(field.ty, &values, &mut offset, env.structs)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    CompiledValue::Struct {
                        struct_id: *struct_id,
                        fields,
                    }
                }
                Type::Array(_) => {
                    let mut offset = 0;
                    compiled_value_from_type(*return_type, &values, &mut offset, env.structs)?
                }
            }
        }
        IrExpression::If { .. } => emit_if_expression(b, module, expr, env, seal_state)?,
        IrExpression::EnumMatch { .. } => emit_enum_match(b, module, expr, env, seal_state)?,
        IrExpression::Propagate { .. } => emit_propagate(b, module, expr, env, seal_state)?,
        IrExpression::Negate(inner, ty) => {
            let (value, actual_ty) = as_numeric(emit_expr(b, module, inner, env, seal_state)?)?;
            if actual_ty != *ty {
                return Err("internal error: negation type differs from checked type".into());
            }
            match ty {
                Type::I8 | Type::I16 | Type::I32 | Type::I64 => {
                    CompiledValue::Integer(b.ins().ineg(value), *ty)
                }
                Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
                    return Err(
                        "internal error: unsigned integer negation reached code generation".into(),
                    );
                }
                Type::F32 => CompiledValue::F32(b.ins().fneg(value)),
                Type::F64 => CompiledValue::F64(b.ins().fneg(value)),
                _ => return Err("internal error: invalid negation type".into()),
            }
        }
        IrExpression::Not(inner) => {
            let value = as_bool(emit_expr(b, module, inner, env, seal_state)?)?;
            CompiledValue::Bool(b.ins().icmp_imm_s(IntCC::Equal, value, 0))
        }
        IrExpression::BitNot(inner, ty) => {
            let (value, actual_ty) = as_numeric(emit_expr(b, module, inner, env, seal_state)?)?;
            if actual_ty != *ty || clif_integer_type(*ty).is_none() {
                return Err(
                    "internal error: bitwise complement type differs from checked integer type"
                        .into(),
                );
            }
            CompiledValue::Integer(b.ins().bnot(value), *ty)
        }
        IrExpression::FunctionAddress { function, ty } => {
            let address = b
                .ins()
                .func_addr(module.target_config().pointer_type(), env.calls[*function]);
            CompiledValue::Integer(address, *ty)
        }
        IrExpression::Cast {
            value,
            source,
            target,
        } => {
            if matches!(target, Type::RawPointer(_)) && integer_width(*source).is_some() {
                let (value, actual_source) =
                    as_numeric(emit_expr(b, module, value, env, seal_state)?)?;
                if actual_source != *source {
                    return Err(
                        "internal error: address cast source differs from checked type".into(),
                    );
                }
                let pointer_type = module.target_config().pointer_type();
                let source_width = integer_width(*source).expect("integer source");
                let converted = match u32::from(source_width).cmp(&pointer_type.bits()) {
                    std::cmp::Ordering::Less if is_signed_integer_type(*source) => {
                        b.ins().sextend(pointer_type, value)
                    }
                    std::cmp::Ordering::Less => b.ins().uextend(pointer_type, value),
                    std::cmp::Ordering::Greater => b.ins().ireduce(pointer_type, value),
                    std::cmp::Ordering::Equal => value,
                };
                CompiledValue::Integer(converted, *target)
            } else if matches!(source, Type::RawPointer(_) | Type::FunctionPointer(_))
                && matches!(target, Type::RawPointer(_) | Type::FunctionPointer(_))
            {
                let CompiledValue::Integer(pointer, actual_source) =
                    emit_expr(b, module, value, env, seal_state)?
                else {
                    return Err("internal error: pointer cast source is not a pointer value".into());
                };
                if actual_source != *source {
                    return Err(
                        "internal error: pointer cast source differs from checked type".into(),
                    );
                }
                CompiledValue::Integer(pointer, *target)
            } else if matches!(source, Type::RawPointer(_) | Type::FunctionPointer(_))
                && matches!(target, Type::U64 | Type::I64)
            {
                let CompiledValue::Integer(pointer, _) =
                    emit_expr(b, module, value, env, seal_state)?
                else {
                    return Err("internal error: pointer address cast is not a pointer".into());
                };
                let integer = if module.target_config().pointer_type().bits() < 64 {
                    b.ins().uextend(types::I64, pointer)
                } else {
                    pointer
                };
                CompiledValue::Integer(integer, *target)
            } else if *source == Type::Char {
                let CompiledValue::Integer(value, Type::Char) =
                    emit_expr(b, module, value, env, seal_state)?
                else {
                    return Err("internal error: char cast source is not a char".into());
                };
                let target_clif = clif_integer_type(*target).ok_or_else(|| {
                    "internal error: char cast target is not an integer".to_string()
                })?;
                let converted = match target_clif.bits().cmp(&32) {
                    std::cmp::Ordering::Less => b.ins().ireduce(target_clif, value),
                    std::cmp::Ordering::Greater => b.ins().uextend(target_clif, value),
                    std::cmp::Ordering::Equal => value,
                };
                CompiledValue::Integer(converted, *target)
            } else {
                let (value, actual_source) =
                    as_numeric(emit_expr(b, module, value, env, seal_state)?)?;
                if actual_source != *source {
                    return Err(
                        "internal error: numeric cast source differs from checked type".into(),
                    );
                }
                if let Some(source_width) = integer_width(*source) {
                    if let Some(target_width) = integer_width(*target) {
                        let target_clif = clif_integer_type(*target).ok_or_else(|| {
                            "internal error: integer cast target is not an integer".to_string()
                        })?;
                        let converted = match target_width.cmp(&source_width) {
                            std::cmp::Ordering::Less => b.ins().ireduce(target_clif, value),
                            std::cmp::Ordering::Greater if is_signed_integer_type(*source) => {
                                b.ins().sextend(target_clif, value)
                            }
                            std::cmp::Ordering::Greater => b.ins().uextend(target_clif, value),
                            std::cmp::Ordering::Equal => value,
                        };
                        CompiledValue::Integer(converted, *target)
                    } else {
                        let converted = match target {
                            Type::F32 if is_signed_integer_type(*source) => {
                                b.ins().fcvt_from_sint(types::F32, value)
                            }
                            Type::F64 if is_signed_integer_type(*source) => {
                                b.ins().fcvt_from_sint(types::F64, value)
                            }
                            Type::F32 => b.ins().fcvt_from_uint(types::F32, value),
                            Type::F64 => b.ins().fcvt_from_uint(types::F64, value),
                            _ => {
                                return Err(
                                    "internal error: numeric cast target is not numeric".into()
                                );
                            }
                        };
                        if *target == Type::F32 {
                            CompiledValue::F32(converted)
                        } else {
                            CompiledValue::F64(converted)
                        }
                    }
                } else {
                    if integer_width(*target).is_some() {
                        let target_clif = clif_integer_type(*target).ok_or_else(|| {
                            "internal error: float cast target is not an integer".to_string()
                        })?;
                        let target_width = integer_width(*target).ok_or_else(|| {
                            "internal error: float cast target has no width".to_string()
                        })?;
                        let converted = if is_signed_integer_type(*target) {
                            b.ins().fcvt_to_sint_sat(types::I64, value)
                        } else {
                            b.ins().fcvt_to_uint_sat(types::I64, value)
                        };
                        let converted = if target_width == 64 {
                            converted
                        } else if is_signed_integer_type(*target) {
                            let minimum = -(1i64 << (target_width - 1));
                            let maximum = (1i64 << (target_width - 1)) - 1;
                            let minimum_value = b.ins().iconst(types::I64, minimum);
                            let maximum_value = b.ins().iconst(types::I64, maximum);
                            let below_minimum =
                                b.ins()
                                    .icmp(IntCC::SignedLessThan, converted, minimum_value);
                            let lower_clamped =
                                b.ins().select(below_minimum, minimum_value, converted);
                            let above_maximum = b.ins().icmp(
                                IntCC::SignedGreaterThan,
                                lower_clamped,
                                maximum_value,
                            );
                            b.ins().select(above_maximum, maximum_value, lower_clamped)
                        } else {
                            let maximum = ((1u64 << target_width) - 1) as i64;
                            let maximum_value = b.ins().iconst(types::I64, maximum);
                            let above_maximum =
                                b.ins()
                                    .icmp(IntCC::UnsignedGreaterThan, converted, maximum_value);
                            b.ins().select(above_maximum, maximum_value, converted)
                        };
                        let converted = if target_width < 64 {
                            b.ins().ireduce(target_clif, converted)
                        } else {
                            converted
                        };
                        CompiledValue::Integer(converted, *target)
                    } else {
                        let converted = match (source, target) {
                            (Type::F32, Type::F64) => b.ins().fpromote(types::F64, value),
                            (Type::F64, Type::F32) => b.ins().fdemote(types::F32, value),
                            (Type::F32, Type::F32) | (Type::F64, Type::F64) => value,
                            _ => {
                                return Err(
                                "internal error: unsupported numeric cast reached code generation"
                                    .into(),
                            );
                            }
                        };
                        if *target == Type::F32 {
                            CompiledValue::F32(converted)
                        } else {
                            CompiledValue::F64(converted)
                        }
                    }
                }
            }
        }
        IrExpression::Binary {
            op,
            left,
            right,
            ty,
        } => {
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                return Ok(CompiledValue::Bool(emit_short_circuit(
                    b, module, *op, left, right, env, seal_state,
                )?));
            }
            let left = emit_expr(b, module, left, env, seal_state)?;
            let right = emit_expr(b, module, right, env, seal_state)?;
            if matches!(
                op,
                BinaryOp::BitAnd
                    | BinaryOp::BitXor
                    | BinaryOp::BitOr
                    | BinaryOp::ShiftLeft
                    | BinaryOp::ShiftRight
            ) {
                let (left, left_ty) = as_numeric(left)?;
                let (right, right_ty) = as_numeric(right)?;
                let shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
                if left_ty != *ty
                    || (if shift {
                        right_ty != Type::U32
                    } else {
                        right_ty != *ty
                    })
                    || clif_integer_type(*ty).is_none()
                {
                    return Err(
                        "internal error: bitwise or shift operands differ from checked types"
                            .into(),
                    );
                }
                let result = match op {
                    BinaryOp::BitAnd => b.ins().band(left, right),
                    BinaryOp::BitXor => b.ins().bxor(left, right),
                    BinaryOp::BitOr => b.ins().bor(left, right),
                    BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
                        let width = match ty {
                            Type::I8 | Type::U8 => 8,
                            Type::I16 | Type::U16 => 16,
                            Type::I32 | Type::U32 => 32,
                            Type::I64 | Type::U64 => 64,
                            _ => {
                                return Err(
                                    "internal error: shift type differs from checked integer type"
                                        .into(),
                                );
                            }
                        };
                        let within_width =
                            b.ins()
                                .icmp_imm_u(IntCC::UnsignedLessThan, right, i64::from(width));
                        let shift_count = match ty {
                            Type::I8 | Type::U8 => b.ins().ireduce(types::I8, right),
                            Type::I16 | Type::U16 => b.ins().ireduce(types::I16, right),
                            Type::I32 | Type::U32 => right,
                            Type::I64 | Type::U64 => b.ins().uextend(types::I64, right),
                            _ => unreachable!(),
                        };
                        let zero_shift = b.ins().iconst(clif_integer_type(*ty).unwrap(), 0);
                        let safe_shift_count =
                            b.ins().select(within_width, shift_count, zero_shift);
                        let shifted = match op {
                            BinaryOp::ShiftLeft => b.ins().ishl(left, safe_shift_count),
                            BinaryOp::ShiftRight
                                if matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64) =>
                            {
                                b.ins().sshr(left, safe_shift_count)
                            }
                            BinaryOp::ShiftRight => b.ins().ushr(left, safe_shift_count),
                            _ => unreachable!(),
                        };
                        let out_of_range = if matches!(
                            (op, ty),
                            (
                                BinaryOp::ShiftRight,
                                Type::I8 | Type::I16 | Type::I32 | Type::I64
                            )
                        ) {
                            let is_negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, left, 0);
                            let all_ones = b.ins().iconst(clif_integer_type(*ty).unwrap(), -1);
                            let zero = b.ins().iconst(clif_integer_type(*ty).unwrap(), 0);
                            b.ins().select(is_negative, all_ones, zero)
                        } else {
                            b.ins().iconst(clif_integer_type(*ty).unwrap(), 0)
                        };
                        b.ins().select(within_width, shifted, out_of_range)
                    }
                    _ => unreachable!(),
                };
                CompiledValue::Integer(result, *ty)
            } else if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) {
                let (left, left_ty) = as_numeric(left)?;
                let (right, right_ty) = as_numeric(right)?;
                if left_ty != *ty || right_ty != *ty {
                    return Err(
                        "internal error: arithmetic operand differs from checked type".into(),
                    );
                }
                if matches!(ty, Type::F32 | Type::F64) {
                    let result = match op {
                        BinaryOp::Add => b.ins().fadd(left, right),
                        BinaryOp::Sub => b.ins().fsub(left, right),
                        BinaryOp::Mul => b.ins().fmul(left, right),
                        BinaryOp::Div => b.ins().fdiv(left, right),
                        BinaryOp::Rem => {
                            return Err(
                                "internal error: float remainder reached code generation".into()
                            );
                        }
                        _ => unreachable!(),
                    };
                    if *ty == Type::F32 {
                        CompiledValue::F32(result)
                    } else {
                        CompiledValue::F64(result)
                    }
                } else {
                    let unsigned = matches!(ty, Type::U8 | Type::U16 | Type::U32 | Type::U64);
                    let result = match op {
                        BinaryOp::Add => b.ins().iadd(left, right),
                        BinaryOp::Sub => b.ins().isub(left, right),
                        BinaryOp::Mul => b.ins().imul(left, right),
                        BinaryOp::Div | BinaryOp::Rem => emit_checked_integer_division(
                            b,
                            *op,
                            [left, right],
                            *ty,
                            unsigned,
                            env.print_functions,
                            seal_state,
                        )?,
                        _ => unreachable!(),
                    };
                    CompiledValue::Integer(result, *ty)
                }
            } else if *ty == Type::Bool {
                let left = as_bool(left)?;
                let right = as_bool(right)?;
                let condition = match op {
                    BinaryOp::Eq => b.ins().icmp(IntCC::Equal, left, right),
                    BinaryOp::Ne => b.ins().icmp(IntCC::NotEqual, left, right),
                    _ => {
                        return Err(
                            "internal error: ordered bool comparison reached code generation"
                                .into(),
                        );
                    }
                };
                CompiledValue::Bool(condition)
            } else if *ty == Type::OwnedString {
                let equal =
                    emit_equality_value(b, env.print_functions, env.structs, *ty, left, right)?;
                CompiledValue::Bool(if *op == BinaryOp::Eq {
                    equal
                } else {
                    b.ins().icmp_imm_s(IntCC::Equal, equal, 0)
                })
            } else if *ty == Type::Str {
                // Either side may be a borrowed view of an owning String
                // (`name == "fun"`); release a temporary owner afterwards.
                let (left_ptr, left_len) = match left {
                    CompiledValue::Str { ptr, len }
                    | CompiledValue::StrViewOwned { ptr, len, .. } => (ptr, len),
                    _ => {
                        return Err(
                            "internal error: string comparison has a non-string left operand"
                                .into(),
                        );
                    }
                };
                let (right_ptr, right_len) = match right {
                    CompiledValue::Str { ptr, len }
                    | CompiledValue::StrViewOwned { ptr, len, .. } => (ptr, len),
                    _ => {
                        return Err(
                            "internal error: string comparison has a non-string right operand"
                                .into(),
                        );
                    }
                };
                let call = b.ins().call(
                    env.print_functions.string_equals,
                    &[left_ptr, left_len, right_ptr, right_len],
                );
                let equal = b.func.dfg.inst_results(call)[0];
                drop_temporary(b, env.print_functions, left);
                drop_temporary(b, env.print_functions, right);
                let condition = if *op == BinaryOp::Eq {
                    equal
                } else if *op == BinaryOp::Ne {
                    b.ins().icmp_imm_s(IntCC::Equal, equal, 0)
                } else {
                    return Err(
                        "internal error: ordered string comparison reached code generation".into(),
                    );
                };
                CompiledValue::Bool(condition)
            } else if let Type::Struct(struct_id) = ty {
                let equal = emit_struct_equality(
                    b,
                    env.print_functions,
                    env.structs,
                    *struct_id,
                    left,
                    right,
                )?;
                let condition = if *op == BinaryOp::Eq {
                    equal
                } else if *op == BinaryOp::Ne {
                    b.ins().icmp_imm_s(IntCC::Equal, equal, 0)
                } else {
                    return Err(
                        "internal error: ordered structure comparison reached code generation"
                            .into(),
                    );
                };
                CompiledValue::Bool(condition)
            } else {
                let (left, left_ty) = as_numeric(left)?;
                let (right, right_ty) = as_numeric(right)?;
                if left_ty != *ty || right_ty != *ty {
                    return Err(
                        "internal error: comparison operand differs from checked type".into(),
                    );
                }
                if matches!(ty, Type::F32 | Type::F64) {
                    let cc = match op {
                        BinaryOp::Eq => FloatCC::Equal,
                        BinaryOp::Ne => FloatCC::NotEqual,
                        BinaryOp::Lt => FloatCC::LessThan,
                        BinaryOp::Le => FloatCC::LessThanOrEqual,
                        BinaryOp::Gt => FloatCC::GreaterThan,
                        BinaryOp::Ge => FloatCC::GreaterThanOrEqual,
                        _ => unreachable!(),
                    };
                    CompiledValue::Bool(b.ins().fcmp(cc, left, right))
                } else {
                    let unsigned = matches!(ty, Type::U8 | Type::U16 | Type::U32 | Type::U64);
                    let cc = match op {
                        BinaryOp::Eq => IntCC::Equal,
                        BinaryOp::Ne => IntCC::NotEqual,
                        BinaryOp::Lt if unsigned => IntCC::UnsignedLessThan,
                        BinaryOp::Le if unsigned => IntCC::UnsignedLessThanOrEqual,
                        BinaryOp::Gt if unsigned => IntCC::UnsignedGreaterThan,
                        BinaryOp::Ge if unsigned => IntCC::UnsignedGreaterThanOrEqual,
                        BinaryOp::Lt => IntCC::SignedLessThan,
                        BinaryOp::Le => IntCC::SignedLessThanOrEqual,
                        BinaryOp::Gt => IntCC::SignedGreaterThan,
                        BinaryOp::Ge => IntCC::SignedGreaterThanOrEqual,
                        _ => unreachable!(),
                    };
                    CompiledValue::Bool(b.ins().icmp(cc, left, right))
                }
            }
        }
    })
}

fn emit_vec_call(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    operation: VecOp,
    elem_id: usize,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Vec<Value>, String> {
    let elem = vec_elem(elem_id);
    let pointer_type = module.target_config().pointer_type();
    fn eval_arg(
        b: &mut FunctionBuilder<'_>,
        module: &ObjectModule,
        argument: &IrExpression,
        env: &ExprEnv<'_>,
        seal_state: &mut BlockSealState,
    ) -> Result<CompiledValue, String> {
        emit_expr(b, module, argument, env, seal_state)
    }
    match operation {
        VecOp::New => {
            let stride = b
                .ins()
                .iconst(types::I64, value_width(elem, env.structs) as i64);
            let (drop_ptr, clone_ptr) = match elem {
                Type::OwnedString => {
                    let slot = b.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        16,
                        3,
                    ));
                    let addr = b.ins().stack_addr(pointer_type, slot, 0);
                    let addr2 = b.ins().iadd_imm_s(addr, 8);
                    b.ins()
                        .call(env.print_functions.vec_string_element, &[addr, addr2]);
                    let drop_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 0);
                    let clone_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 8);
                    (drop_ptr, clone_ptr)
                }
                Type::Map(_) => {
                    let slot = b.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        16,
                        3,
                    ));
                    let addr = b.ins().stack_addr(pointer_type, slot, 0);
                    let addr2 = b.ins().iadd_imm_s(addr, 8);
                    b.ins()
                        .call(env.print_functions.vec_map_element, &[addr, addr2]);
                    let drop_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 0);
                    let clone_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 8);
                    (drop_ptr, clone_ptr)
                }
                Type::Enum(_) => {
                    let slot = b.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        16,
                        3,
                    ));
                    let addr = b.ins().stack_addr(pointer_type, slot, 0);
                    let addr2 = b.ins().iadd_imm_s(addr, 8);
                    b.ins()
                        .call(env.print_functions.vec_enum_element, &[addr, addr2]);
                    let drop_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 0);
                    let clone_ptr = b.ins().load(pointer_type, MemFlagsData::new(), addr, 8);
                    (drop_ptr, clone_ptr)
                }
                Type::Struct(struct_id) => {
                    if let Some((drop_callback, clone_callback)) =
                        env.vec_struct_callbacks[struct_id]
                    {
                        (
                            b.ins().func_addr(pointer_type, drop_callback),
                            b.ins().func_addr(pointer_type, clone_callback),
                        )
                    } else {
                        (
                            b.ins().iconst(pointer_type, 0),
                            b.ins().iconst(pointer_type, 0),
                        )
                    }
                }
                _ => (
                    b.ins().iconst(pointer_type, 0),
                    b.ins().iconst(pointer_type, 0),
                ),
            };
            let call = b.ins().call(
                env.print_functions.owned_vecs[VecOp::New as usize],
                &[stride, drop_ptr, clone_ptr],
            );
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        VecOp::Len
        | VecOp::Capacity
        | VecOp::Clear
        | VecOp::Clone
        | VecOp::Drop
        | VecOp::IsEmpty
        | VecOp::Reverse => {
            let [argument] = arguments else {
                return Err("internal error: `Vec` operation expects the receiver".into());
            };
            let receiver = emit_expr(b, module, argument, env, seal_state)?;
            let receiver_values = flatten_value(receiver);
            let call = b.ins().call(
                env.print_functions.owned_vecs[operation as usize],
                &receiver_values,
            );
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        VecOp::Reserve => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let additional = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let mut args = flatten_value(receiver);
            args.extend(flatten_value(additional));
            let call = b
                .ins()
                .call(env.print_functions.owned_vecs[operation as usize], &args);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        VecOp::Sort => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let receiver_values = flatten_value(receiver);
            let kind = match elem {
                Type::I8 => 0,
                Type::U8 => 1,
                Type::I16 => 2,
                Type::U16 => 3,
                Type::I32 => 4,
                Type::U32 => 5,
                Type::I64 => 6,
                Type::U64 => 7,
                Type::F32 => 8,
                Type::F64 => 9,
                Type::Char => 11,
                Type::OwnedString => 12,
                _ => return Err("internal error: unsupported Vec.sort element type".into()),
            };
            let kind = b.ins().iconst(types::I64, kind);
            b.ins().call(
                env.print_functions.owned_vecs[VecOp::Sort as usize],
                &[receiver_values[0], kind],
            );
            Ok(Vec::new())
        }
        VecOp::Contains => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let element = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let elem_addr = emit_elem_slot(b, element.clone(), elem, pointer_type, env.structs)?;
            let kind = match elem {
                Type::I8 => 0,
                Type::U8 => 1,
                Type::I16 => 2,
                Type::U16 => 3,
                Type::I32 => 4,
                Type::U32 => 5,
                Type::I64 => 6,
                Type::U64 => 7,
                Type::F32 => 8,
                Type::F64 => 9,
                Type::Bool => 10,
                Type::Char => 11,
                Type::OwnedString => 12,
                _ => return Err("internal error: unsupported Vec.contains element type".into()),
            };
            let kind = b.ins().iconst(types::I64, kind);
            let vector = flatten_value(receiver);
            let call = b.ins().call(
                env.print_functions.owned_vecs[VecOp::Contains as usize],
                &[vector[0], elem_addr, kind],
            );
            let result = b.func.dfg.inst_results(call).to_vec();
            drop_temporary(b, env.print_functions, element);
            Ok(result)
        }
        VecOp::Push => {
            let receiver = eval_arg(b, module, &arguments[0], env, seal_state)?;
            let element = eval_arg(b, module, &arguments[1], env, seal_state)?;
            let elem_addr = emit_elem_slot(b, element, elem, pointer_type, env.structs)?;
            let receiver_values = flatten_value(receiver);
            let call = b.ins().call(
                env.print_functions.owned_vecs[VecOp::Push as usize],
                &[receiver_values[0], elem_addr],
            );
            let _ = call;
            Ok(Vec::new())
        }
        VecOp::Set => {
            let receiver = eval_arg(b, module, &arguments[0], env, seal_state)?;
            let index = eval_arg(b, module, &arguments[1], env, seal_state)?;
            let element = eval_arg(b, module, &arguments[2], env, seal_state)?;
            let elem_addr = emit_elem_slot(b, element, elem, pointer_type, env.structs)?;
            let receiver_values = flatten_value(receiver);
            let index_values = flatten_value(index);
            b.ins().call(
                env.print_functions.owned_vecs[VecOp::Set as usize],
                &[receiver_values[0], index_values[0], elem_addr],
            );
            Ok(Vec::new())
        }
        VecOp::Insert => {
            let receiver = eval_arg(b, module, &arguments[0], env, seal_state)?;
            let index = eval_arg(b, module, &arguments[1], env, seal_state)?;
            let element = eval_arg(b, module, &arguments[2], env, seal_state)?;
            let elem_addr = emit_elem_slot(b, element, elem, pointer_type, env.structs)?;
            let receiver_values = flatten_value(receiver);
            let index_values = flatten_value(index);
            b.ins().call(
                env.print_functions.owned_vecs[VecOp::Insert as usize],
                &[receiver_values[0], index_values[0], elem_addr],
            );
            Ok(Vec::new())
        }
        VecOp::Index => {
            let receiver = eval_arg(b, module, &arguments[0], env, seal_state)?;
            let index = eval_arg(b, module, &arguments[1], env, seal_state)?;
            let receiver_values = flatten_value(receiver);
            let index_values = flatten_value(index);
            let call = b.ins().call(
                env.print_functions.owned_vecs[VecOp::Index as usize],
                &[receiver_values[0], index_values[0]],
            );
            let slot_ptr = b.func.dfg.inst_results(call)[0];
            let mut result = emit_elem_load(b, slot_ptr, elem, pointer_type, env.structs)?;
            // Reading an element yields an independent owner; the Vec keeps
            // its own copy.
            if matches!(
                elem,
                Type::Struct(_)
                    | Type::OwnedString
                    | Type::Vec(_)
                    | Type::Map(_)
                    | Type::Set(_)
                    | Type::Enum(_)
            ) {
                result = clone_array_element(b, env.print_functions, result)?;
            }
            Ok(flatten_value(result))
        }
        VecOp::Take | VecOp::Extract => {
            let receiver = eval_arg(b, module, &arguments[0], env, seal_state)?;
            let index = eval_arg(b, module, &arguments[1], env, seal_state)?;
            let receiver_values = flatten_value(receiver);
            let index_values = flatten_value(index);
            let out_size = u32::try_from(value_width(elem, env.structs).saturating_mul(8))
                .map_err(|_| "internal error: Vec element layout is too large".to_string())?;
            let out_slot = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                out_size,
                3,
            ));
            let out_addr = b.ins().stack_addr(pointer_type, out_slot, 0);
            b.ins().call(
                env.print_functions.owned_vecs[operation as usize],
                &[receiver_values[0], index_values[0], out_addr],
            );
            let result = emit_elem_load(b, out_addr, elem, pointer_type, env.structs)?;
            Ok(flatten_value(result))
        }
        VecOp::GetOption => {
            Err("internal error: Vec.get Option must use its specialized lowering".into())
        }
        VecOp::PopOption => {
            Err("internal error: Vec.pop Option must use its specialized lowering".into())
        }
    }
}

fn emit_map_call(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    operation: MapOp,
    map_id: usize,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Vec<Value>, String> {
    let (key, value) = map_info(map_id);
    let pointer_type = module.target_config().pointer_type();
    let runtime = env.print_functions.maps[operation as usize];
    match operation {
        MapOp::New => {
            let key_kind = i64::from(key == Type::OwnedString);
            let value_kind = match value {
                Type::OwnedString => 1,
                Type::Vec(_) => 2,
                Type::Map(_) | Type::Set(_) => 3,
                Type::Struct(id) if env.structs[id].drop_function.is_some() => 4,
                Type::Struct(_) if type_has_custom_drop(value, env.structs) => 6,
                Type::Struct(_) => 5,
                _ => 0,
            };
            let mut args = vec![
                b.ins()
                    .iconst(types::I64, storage_slot_width(key, env.structs) as i64),
                b.ins()
                    .iconst(types::I64, value_width(value, env.structs) as i64),
                b.ins().iconst(types::I64, key_kind),
                b.ins().iconst(types::I64, value_kind),
            ];
            if let Type::Struct(id) = value {
                if let Some((drop_callback, clone_callback)) = env.vec_struct_callbacks[id] {
                    args.push(b.ins().func_addr(pointer_type, drop_callback));
                    args.push(b.ins().func_addr(pointer_type, clone_callback));
                } else {
                    args.push(b.ins().iconst(pointer_type, 0));
                    args.push(b.ins().iconst(pointer_type, 0));
                }
            } else {
                args.push(b.ins().iconst(pointer_type, 0));
                args.push(b.ins().iconst(pointer_type, 0));
            }
            let call = b.ins().call(runtime, &args);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::Drop | MapOp::Len | MapOp::Clear | MapOp::IsEmpty => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let receiver = flatten_value(receiver);
            let call = b.ins().call(runtime, &receiver);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::Clone => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let receiver = flatten_value(receiver);
            let call = b.ins().call(runtime, &receiver);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::Keys | MapOp::Values => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let receiver = flatten_value(receiver);
            let call = b.ins().call(runtime, &receiver);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::ContainsKey | MapOp::Remove => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let key_value = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let key_slot = emit_elem_slot(b, key_value.clone(), key, pointer_type, env.structs)?;
            let receiver = flatten_value(receiver);
            let call = b.ins().call(runtime, &[receiver[0], key_slot]);
            drop_temporary(b, env.print_functions, key_value);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::Insert => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let key_value = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let value_value = emit_expr(b, module, &arguments[2], env, seal_state)?;
            let key_slot = emit_elem_slot(b, key_value, key, pointer_type, env.structs)?;
            let value_slot = emit_elem_slot(b, value_value, value, pointer_type, env.structs)?;
            let receiver = flatten_value(receiver);
            let call = b.ins().call(runtime, &[receiver[0], key_slot, value_slot]);
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        MapOp::Get => {
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let key_value = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let key_slot = emit_elem_slot(b, key_value.clone(), key, pointer_type, env.structs)?;
            let output_slot = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                (value_width(value, env.structs) * 8) as u32,
                3,
            ));
            let output = b.ins().stack_addr(pointer_type, output_slot, 0);
            let receiver = flatten_value(receiver);
            let get_call = b.ins().call(runtime, &[receiver[0], key_slot, output]);
            let found = b.func.dfg.inst_results(get_call)[0];
            drop_temporary(b, env.print_functions, key_value);
            let option_id = env
                .enums
                .iter()
                .position(|definition| {
                    definition.name.starts_with("$RynOption#")
                        && definition.variants.len() == 2
                        && definition.variants[0].name == "Some"
                        && definition.variants[0].fields == [value]
                        && definition.variants[1].name == "None"
                        && definition.variants[1].fields.is_empty()
                })
                .ok_or("internal error: Map.get Option specialization is missing")?;
            let option = &env.enums[option_id];
            let some_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "Some")
                .ok_or("internal error: Map.get Option has no Some variant")?;
            let none_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "None")
                .ok_or("internal error: Map.get Option has no None variant")?;
            let some_block = b.create_block();
            let none_block = b.create_block();
            let merge_block = b.create_block();
            b.append_block_param(merge_block, pointer_type);
            let is_found = b.ins().icmp_imm_u(IntCC::Equal, found, 1);
            let from = b
                .current_block()
                .ok_or("internal error: Map.get has no source block")?;
            b.ins().brif(is_found, some_block, &[], none_block, &[]);
            seal_ready(b, from, seal_state);

            b.switch_to_block(some_block);
            let tag = b.ins().iconst(types::I64, some_tag as i64);
            let payload_len = b
                .ins()
                .iconst(types::I64, (value_width(value, env.structs) * 8) as i64);
            let drop_plan = if let Some(data_id) = env.enum_drop_plans[option_id] {
                let global = module.declare_data_in_func(data_id, b.func);
                b.ins().symbol_value(pointer_type, global)
            } else {
                b.ins().iconst(pointer_type, 0)
            };
            let drop_plan_len = b
                .ins()
                .iconst(types::I64, enum_drop_plan_len(option, env.structs) as i64);
            let some_call = b.ins().call(
                env.print_functions.enum_new,
                &[tag, output, payload_len, drop_plan, drop_plan_len],
            );
            let some_value = b.func.dfg.inst_results(some_call)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(some_value)]);
            let from = b
                .current_block()
                .ok_or("internal error: Map.get Some block is missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(none_block);
            let tag = b.ins().iconst(types::I64, none_tag as i64);
            let no_payload = b.ins().iconst(pointer_type, 0);
            let no_payload_len = b.ins().iconst(types::I64, 0);
            let no_drops = b.ins().iconst(pointer_type, 0);
            let no_drop_len = b.ins().iconst(types::I64, 0);
            let none_call = b.ins().call(
                env.print_functions.enum_new,
                &[tag, no_payload, no_payload_len, no_drops, no_drop_len],
            );
            let none_value = b.func.dfg.inst_results(none_call)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(none_value)]);
            let from = b
                .current_block()
                .ok_or("internal error: Map.get None block is missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(merge_block);
            seal_ready(b, merge_block, seal_state);
            Ok(vec![b.block_params(merge_block)[0]])
        }
    }
}

fn emit_elem_slot(
    b: &mut FunctionBuilder<'_>,
    value: CompiledValue,
    elem: Type,
    pointer_type: types::Type,
    structs: &[RynStruct],
) -> Result<Value, String> {
    let bytes = u32::try_from(value_width(elem, structs).saturating_mul(8))
        .map_err(|_| "internal error: Vec element layout is too large".to_string())?;
    let slot = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
    let addr = b.ins().stack_addr(pointer_type, slot, 0);
    for word in 0..value_width(elem, structs) {
        let zero = b.ins().iconst(types::I64, 0);
        b.ins()
            .store(MemFlagsData::new(), zero, addr, (word * 8) as i32);
    }
    match (elem, value) {
        (Type::I8 | Type::U8 | Type::Bool, CompiledValue::Integer(v, _)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::I16 | Type::U16, CompiledValue::Integer(v, _)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::I32 | Type::U32 | Type::Char, CompiledValue::Integer(v, _)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::I64 | Type::U64, CompiledValue::Integer(v, _)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::F32, CompiledValue::F32(v)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::F64, CompiledValue::F64(v)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::Bool, CompiledValue::Bool(v)) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::OwnedString, CompiledValue::OwnedString { ptr: v, .. }) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::Vec(_), CompiledValue::Vec { ptr: v, .. }) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::Map(_) | Type::Set(_), CompiledValue::Map { ptr: v, .. }) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (Type::Enum(_), CompiledValue::Enum { ptr: v, .. }) => {
            b.ins().store(MemFlagsData::new(), v, addr, 0);
        }
        (
            Type::Struct(struct_id),
            CompiledValue::Struct {
                struct_id: actual,
                fields,
            },
        ) if struct_id == actual && fields.len() == structs[struct_id].fields.len() => {
            for (field, value) in structs[struct_id].fields.iter().zip(fields) {
                let words = flatten_value(value);
                if words.len() != value_width(field.ty, structs) {
                    return Err(
                        "internal error: Vec structure field has an incompatible layout".into(),
                    );
                }
                for (word_index, word) in words.into_iter().enumerate() {
                    b.ins().store(
                        MemFlagsData::new(),
                        word,
                        addr,
                        ((field.slot_offset + word_index) * 8) as i32,
                    );
                }
            }
        }
        _ => return Err("internal error: cannot place a `Vec` element".into()),
    }
    Ok(addr)
}

fn emit_elem_load(
    b: &mut FunctionBuilder<'_>,
    ptr: Value,
    elem: Type,
    pointer_type: types::Type,
    structs: &[RynStruct],
) -> Result<CompiledValue, String> {
    Ok(match elem {
        Type::I8
        | Type::U8
        | Type::I16
        | Type::U16
        | Type::I32
        | Type::U32
        | Type::I64
        | Type::U64
        | Type::Char => {
            let clif_ty = clif_scalar_type(elem, pointer_type)?;
            CompiledValue::Integer(b.ins().load(clif_ty, MemFlagsData::new(), ptr, 0), elem)
        }
        Type::Bool => CompiledValue::Bool(b.ins().load(types::I8, MemFlagsData::new(), ptr, 0)),
        Type::F32 => CompiledValue::F32(b.ins().load(types::F32, MemFlagsData::new(), ptr, 0)),
        Type::F64 => CompiledValue::F64(b.ins().load(types::F64, MemFlagsData::new(), ptr, 0)),
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            CompiledValue::Integer(
                b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
                elem,
            )
        }
        Type::OwnedString => CompiledValue::OwnedString {
            ptr: b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
            temporary: true,
        },
        Type::Vec(_) => CompiledValue::Vec {
            ptr: b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
            temporary: true,
        },
        Type::Map(_) | Type::Set(_) => CompiledValue::Map {
            ptr: b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
            temporary: true,
        },
        Type::Enum(_) => CompiledValue::Enum {
            ptr: b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
            temporary: true,
        },
        Type::Str | Type::Slice(_) => CompiledValue::Str {
            ptr: b.ins().load(pointer_type, MemFlagsData::new(), ptr, 0),
            len: b.ins().load(types::I64, MemFlagsData::new(), ptr, 8),
        },
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                let value = emit_vec_struct_field_load(
                    b,
                    ptr,
                    field.ty,
                    field.slot_offset,
                    pointer_type,
                    structs,
                )?;
                fields.push(value);
            }
            CompiledValue::Struct { struct_id, fields }
        }
        _ => return Err("internal error: unsupported `Vec` element type".into()),
    })
}

fn emit_vec_struct_field_load(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    ty: Type,
    slot_offset: usize,
    pointer_type: types::Type,
    structs: &[RynStruct],
) -> Result<CompiledValue, String> {
    let pointer = callback_field_pointer(builder, base, slot_offset);
    Ok(match ty {
        Type::I8
        | Type::U8
        | Type::I16
        | Type::U16
        | Type::I32
        | Type::U32
        | Type::I64
        | Type::U64
        | Type::Char => CompiledValue::Integer(
            builder.ins().load(
                clif_scalar_type(ty, pointer_type)?,
                MemFlagsData::new(),
                pointer,
                0,
            ),
            ty,
        ),
        Type::Bool => CompiledValue::Bool(builder.ins().load(
            types::I8,
            MemFlagsData::new(),
            pointer,
            0,
        )),
        Type::F32 => CompiledValue::F32(builder.ins().load(
            types::F32,
            MemFlagsData::new(),
            pointer,
            0,
        )),
        Type::F64 => CompiledValue::F64(builder.ins().load(
            types::F64,
            MemFlagsData::new(),
            pointer,
            0,
        )),
        Type::OwnedString => CompiledValue::OwnedString {
            ptr: builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, 0),
            temporary: true,
        },
        Type::Vec(_) => CompiledValue::Vec {
            ptr: builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, 0),
            temporary: true,
        },
        Type::Map(_) | Type::Set(_) => CompiledValue::Map {
            ptr: builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, 0),
            temporary: true,
        },
        Type::Enum(_) => CompiledValue::Enum {
            ptr: builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, 0),
            temporary: true,
        },
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            CompiledValue::Integer(
                builder
                    .ins()
                    .load(pointer_type, MemFlagsData::new(), pointer, 0),
                ty,
            )
        }
        Type::Str | Type::Slice(_) => CompiledValue::Str {
            ptr: builder
                .ins()
                .load(pointer_type, MemFlagsData::new(), pointer, 0),
            len: builder
                .ins()
                .load(types::I64, MemFlagsData::new(), pointer, 8),
        },
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                fields.push(emit_vec_struct_field_load(
                    builder,
                    base,
                    field.ty,
                    slot_offset + field.slot_offset,
                    pointer_type,
                    structs,
                )?);
            }
            CompiledValue::Struct { struct_id, fields }
        }
        Type::Array(array_id) => {
            let (element, length) = array_info(array_id);
            let stride = storage_slot_width(element, structs);
            let mut values = Vec::with_capacity(length);
            for index in 0..length {
                values.push(emit_vec_struct_field_load(
                    builder,
                    base,
                    element,
                    slot_offset + index * stride,
                    pointer_type,
                    structs,
                )?);
            }
            CompiledValue::Array(values)
        }
    })
}

fn value_width(ty: Type, structs: &[RynStruct]) -> usize {
    match ty {
        Type::Struct(id) => structs[id].slot_count,
        Type::Array(id) => {
            let (element, length) = array_info(id);
            length.saturating_mul(value_width(element, structs))
        }
        Type::Str | Type::Slice(_) => 2,
        _ => 1,
    }
}

fn enum_payload_types(ty: Type, structs: &[RynStruct]) -> Vec<Type> {
    match ty {
        Type::Struct(id) => structs[id]
            .fields
            .iter()
            .flat_map(|field| enum_payload_types(field.ty, structs))
            .collect(),
        Type::Array(id) => {
            let (element, length) = array_info(id);
            (0..length)
                .flat_map(|_| enum_payload_types(element, structs))
                .collect()
        }
        other => vec![other],
    }
}

fn append_enum_drop_entries(
    bytes: &mut Vec<u8>,
    variant: usize,
    ty: Type,
    offset: usize,
    structs: &[RynStruct],
) {
    let kind = match ty {
        Type::OwnedString => Some(0),
        Type::Vec(_) => Some(1),
        Type::Enum(_) => Some(2),
        Type::Map(_) | Type::Set(_) => Some(3),
        Type::Struct(id) => {
            for field in &structs[id].fields {
                append_enum_drop_entries(
                    bytes,
                    variant,
                    field.ty,
                    offset + field.slot_offset,
                    structs,
                );
            }
            None
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let stride = value_width(element, structs);
            for index in 0..length {
                append_enum_drop_entries(bytes, variant, element, offset + index * stride, structs);
            }
            None
        }
        _ => None,
    };
    if let Some(kind) = kind {
        bytes.extend_from_slice(&(variant as u64).to_le_bytes());
        bytes.extend_from_slice(&(offset as u64).to_le_bytes());
        bytes.extend_from_slice(&(kind as u64).to_le_bytes());
    }
}

fn emit_call(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    target: IrCallTarget,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Vec<Value>, String> {
    match target {
        IrCallTarget::String(operation) => {
            let mut borrowed = Vec::new();
            let mut values = Vec::new();
            for argument in arguments {
                let value = emit_expr(b, module, argument, env, seal_state)?;
                values.extend(flatten_value(value.clone()));
                borrowed.push(value);
            }
            let call = b.ins().call(
                env.print_functions.owned_strings[operation as usize],
                &values,
            );
            let results = b.func.dfg.inst_results(call).to_vec();
            for value in borrowed.into_iter().rev() {
                drop_temporary(b, env.print_functions, value);
            }
            Ok(results)
        }
        IrCallTarget::Filesystem(operation) => {
            let mut borrowed = Vec::new();
            let mut values = Vec::new();
            for argument in arguments {
                let value = emit_expr(b, module, argument, env, seal_state)?;
                values.extend(flatten_value(value.clone()));
                borrowed.push(value);
            }
            let call = b
                .ins()
                .call(env.print_functions.filesystem[operation as usize], &values);
            let results = b.func.dfg.inst_results(call).to_vec();
            for value in borrowed.into_iter().rev() {
                drop_temporary(b, env.print_functions, value);
            }
            Ok(results)
        }
        IrCallTarget::TryReadFile(_) => {
            let argument = arguments
                .first()
                .ok_or("internal error: try_read_file argument missing")?;
            let value = emit_expr(b, module, argument, env, seal_state)?;
            let values = flatten_value(value.clone());
            let call = b.ins().call(env.print_functions.try_read_file, &values);
            let results = b.func.dfg.inst_results(call).to_vec();
            drop_temporary(b, env.print_functions, value);
            Ok(results)
        }
        IrCallTarget::ReadFileResult(_) => {
            let argument = arguments
                .first()
                .ok_or("internal error: read_file_result argument missing")?;
            let value = emit_expr(b, module, argument, env, seal_state)?;
            let values = flatten_value(value.clone());
            let call = b.ins().call(env.print_functions.read_file_result, &values);
            let results = b.func.dfg.inst_results(call).to_vec();
            drop_temporary(b, env.print_functions, value);
            Ok(results)
        }
        IrCallTarget::System(operation) => {
            let mut borrowed = Vec::new();
            let mut values = Vec::new();
            for argument in arguments {
                let value = emit_expr(b, module, argument, env, seal_state)?;
                values.extend(flatten_value(value.clone()));
                borrowed.push(value);
            }
            let call = b
                .ins()
                .call(env.print_functions.system[operation as usize], &values);
            let results = b.func.dfg.inst_results(call).to_vec();
            for value in borrowed.into_iter().rev() {
                drop_temporary(b, env.print_functions, value);
            }
            Ok(results)
        }
        IrCallTarget::ArgumentCount => {
            let inst = b.ins().call(env.print_functions.args_count, &[]);
            Ok(b.func.dfg.inst_results(inst).to_vec())
        }
        IrCallTarget::Vec(operation, elem_id) => {
            emit_vec_call(b, module, operation, elem_id, arguments, env, seal_state)
        }
        IrCallTarget::VecSlice(_) => {
            if arguments.len() != 3 {
                return Err("internal error: Vec slice call has an invalid argument count".into());
            }
            let vector = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let CompiledValue::Vec { ptr, .. } = vector else {
                return Err("internal error: Vec slice receiver is not a Vec".into());
            };
            let start = emit_expr(b, module, &arguments[1], env, seal_state)?;
            let end = emit_expr(b, module, &arguments[2], env, seal_state)?;
            let CompiledValue::Integer(start, Type::U64) = start else {
                return Err("internal error: Vec slice start is not u64".into());
            };
            let CompiledValue::Integer(end, Type::U64) = end else {
                return Err("internal error: Vec slice end is not u64".into());
            };
            let length_slot =
                b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
            let length_pointer =
                b.ins()
                    .stack_addr(module.target_config().pointer_type(), length_slot, 0);
            let call = b.ins().call(
                env.print_functions.vec_slice,
                &[ptr, start, end, length_pointer],
            );
            let slice_pointer = b.func.dfg.inst_results(call)[0];
            let slice_length = b.ins().stack_load(
                module.target_config().pointer_type(),
                types::I64,
                length_slot,
                0,
            );
            Ok(vec![slice_pointer, slice_length])
        }
        IrCallTarget::SliceLen => {
            let Some(argument) = arguments.first() else {
                return Err("internal error: slice length call is missing its value".into());
            };
            let CompiledValue::Slice { len, .. } = emit_expr(b, module, argument, env, seal_state)?
            else {
                return Err("internal error: slice length receiver is not a slice".into());
            };
            Ok(vec![len])
        }
        IrCallTarget::Map(operation, map_id) => {
            emit_map_call(b, module, operation, map_id, arguments, env, seal_state)
        }
        IrCallTarget::Set(operation, map_id) => {
            emit_map_call(b, module, operation, map_id, arguments, env, seal_state)
        }
        IrCallTarget::EnumNew { enum_id, tag } => {
            let definition = env
                .enums
                .get(enum_id)
                .ok_or("internal error: enum type is out of range")?;
            let variant = definition
                .variants
                .get(tag)
                .ok_or("internal error: enum variant is out of range")?;
            if arguments.len() != variant.fields.len() {
                return Err(
                    "internal error: enum constructor field count differs from checked IR".into(),
                );
            }
            let pointer_type = module.target_config().pointer_type();
            let flattened_types = variant
                .fields
                .iter()
                .flat_map(|ty| enum_payload_types(*ty, env.structs))
                .collect::<Vec<_>>();
            let payload_len = flattened_types.len() * 8;
            let payload_ptr = if payload_len == 0 {
                b.ins().iconst(pointer_type, 0)
            } else {
                let slot = b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    payload_len as u32,
                    3,
                ));
                let payload = b.ins().stack_addr(pointer_type, slot, 0);
                let mut words = Vec::with_capacity(flattened_types.len());
                for argument in arguments {
                    let value = emit_expr(b, module, argument, env, seal_state)?;
                    words.extend(flatten_value(value));
                }
                if words.len() != flattened_types.len() {
                    return Err(
                        "internal error: enum payload width differs from checked layout".into(),
                    );
                }
                for (index, (word, ty)) in words.into_iter().zip(flattened_types).enumerate() {
                    let encoded = encode_enum_word(b, word, ty, pointer_type)?;
                    b.ins()
                        .store(MemFlagsData::new(), encoded, payload, (index * 8) as i32);
                }
                payload
            };
            let drops = env.enum_drop_plans.get(enum_id).copied().flatten();
            let (drop_ptr, drop_len) = if let Some(data_id) = drops {
                let data = module.declare_data_in_func(data_id, b.func);
                (
                    b.ins().symbol_value(pointer_type, data),
                    b.ins().iconst(
                        types::I64,
                        enum_drop_plan_len(definition, env.structs) as i64,
                    ),
                )
            } else {
                (
                    b.ins().iconst(pointer_type, 0),
                    b.ins().iconst(types::I64, 0),
                )
            };
            let tag_value = b.ins().iconst(types::I64, tag as i64);
            let payload_length = b.ins().iconst(types::I64, payload_len as i64);
            let call = b.ins().call(
                env.print_functions.enum_new,
                &[tag_value, payload_ptr, payload_length, drop_ptr, drop_len],
            );
            Ok(b.func.dfg.inst_results(call).to_vec())
        }
        IrCallTarget::EnumPredicate(predicate, enum_id) => {
            let argument = arguments
                .first()
                .ok_or("internal error: enum predicate receiver missing")?;
            let value = emit_expr(b, module, argument, env, seal_state)?;
            let values = flatten_value(value.clone());
            let definition = env
                .enums
                .get(enum_id)
                .ok_or("internal error: enum predicate type is out of range")?;
            let variant = match predicate {
                EnumPredicate::IsSome => "Some",
                EnumPredicate::IsOk => "Ok",
                EnumPredicate::IsNone => "None",
                EnumPredicate::IsErr => "Err",
            };
            let tag = definition
                .variants
                .iter()
                .position(|item| item.name == variant)
                .ok_or("internal error: enum predicate variant is missing")?;
            let call = b.ins().call(env.print_functions.enum_tag, &[values[0]]);
            let actual = b.func.dfg.inst_results(call)[0];
            let expected = b.ins().iconst(types::I64, tag as i64);
            let result = b.ins().icmp(IntCC::Equal, actual, expected);
            drop_temporary(b, env.print_functions, value);
            Ok(vec![result])
        }
        IrCallTarget::EnumUnwrapOr {
            enum_id,
            value_type,
        } => {
            let receiver_expr = arguments
                .first()
                .ok_or("internal error: Option receiver missing")?;
            let default_expr = arguments
                .get(1)
                .ok_or("internal error: Option default missing")?;
            let receiver = emit_expr(b, module, receiver_expr, env, seal_state)?;
            let default = emit_expr(b, module, default_expr, env, seal_state)?;
            let pointer_type = module.target_config().pointer_type();
            let receiver_values = flatten_value(receiver.clone());
            let tag_call = b
                .ins()
                .call(env.print_functions.enum_tag, &[receiver_values[0]]);
            let tag_value = b.func.dfg.inst_results(tag_call)[0];
            let success_variant = if env
                .enums
                .get(enum_id)
                .is_some_and(|definition| definition.name.starts_with("$RynResult#"))
            {
                "Ok"
            } else {
                "Some"
            };
            let success_tag = env
                .enums
                .get(enum_id)
                .and_then(|definition| {
                    definition
                        .variants
                        .iter()
                        .position(|variant| variant.name == success_variant)
                })
                .ok_or("internal error: Option/Result success variant is missing")?;
            let expected_tag = b.ins().iconst(types::I64, success_tag as i64);
            let is_success = b.ins().icmp(IntCC::Equal, tag_value, expected_tag);
            let success_block = b.create_block();
            let fallback_block = b.create_block();
            let merge_block = b.create_block();
            for ty in clif_types(value_type, pointer_type, env.structs) {
                b.append_block_param(merge_block, ty);
            }
            let from = b
                .current_block()
                .ok_or("internal error: Option/Result.unwrap_or has no source block")?;
            b.ins()
                .brif(is_success, success_block, &[], fallback_block, &[]);
            seal_ready(b, from, seal_state);

            b.switch_to_block(success_block);
            let value = decode_enum_value_payload(
                b,
                receiver_values[0],
                0,
                value_type,
                pointer_type,
                env.structs,
                env.print_functions,
                false,
            )?;
            let value = clone_enum_payload(b, env.print_functions, value_type, value)?;
            drop_temporary(b, env.print_functions, default.clone());
            drop_temporary(b, env.print_functions, receiver.clone());
            let from = b
                .current_block()
                .ok_or("internal error: Option/Result.unwrap_or success block missing")?;
            let args = flatten_value(value)
                .into_iter()
                .map(BlockArg::Value)
                .collect::<Vec<_>>();
            b.ins().jump(merge_block, &args);
            seal_ready(b, from, seal_state);

            b.switch_to_block(fallback_block);
            drop_temporary(b, env.print_functions, receiver);
            let args = flatten_value(default)
                .into_iter()
                .map(BlockArg::Value)
                .collect::<Vec<_>>();
            b.ins().jump(merge_block, &args);
            let from = b
                .current_block()
                .ok_or("internal error: Option/Result.unwrap_or fallback block missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(merge_block);
            seal_ready(b, merge_block, seal_state);
            let mut offset = 0;
            let result = compiled_value_from_type(
                value_type,
                b.block_params(merge_block),
                &mut offset,
                env.structs,
            )?;
            Ok(flatten_value(result))
        }
        IrCallTarget::EnumUnwrap {
            enum_id: _,
            value_type,
            message,
            success_tag,
            failure,
        } => {
            let receiver_expr = arguments
                .first()
                .ok_or("internal error: Option receiver missing")?;
            let receiver = emit_expr(b, module, receiver_expr, env, seal_state)?;
            let expect_message = if message {
                Some(emit_expr(b, module, &arguments[1], env, seal_state)?)
            } else {
                None
            };
            let pointer_type = module.target_config().pointer_type();
            let receiver_values = flatten_value(receiver.clone());
            let tag_call = b
                .ins()
                .call(env.print_functions.enum_tag, &[receiver_values[0]]);
            let tag_value = b.func.dfg.inst_results(tag_call)[0];
            let expected_tag = b.ins().iconst(types::I64, success_tag as i64);
            let is_some = b.ins().icmp(IntCC::Equal, tag_value, expected_tag);
            let some_block = b.create_block();
            let none_block = b.create_block();
            let merge_block = b.create_block();
            for ty in clif_types(value_type, pointer_type, env.structs) {
                b.append_block_param(merge_block, ty);
            }
            let from = b
                .current_block()
                .ok_or("internal error: Option.unwrap has no source block")?;
            b.ins().brif(is_some, some_block, &[], none_block, &[]);
            seal_ready(b, from, seal_state);

            b.switch_to_block(some_block);
            let value = decode_enum_value_payload(
                b,
                receiver_values[0],
                0,
                value_type,
                pointer_type,
                env.structs,
                env.print_functions,
                false,
            )?;
            let value = clone_enum_payload(b, env.print_functions, value_type, value)?;
            drop_temporary(b, env.print_functions, receiver.clone());
            if let Some(message) = expect_message.clone() {
                drop_temporary(b, env.print_functions, message);
            }
            let args = flatten_value(value)
                .into_iter()
                .map(BlockArg::Value)
                .collect::<Vec<_>>();
            b.ins().jump(merge_block, &args);
            let from = b
                .current_block()
                .ok_or("internal error: Option.unwrap Some block missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(none_block);
            if let Some(message) = expect_message {
                let values = flatten_value(message);
                b.ins()
                    .call(env.print_functions.system[failure as usize], &values);
            } else {
                b.ins()
                    .call(env.print_functions.system[failure as usize], &[]);
            }
            b.ins().trap(TrapCode::unwrap_user(1));
            let from = b
                .current_block()
                .ok_or("internal error: Option.unwrap None block missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(merge_block);
            seal_ready(b, merge_block, seal_state);
            let mut offset = 0;
            let result = compiled_value_from_type(
                value_type,
                b.block_params(merge_block),
                &mut offset,
                env.structs,
            )?;
            Ok(flatten_value(result))
        }
        IrCallTarget::VecGetOption {
            elem_id,
            option_id,
            pop,
        } => {
            let elem = vec_elem(elem_id);
            let pointer_type = module.target_config().pointer_type();
            let receiver = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let receiver_values = flatten_value(receiver.clone());
            let output = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                (value_width(elem, env.structs) * 8) as u32,
                3,
            ));
            let output_ptr = b.ins().stack_addr(pointer_type, output, 0);
            let get = if pop {
                b.ins().call(
                    env.print_functions.owned_vecs[VecOp::PopOption as usize],
                    &[receiver_values[0], output_ptr],
                )
            } else {
                let index = emit_expr(b, module, &arguments[1], env, seal_state)?;
                let index_values = flatten_value(index);
                b.ins().call(
                    env.print_functions.owned_vecs[VecOp::GetOption as usize],
                    &[receiver_values[0], index_values[0], output_ptr],
                )
            };
            let found = b.func.dfg.inst_results(get)[0];
            drop_temporary(b, env.print_functions, receiver);
            let option = env
                .enums
                .get(option_id)
                .ok_or("internal error: Vec Option type is out of range")?;
            let some_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "Some")
                .ok_or("internal error: Vec Option lacks Some")?;
            let none_tag = option
                .variants
                .iter()
                .position(|variant| variant.name == "None")
                .ok_or("internal error: Vec Option lacks None")?;
            let some_block = b.create_block();
            let none_block = b.create_block();
            let merge_block = b.create_block();
            b.append_block_param(merge_block, pointer_type);
            let one = b.ins().iconst(types::I8, 1);
            let is_found = b.ins().icmp(IntCC::Equal, found, one);
            let from = b
                .current_block()
                .ok_or("internal error: Vec.get has no source block")?;
            b.ins().brif(is_found, some_block, &[], none_block, &[]);
            seal_ready(b, from, seal_state);

            b.switch_to_block(some_block);
            let tag = b.ins().iconst(types::I64, some_tag as i64);
            let payload_len = b
                .ins()
                .iconst(types::I64, (value_width(elem, env.structs) * 8) as i64);
            let drop_plan = if let Some(data_id) = env.enum_drop_plans[option_id] {
                let data = module.declare_data_in_func(data_id, b.func);
                b.ins().symbol_value(pointer_type, data)
            } else {
                b.ins().iconst(pointer_type, 0)
            };
            let drop_len = b
                .ins()
                .iconst(types::I64, enum_drop_plan_len(option, env.structs) as i64);
            let some = b.ins().call(
                env.print_functions.enum_new,
                &[tag, output_ptr, payload_len, drop_plan, drop_len],
            );
            let some_value = b.func.dfg.inst_results(some)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(some_value)]);
            let from = b
                .current_block()
                .ok_or("internal error: Vec.get Some block missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(none_block);
            let tag = b.ins().iconst(types::I64, none_tag as i64);
            let null = b.ins().iconst(pointer_type, 0);
            let zero = b.ins().iconst(types::I64, 0);
            let none_drop_plan = if let Some(data_id) = env.enum_drop_plans[option_id] {
                let data = module.declare_data_in_func(data_id, b.func);
                b.ins().symbol_value(pointer_type, data)
            } else {
                b.ins().iconst(pointer_type, 0)
            };
            let none_drop_len = b
                .ins()
                .iconst(types::I64, enum_drop_plan_len(option, env.structs) as i64);
            let none = b.ins().call(
                env.print_functions.enum_new,
                &[tag, null, zero, none_drop_plan, none_drop_len],
            );
            let none_value = b.func.dfg.inst_results(none)[0];
            b.ins().jump(merge_block, &[BlockArg::Value(none_value)]);
            let from = b
                .current_block()
                .ok_or("internal error: Vec.get None block missing")?;
            seal_ready(b, from, seal_state);

            b.switch_to_block(merge_block);
            seal_ready(b, merge_block, seal_state);
            Ok(vec![b.block_params(merge_block)[0]])
        }
        IrCallTarget::Argument => {
            let [argument] = arguments else {
                return Err("internal error: `arg` call is missing its index".into());
            };
            let values = flatten_value(emit_expr(b, module, argument, env, seal_state)?);
            let [index] = values.as_slice() else {
                return Err("internal error: `arg` index has an invalid representation".into());
            };
            let index = *index;
            let pointer_call = b.ins().call(env.print_functions.arg_pointer, &[index]);
            let pointer = b.func.dfg.inst_results(pointer_call)[0];
            let length_call = b.ins().call(env.print_functions.arg_length, &[index]);
            let length = b.func.dfg.inst_results(length_call)[0];
            Ok(vec![pointer, length])
        }
        IrCallTarget::Function(function_index) => {
            emit_function_call(b, module, function_index, arguments, env, seal_state)
        }
        IrCallTarget::IndirectFunctionPointer(signature_id) => {
            let signature = crate::sema::function_pointer_info(signature_id);
            if arguments.len() != signature.parameters.len() + 1 {
                return Err("internal error: indirect function argument count mismatch".into());
            }
            let callee = emit_expr(b, module, &arguments[0], env, seal_state)?;
            let CompiledValue::Integer(callee, Type::FunctionPointer(_)) = callee else {
                return Err("internal error: indirect callee is not a function pointer".into());
            };
            let mut call_signature = Signature::new(module.isa().default_call_conv());
            let mut values = Vec::with_capacity(signature.parameters.len());
            for (argument, ty) in arguments[1..].iter().zip(&signature.parameters) {
                let value = emit_expr(b, module, argument, env, seal_state)?;
                if signature.extern_c
                    && let Some((size, fields)) = c_abi_packed_record_layout(*ty, env.structs)
                {
                    for abi in c_abi_param_types(size, &fields) {
                        call_signature.params.push(AbiParam::new(abi));
                    }
                    values.extend(pack_c_abi_record(b, &flatten_value(value), size, &fields)?);
                } else {
                    call_signature.params.push(AbiParam::new(clif_scalar_type(
                        *ty,
                        module.target_config().pointer_type(),
                    )?));
                    values.extend(flatten_value(value));
                }
            }
            if let Some(result) = signature.result {
                if signature.extern_c
                    && let Some((size, fields)) = c_abi_packed_record_layout(result, env.structs)
                {
                    for abi in c_abi_param_types(size, &fields) {
                        call_signature.returns.push(AbiParam::new(abi));
                    }
                } else {
                    call_signature.returns.push(AbiParam::new(clif_scalar_type(
                        result,
                        module.target_config().pointer_type(),
                    )?));
                }
            }
            let signature_ref = b.import_signature(call_signature);
            let call = b.ins().call_indirect(signature_ref, callee, &values);
            sync_mutably_addressed_arguments(
                b,
                &arguments[1..],
                env,
                module.target_config().pointer_type(),
            )?;
            let results = b.func.dfg.inst_results(call).to_vec();
            if signature.extern_c
                && let Some(result) = signature.result
                && let Some((size, fields)) = c_abi_packed_record_layout(result, env.structs)
            {
                if results.len() != c_abi_param_types(size, &fields).len() {
                    return Err(
                        "internal error: indirect C aggregate return has the wrong ABI value count"
                            .into(),
                    );
                }
                Ok(unpack_c_abi_record(b, &results, size, &fields))
            } else {
                Ok(results)
            }
        }
    }
}

fn sync_mutably_addressed_arguments(
    b: &mut FunctionBuilder<'_>,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    pointer_type: types::Type,
) -> Result<(), String> {
    for argument in arguments {
        let IrExpression::AddressOf {
            slot,
            ty,
            pointer_type: Type::Reference(_, true) | Type::RawPointer(_),
            ..
        } = argument
        else {
            continue;
        };
        if let Some(Some(stack_slot)) = env.address_slots.get(*slot) {
            sync_stack_slot_to_variables(
                b,
                *slot,
                *ty,
                *stack_slot,
                0,
                pointer_type,
                env.structs,
                matches!(*ty, Type::Struct(id) if env.structs[id].repr_c),
            )?;
        }
    }
    Ok(())
}

fn emit_function_call(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    function_index: usize,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Vec<Value>, String> {
    let callee = *env
        .calls
        .get(function_index)
        .ok_or_else(|| "internal error: missing function reference".to_string())?;
    let mut args = Vec::new();
    let return_type = *env
        .function_returns
        .get(function_index)
        .ok_or_else(|| "internal error: missing function return type".to_string())?;
    let direct_c_record = env
        .external_functions
        .get(function_index)
        .copied()
        .unwrap_or(false)
        && return_type.is_some_and(|ty| is_direct_c_abi_record(ty, env.structs));
    let return_slot = if let Some(ty @ (Type::Struct(_) | Type::Array(_))) = return_type
        && !direct_c_record
    {
        let slot = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (value_width(ty, env.structs) * 8) as u32,
            3,
        ));
        let pointer = b
            .ins()
            .stack_addr(module.target_config().pointer_type(), slot, 0);
        args.push(pointer);
        Some((slot, ty))
    } else {
        None
    };
    let external = env
        .external_functions
        .get(function_index)
        .copied()
        .unwrap_or(false);
    let parameter_types = env
        .function_parameters
        .get(function_index)
        .ok_or_else(|| "internal error: missing function parameter types".to_string())?;
    // A `str` argument may borrow a temporary String; release it after the call.
    let mut borrowed_temporaries = Vec::new();
    for (index, arg) in arguments.iter().enumerate() {
        let value = emit_expr(b, module, arg, env, seal_state)?;
        if matches!(
            value,
            CompiledValue::StrViewOwned {
                temporary: true,
                ..
            }
        ) {
            borrowed_temporaries.push(value.clone());
        }
        let flattened = flatten_value(value);
        if external
            && let Some((size, fields)) = parameter_types
                .get(index)
                .and_then(|ty| c_abi_packed_record_layout(*ty, env.structs))
        {
            args.extend(pack_c_abi_record(b, &flattened, size, &fields)?);
        } else {
            args.extend(flattened);
        }
    }
    let inst = b.ins().call(callee, &args);
    for value in borrowed_temporaries {
        drop_temporary(b, env.print_functions, value);
    }
    sync_mutably_addressed_arguments(b, arguments, env, module.target_config().pointer_type())?;
    let results = b.func.dfg.inst_results(inst).to_vec();
    if let Some((slot, ty)) = return_slot {
        let pointer = b
            .ins()
            .stack_addr(module.target_config().pointer_type(), slot, 0);
        let mut values = Vec::new();
        let mut components = Vec::new();
        append_sret_components(
            ty,
            0,
            module.target_config().pointer_type(),
            env.structs,
            &mut components,
        )?;
        for (clif_ty, slot_offset) in components {
            values.push(b.ins().load(
                clif_ty,
                MemFlagsData::new(),
                pointer,
                (slot_offset * 8) as i32,
            ));
        }
        let mut offset = 0;
        flatten_value(compiled_value_from_type(
            ty,
            &values,
            &mut offset,
            env.structs,
        )?)
        .into_iter()
        .map(Ok)
        .collect()
    } else if external
        && let Some((size, fields)) =
            return_type.and_then(|ty| c_abi_packed_record_layout(ty, env.structs))
    {
        if results.len() != c_abi_param_types(size, &fields).len() {
            return Err(
                "internal error: packed C aggregate return has the wrong ABI value count".into(),
            );
        }
        Ok(unpack_c_abi_record(b, &results, size, &fields))
    } else {
        Ok(results)
    }
}

fn clif_scalar_type(ty: Type, pointer_type: types::Type) -> Result<types::Type, String> {
    Ok(match ty {
        Type::I8 | Type::U8 | Type::Bool => types::I8,
        Type::I16 | Type::U16 => types::I16,
        Type::I32 | Type::U32 | Type::Char => types::I32,
        Type::I64 | Type::U64 => types::I64,
        Type::F32 => types::F32,
        Type::F64 => types::F64,
        Type::Str
        | Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Reference(_, _)
        | Type::RawPointer(_)
        | Type::FunctionPointer(_) => pointer_type,
        Type::Slice(_) => {
            return Err("internal error: a borrowed slice cannot be returned as a scalar".into());
        }
        Type::Struct(_) => return Err("internal error: nested structure in return layout".into()),
        Type::Array(_) => return Err("internal error: array in scalar return layout".into()),
    })
}

fn store_aggregate_return(
    b: &mut FunctionBuilder<'_>,
    ty: Type,
    value: CompiledValue,
    pointer: Value,
    structs: &[RynStruct],
) -> Result<(), String> {
    store_sret_value(b, ty, value, 0, pointer, structs)
}

fn append_sret_components(
    ty: Type,
    slot_offset: usize,
    pointer_type: types::Type,
    structs: &[RynStruct],
    components: &mut Vec<(types::Type, usize)>,
) -> Result<(), String> {
    match ty {
        Type::Struct(struct_id) => {
            for field in &structs[struct_id].fields {
                append_sret_components(
                    field.ty,
                    slot_offset + field.slot_offset,
                    pointer_type,
                    structs,
                    components,
                )?;
            }
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            for index in 0..length {
                append_sret_components(
                    element,
                    slot_offset + index * storage_slot_width(element, structs),
                    pointer_type,
                    structs,
                    components,
                )?;
            }
        }
        Type::Str => {
            components.push((pointer_type, slot_offset));
            components.push((types::I64, slot_offset + 1));
        }
        scalar => components.push((clif_scalar_type(scalar, pointer_type)?, slot_offset)),
    }
    Ok(())
}

fn store_sret_value(
    b: &mut FunctionBuilder<'_>,
    ty: Type,
    value: CompiledValue,
    slot_offset: usize,
    pointer: Value,
    structs: &[RynStruct],
) -> Result<(), String> {
    match (ty, value) {
        (
            Type::Struct(struct_id),
            CompiledValue::Struct {
                struct_id: actual,
                fields,
            },
        ) if struct_id == actual && fields.len() == structs[struct_id].fields.len() => {
            for (field, value) in structs[struct_id].fields.iter().zip(fields) {
                store_sret_value(
                    b,
                    field.ty,
                    value,
                    slot_offset + field.slot_offset,
                    pointer,
                    structs,
                )?;
            }
        }
        (Type::Array(id), CompiledValue::Array(values)) => {
            let (element, length) = array_info(id);
            if values.len() != length {
                return Err("internal error: returned array has the wrong length".into());
            }
            for (index, value) in values.into_iter().enumerate() {
                store_sret_value(
                    b,
                    element,
                    value,
                    slot_offset + index * storage_slot_width(element, structs),
                    pointer,
                    structs,
                )?;
            }
        }
        (Type::Str, CompiledValue::Str { ptr, len }) => {
            b.ins()
                .store(MemFlagsData::new(), ptr, pointer, (slot_offset * 8) as i32);
            b.ins().store(
                MemFlagsData::new(),
                len,
                pointer,
                ((slot_offset + 1) * 8) as i32,
            );
        }
        (scalar, value) if !matches!(scalar, Type::Struct(_) | Type::Str) => {
            let mut values = flatten_value(value);
            if values.len() != 1 {
                return Err(
                    "internal error: scalar structure field has an incompatible value".into(),
                );
            }
            b.ins().store(
                MemFlagsData::new(),
                values.remove(0),
                pointer,
                (slot_offset * 8) as i32,
            );
        }
        _ => {
            return Err("internal error: structure return field has an incompatible layout".into());
        }
    }
    Ok(())
}

fn emit_if_expression(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    expression: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<CompiledValue, String> {
    let IrExpression::If {
        condition,
        then_value,
        else_value,
        ty,
    } = expression
    else {
        return Err("internal error: non-conditional IR reached if-expression codegen".into());
    };
    let ty = *ty;
    let condition = as_bool(emit_expr(b, module, condition, env, seal_state)?)?;
    let then_block = b.create_block();
    let else_block = b.create_block();
    let merge_block = b.create_block();
    let pointer_type = module.target_config().pointer_type();
    let result_types = clif_types(ty, pointer_type, env.structs);
    for result_type in result_types {
        b.append_block_param(merge_block, result_type);
    }
    let from = b
        .current_block()
        .ok_or_else(|| "internal error: missing if-expression source block".to_string())?;
    b.ins().brif(condition, then_block, &[], else_block, &[]);
    seal_ready(b, from, seal_state);

    b.switch_to_block(then_block);
    let then_result = emit_expr(b, module, then_value, env, seal_state)?;
    if !compiled_value_matches_type(&then_result, ty) {
        return Err("internal error: if-expression then branch has the wrong type".into());
    }
    let then_args = flatten_value(then_result)
        .into_iter()
        .map(BlockArg::Value)
        .collect::<Vec<_>>();
    b.ins().jump(merge_block, &then_args);
    seal_ready(b, then_block, seal_state);

    b.switch_to_block(else_block);
    let else_result = emit_expr(b, module, else_value, env, seal_state)?;
    if !compiled_value_matches_type(&else_result, ty) {
        return Err("internal error: if-expression else branch has the wrong type".into());
    }
    let else_args = flatten_value(else_result)
        .into_iter()
        .map(BlockArg::Value)
        .collect::<Vec<_>>();
    b.ins().jump(merge_block, &else_args);
    seal_ready(b, else_block, seal_state);

    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    let mut offset = 0;
    compiled_value_from_type(ty, b.block_params(merge_block), &mut offset, env.structs)
}

/// A `choose` payload binding that is borrowed (`x.method()` takes `&x`) needs
/// its stack home refreshed, exactly like `let` and assignment do; otherwise the
/// borrow reads an unwritten stack slot.
fn sync_choose_binding_to_address_slot(
    b: &mut FunctionBuilder<'_>,
    env: &ExprEnv<'_>,
    slot: usize,
    ty: Type,
    pointer_type: types::Type,
) -> Result<(), String> {
    if let Some(Some(stack_slot)) = env.address_slots.get(slot) {
        store_local_variables_to_stack_slot(
            b,
            slot,
            ty,
            *stack_slot,
            0,
            pointer_type,
            env.structs,
            matches!(ty, Type::Struct(id) if env.structs[id].repr_c),
        )?;
    }
    Ok(())
}

fn emit_enum_match(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    expression: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<CompiledValue, String> {
    let IrExpression::EnumMatch {
        value,
        enum_id,
        arms,
        ty,
    } = expression
    else {
        return Err("internal error: non-match IR reached enum code generation".into());
    };
    let definition = env
        .enums
        .get(*enum_id)
        .ok_or("internal error: enum type is out of range")?;
    let value = emit_expr(b, module, value, env, seal_state)?;
    let CompiledValue::Enum { ptr, temporary } = value else {
        return Err("internal error: choose scrutinee is not an enum handle".into());
    };
    let tag_call = b.ins().call(env.print_functions.enum_tag, &[ptr]);
    let tag = b.func.dfg.inst_results(tag_call)[0];

    let wildcard = arms.iter().position(|arm| arm.variant.is_none());
    let mut selected = Vec::with_capacity(definition.variants.len());
    for (variant, _) in definition.variants.iter().enumerate() {
        let arm_index = arms
            .iter()
            .position(|arm| arm.variant == Some(variant))
            .or(wildcard)
            .ok_or("internal error: non-exhaustive choose reached code generation")?;
        selected.push((variant, arm_index));
    }
    if selected.is_empty() {
        return Err("internal error: empty enum reached choose code generation".into());
    }
    let mut arm_blocks = vec![None; arms.len()];
    for (_, arm_index) in &selected {
        if arm_blocks[*arm_index].is_none() {
            arm_blocks[*arm_index] = Some(b.create_block());
        }
    }
    let merge_block = b.create_block();
    let pointer_type = module.target_config().pointer_type();
    for result_type in clif_types(*ty, pointer_type, env.structs) {
        b.append_block_param(merge_block, result_type);
    }

    let mut test_block = b
        .current_block()
        .ok_or("internal error: choose has no source block")?;
    for (position, (variant, arm_index)) in selected.iter().enumerate() {
        if position > 0 {
            b.switch_to_block(test_block);
        }
        let target = arm_blocks[*arm_index].expect("selected choose arm has a block");
        if position + 1 == selected.len() {
            b.ins().jump(target, &[]);
            seal_ready(b, test_block, seal_state);
        } else {
            let next_test = b.create_block();
            let expected = b.ins().iconst(types::I64, *variant as i64);
            let matches = b.ins().icmp(IntCC::Equal, tag, expected);
            b.ins().brif(matches, target, &[], next_test, &[]);
            seal_ready(b, test_block, seal_state);
            test_block = next_test;
        }
    }

    for (arm_index, arm) in arms.iter().enumerate() {
        let Some(arm_block) = arm_blocks[arm_index] else {
            continue;
        };
        b.switch_to_block(arm_block);
        if let Some(variant_index) = arm.variant {
            let variant = definition
                .variants
                .get(variant_index)
                .ok_or("internal error: choose variant is out of range")?;
            if arm.bindings.len() != variant.fields.len() {
                return Err("internal error: choose binding count differs from checked IR".into());
            }
            for binding in &arm.bindings {
                let field_type = *variant
                    .fields
                    .get(binding.field_index)
                    .ok_or("internal error: choose field index is out of range")?;
                if field_type != binding.ty {
                    return Err(
                        "internal error: choose binding type differs from checked IR".into(),
                    );
                }
                let payload_word_offset = variant.fields[..binding.field_index]
                    .iter()
                    .map(|ty| value_width(*ty, env.structs))
                    .sum::<usize>();
                if let Type::Struct(struct_id) = field_type {
                    let local_value = decode_enum_struct_payload(
                        b,
                        ptr,
                        payload_word_offset,
                        struct_id,
                        pointer_type,
                        env.structs,
                        env.print_functions,
                        true,
                    )?;
                    store_local(b, binding.slot, field_type, local_value, env.structs)?;
                    sync_choose_binding_to_address_slot(
                        b,
                        env,
                        binding.slot,
                        field_type,
                        pointer_type,
                    )?;
                    continue;
                }
                let word_index = b.ins().iconst(types::I64, payload_word_offset as i64);
                let local_value = match field_type {
                    Type::OwnedString
                    | Type::Vec(_)
                    | Type::Enum(_)
                    | Type::Map(_)
                    | Type::Set(_) => {
                        let get = b
                            .ins()
                            .call(env.print_functions.enum_word, &[ptr, word_index]);
                        let raw = b.func.dfg.inst_results(get)[0];
                        let field = if pointer_type == types::I64 {
                            raw
                        } else {
                            b.ins().ireduce(pointer_type, raw)
                        };
                        b.ins()
                            .call(env.print_functions.enum_clear_word, &[ptr, word_index]);
                        field
                    }
                    other => {
                        let get = b
                            .ins()
                            .call(env.print_functions.enum_word, &[ptr, word_index]);
                        let raw = b.func.dfg.inst_results(get)[0];
                        match other {
                            Type::F32 => {
                                let bits = b.ins().ireduce(types::I32, raw);
                                b.ins().bitcast(types::F32, MemFlagsData::new(), bits)
                            }
                            Type::F64 => b.ins().bitcast(types::F64, MemFlagsData::new(), raw),
                            Type::I8
                            | Type::I16
                            | Type::I32
                            | Type::U8
                            | Type::U16
                            | Type::U32
                            | Type::Bool
                            | Type::Char => {
                                b.ins().ireduce(clif_scalar_type(other, pointer_type)?, raw)
                            }
                            Type::I64 | Type::U64 => raw,
                            _ => {
                                return Err("internal error: unsupported enum payload field".into());
                            }
                        }
                    }
                };
                b.def_var(Variable::from_u32(binding.slot as u32), local_value);
                sync_choose_binding_to_address_slot(
                    b,
                    env,
                    binding.slot,
                    field_type,
                    pointer_type,
                )?;
            }
        }
        let result = emit_expr(b, module, &arm.body, env, seal_state)?;
        if !compiled_value_matches_type(&result, *ty) {
            return Err("internal error: choose arm has the wrong result type".into());
        }
        for binding in arm.bindings.iter().rev() {
            drop_binding(b, env, binding.slot, binding.ty, seal_state);
        }
        if temporary {
            drop_temporary(
                b,
                env.print_functions,
                CompiledValue::Enum {
                    ptr,
                    temporary: true,
                },
            );
        }
        let args = flatten_value(result)
            .into_iter()
            .map(BlockArg::Value)
            .collect::<Vec<_>>();
        let from = b
            .current_block()
            .ok_or("internal error: choose arm has no block")?;
        b.ins().jump(merge_block, &args);
        seal_ready(b, from, seal_state);
    }
    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    let mut offset = 0;
    compiled_value_from_type(*ty, b.block_params(merge_block), &mut offset, env.structs)
}

fn emit_propagate(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    expression: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<CompiledValue, String> {
    let IrExpression::Propagate {
        value,
        input_enum,
        output_enum,
        success_type,
        failure_type,
        success_variant,
        failure_variant,
        output_failure_variant,
        deferred,
    } = expression
    else {
        return Err(
            "internal error: non-propagation IR reached propagation code generation".into(),
        );
    };
    if env.function_return != Some(Type::Enum(*output_enum)) {
        return Err(
            "internal error: propagated Option/Result does not match the function return type"
                .into(),
        );
    }
    let input_definition = env
        .enums
        .get(*input_enum)
        .ok_or("internal error: input Option/Result type is out of range")?;
    let output_definition = env
        .enums
        .get(*output_enum)
        .ok_or("internal error: output Option/Result type is out of range")?;
    let input_value = emit_expr(b, module, value, env, seal_state)?;
    let CompiledValue::Enum { ptr, .. } = input_value else {
        return Err("internal error: `?` operand is not an enum handle".into());
    };
    let tag_call = b.ins().call(env.print_functions.enum_tag, &[ptr]);
    let tag = b.func.dfg.inst_results(tag_call)[0];
    let success_block = b.create_block();
    let error_block = b.create_block();
    let merge_block = b.create_block();
    let pointer_type = module.target_config().pointer_type();
    for ty in clif_types(*success_type, pointer_type, env.structs) {
        b.append_block_param(merge_block, ty);
    }
    let success_tag = b.ins().iconst(types::I64, *success_variant as i64);
    let is_success = b.ins().icmp(IntCC::Equal, tag, success_tag);
    let source = b
        .current_block()
        .ok_or("internal error: `?` has no source block")?;
    b.ins()
        .brif(is_success, success_block, &[], error_block, &[]);
    seal_ready(b, source, seal_state);

    b.switch_to_block(success_block);
    let success_definition = input_definition
        .variants
        .get(*success_variant)
        .ok_or("internal error: generic success tag is out of range")?;
    let success_field = *success_definition
        .fields
        .first()
        .ok_or("internal error: generic success variant has no payload")?;
    if success_field != *success_type {
        return Err("internal error: Result `Ok` payload differs from the checked type".into());
    }
    let success_value = if let Type::Struct(struct_id) = success_field {
        decode_enum_struct_payload(
            b,
            ptr,
            0,
            struct_id,
            pointer_type,
            env.structs,
            env.print_functions,
            true,
        )?
    } else {
        let word_index = b.ins().iconst(types::I64, 0);
        let get = b
            .ins()
            .call(env.print_functions.enum_word, &[ptr, word_index]);
        let raw = b.func.dfg.inst_results(get)[0];
        let decoded = decode_enum_payload(b, raw, *success_type, pointer_type)?;
        b.ins()
            .call(env.print_functions.enum_clear_word, &[ptr, word_index]);
        decoded
    };
    b.ins().call(env.print_functions.enum_drop, &[ptr]);
    let from = b
        .current_block()
        .ok_or("internal error: missing `?` success block")?;
    let args = flatten_value(success_value)
        .into_iter()
        .map(BlockArg::Value)
        .collect::<Vec<_>>();
    b.ins().jump(merge_block, &args);
    seal_ready(b, from, seal_state);

    b.switch_to_block(error_block);
    let failure_definition = input_definition
        .variants
        .get(*failure_variant)
        .ok_or("internal error: generic failure tag is out of range")?;
    // Aggregate error payloads (structures and arrays) keep the same word
    // layout in the returned enum, so their words move across unchanged.
    let aggregate_failure = failure_type
        .filter(|ty| matches!(ty, Type::Struct(_) | Type::Array(_)))
        .map(|ty| value_width(ty, env.structs));
    let moved_failure = if let Some(words) = aggregate_failure {
        if failure_definition.fields.as_slice() != [failure_type.unwrap_or(Type::Bool)] {
            return Err(
                "internal error: generic failure payload differs from the checked type".into(),
            );
        }
        let slot = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (words.max(1) * 8) as u32,
            3,
        ));
        for word in 0..words {
            let word_index = b.ins().iconst(types::I64, word as i64);
            let get = b
                .ins()
                .call(env.print_functions.enum_word, &[ptr, word_index]);
            let raw = b.func.dfg.inst_results(get)[0];
            b.ins()
                .stack_store(types::I64, raw, slot, (word * 8) as i32);
            b.ins()
                .call(env.print_functions.enum_clear_word, &[ptr, word_index]);
        }
        Some((
            b.ins().stack_addr(pointer_type, slot, 0),
            (words * 8) as u64,
        ))
    } else {
        None
    };
    let encoded_failure = if moved_failure.is_some() {
        None
    } else if let Some(failure_type) = failure_type {
        if failure_definition.fields.as_slice() != [*failure_type] {
            return Err(
                "internal error: generic failure payload differs from the checked type".into(),
            );
        }
        let word_index = b.ins().iconst(types::I64, 0);
        let get = b
            .ins()
            .call(env.print_functions.enum_word, &[ptr, word_index]);
        let raw = b.func.dfg.inst_results(get)[0];
        let failure_value = decode_enum_payload(b, raw, *failure_type, pointer_type)?;
        let flattened = flatten_value(failure_value);
        let [failure_word] = flattened.as_slice() else {
            return Err("internal error: generic failure payload has an aggregate layout".into());
        };
        let encoded = encode_enum_word(b, *failure_word, *failure_type, pointer_type)?;
        b.ins()
            .call(env.print_functions.enum_clear_word, &[ptr, word_index]);
        Some(encoded)
    } else {
        if !failure_definition.fields.is_empty() {
            return Err("internal error: Option `None` unexpectedly has a payload".into());
        }
        None
    };
    b.ins().call(env.print_functions.enum_drop, &[ptr]);

    let output_variant = output_definition
        .variants
        .get(*output_failure_variant)
        .ok_or("internal error: return generic failure tag is out of range")?;
    if let Some(failure_type) = failure_type {
        if output_variant.fields.as_slice() != [*failure_type] {
            return Err(
                "internal error: return Result error payload differs from the checked type".into(),
            );
        }
    } else if !output_variant.fields.is_empty() {
        return Err("internal error: return Option `None` unexpectedly has a payload".into());
    }
    let (payload, payload_len) = if let Some(moved) = moved_failure {
        moved
    } else if let Some(encoded) = encoded_failure {
        let payload_slot =
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let payload = b.ins().stack_addr(pointer_type, payload_slot, 0);
        b.ins().store(MemFlagsData::new(), encoded, payload, 0);
        (payload, 8_u64)
    } else {
        (b.ins().iconst(pointer_type, 0), 0_u64)
    };
    let (drop_ptr, drop_len) =
        if let Some(data_id) = env.enum_drop_plans.get(*output_enum).copied().flatten() {
            let data = module.declare_data_in_func(data_id, b.func);
            (
                b.ins().symbol_value(pointer_type, data),
                b.ins().iconst(
                    types::I64,
                    enum_drop_plan_len(output_definition, env.structs) as i64,
                ),
            )
        } else {
            (
                b.ins().iconst(pointer_type, 0),
                b.ins().iconst(types::I64, 0),
            )
        };
    let output_tag = b.ins().iconst(types::I64, *output_failure_variant as i64);
    let payload_len = b.ins().iconst(types::I64, payload_len as i64);
    let create = b.ins().call(
        env.print_functions.enum_new,
        &[output_tag, payload, payload_len, drop_ptr, drop_len],
    );
    let output_value = b.func.dfg.inst_results(create)[0];
    // The error result is built first, then the pending `defer` blocks run, as they do before
    // any other return.
    let mut loops = Vec::new();
    emit_statements(b, module, env, deferred, seal_state, &mut loops)?;
    drop_slots(b, env, env.owned_slots, seal_state);
    b.ins().return_(&[output_value]);
    let returned = b
        .current_block()
        .ok_or("internal error: missing `?` error block")?;
    seal_ready(b, returned, seal_state);

    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    let params = b.block_params(merge_block).to_vec();
    let mut offset = 0;
    compiled_value_from_type(*success_type, &params, &mut offset, env.structs)
}

fn decode_enum_payload(
    b: &mut FunctionBuilder<'_>,
    raw: Value,
    ty: Type,
    pointer_type: types::Type,
) -> Result<CompiledValue, String> {
    Ok(match ty {
        Type::OwnedString | Type::Vec(_) | Type::Enum(_) | Type::Map(_) | Type::Set(_) => {
            let pointer = if pointer_type == types::I64 {
                raw
            } else {
                b.ins().ireduce(pointer_type, raw)
            };
            match ty {
                Type::OwnedString => CompiledValue::OwnedString {
                    ptr: pointer,
                    temporary: true,
                },
                Type::Vec(_) => CompiledValue::Vec {
                    ptr: pointer,
                    temporary: true,
                },
                Type::Enum(_) => CompiledValue::Enum {
                    ptr: pointer,
                    temporary: true,
                },
                Type::Map(_) | Type::Set(_) => CompiledValue::Map {
                    ptr: pointer,
                    temporary: true,
                },
                _ => unreachable!(),
            }
        }
        Type::RawPointer(_) | Type::Reference(_, _) | Type::FunctionPointer(_) => {
            let pointer = if pointer_type == types::I64 {
                raw
            } else {
                b.ins().ireduce(pointer_type, raw)
            };
            CompiledValue::Integer(pointer, ty)
        }
        Type::F64 => CompiledValue::F64(b.ins().bitcast(types::F64, MemFlagsData::new(), raw)),
        Type::F32 => {
            let bits = b.ins().ireduce(types::I32, raw);
            CompiledValue::F32(b.ins().bitcast(types::F32, MemFlagsData::new(), bits))
        }
        Type::I8
        | Type::I16
        | Type::I32
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::Bool
        | Type::Char => {
            let value = b.ins().ireduce(clif_scalar_type(ty, pointer_type)?, raw);
            if ty == Type::Bool {
                CompiledValue::Bool(value)
            } else {
                CompiledValue::Integer(value, ty)
            }
        }
        Type::I64 | Type::U64 => CompiledValue::Integer(raw, ty),
        _ => return Err("internal error: unsupported `?` payload layout".into()),
    })
}

fn clone_enum_payload(
    b: &mut FunctionBuilder<'_>,
    functions: PrintFunctions,
    ty: Type,
    value: CompiledValue,
) -> Result<CompiledValue, String> {
    if matches!(ty, Type::Struct(_) | Type::Array(_)) {
        return clone_array_element(b, functions, value);
    }
    let (pointer, clone_function) = match (ty, value) {
        (Type::OwnedString, CompiledValue::OwnedString { ptr, .. }) => {
            (ptr, functions.owned_strings[StringOp::Clone as usize])
        }
        (Type::Vec(_), CompiledValue::Vec { ptr, .. }) => {
            (ptr, functions.owned_vecs[VecOp::Clone as usize])
        }
        (Type::Map(_), CompiledValue::Map { ptr, .. })
        | (Type::Set(_), CompiledValue::Map { ptr, .. }) => {
            (ptr, functions.maps[MapOp::Clone as usize])
        }
        (Type::Enum(_), CompiledValue::Enum { ptr, .. }) => (ptr, functions.enum_clone),
        (_, value) => return Ok(value),
    };
    let clone = b.ins().call(clone_function, &[pointer]);
    let pointer = b.func.dfg.inst_results(clone)[0];
    Ok(match ty {
        Type::OwnedString => CompiledValue::OwnedString {
            ptr: pointer,
            temporary: true,
        },
        Type::Vec(_) => CompiledValue::Vec {
            ptr: pointer,
            temporary: true,
        },
        Type::Map(_) | Type::Set(_) => CompiledValue::Map {
            ptr: pointer,
            temporary: true,
        },
        Type::Enum(_) => CompiledValue::Enum {
            ptr: pointer,
            temporary: true,
        },
        _ => unreachable!("only pointer-backed enum payloads are cloned"),
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_enum_struct_payload(
    b: &mut FunctionBuilder<'_>,
    pointer: Value,
    word_offset: usize,
    struct_id: usize,
    pointer_type: types::Type,
    structs: &[RynStruct],
    runtime: PrintFunctions,
    clear: bool,
) -> Result<CompiledValue, String> {
    let mut values = Vec::new();
    for field in &structs[struct_id].fields {
        values.push(decode_enum_value_payload(
            b,
            pointer,
            word_offset + field.slot_offset,
            field.ty,
            pointer_type,
            structs,
            runtime,
            clear,
        )?);
    }
    Ok(CompiledValue::Struct {
        struct_id,
        fields: values,
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_enum_value_payload(
    b: &mut FunctionBuilder<'_>,
    pointer: Value,
    offset: usize,
    ty: Type,
    pointer_type: types::Type,
    structs: &[RynStruct],
    runtime: PrintFunctions,
    clear: bool,
) -> Result<CompiledValue, String> {
    match ty {
        Type::Struct(struct_id) => decode_enum_struct_payload(
            b,
            pointer,
            offset,
            struct_id,
            pointer_type,
            structs,
            runtime,
            clear,
        ),
        Type::Array(array_id) => {
            let (element, length) = array_info(array_id);
            let stride = value_width(element, structs);
            let mut values = Vec::with_capacity(length);
            for index in 0..length {
                values.push(decode_enum_value_payload(
                    b,
                    pointer,
                    offset + index * stride,
                    element,
                    pointer_type,
                    structs,
                    runtime,
                    clear,
                )?);
            }
            Ok(CompiledValue::Array(values))
        }
        _ => {
            let word_index = b.ins().iconst(types::I64, offset as i64);
            let get = b.ins().call(runtime.enum_word, &[pointer, word_index]);
            let raw = b.func.dfg.inst_results(get)[0];
            let value = decode_enum_payload(b, raw, ty, pointer_type)?;
            if matches!(
                ty,
                Type::OwnedString | Type::Vec(_) | Type::Enum(_) | Type::Map(_)
            ) {
                if clear {
                    b.ins()
                        .call(runtime.enum_clear_word, &[pointer, word_index]);
                } else {
                    return Ok(mark_borrowed(value));
                }
            }
            Ok(value)
        }
    }
}

/// Rewrites decoded owner handles from moved temporaries into borrowed views.
fn mark_borrowed(value: CompiledValue) -> CompiledValue {
    match value {
        CompiledValue::OwnedString { ptr, .. } => CompiledValue::OwnedString {
            ptr,
            temporary: false,
        },
        CompiledValue::Vec { ptr, .. } => CompiledValue::Vec {
            ptr,
            temporary: false,
        },
        CompiledValue::Enum { ptr, .. } => CompiledValue::Enum {
            ptr,
            temporary: false,
        },
        CompiledValue::Map { ptr, .. } => CompiledValue::Map {
            ptr,
            temporary: false,
        },
        CompiledValue::Struct { struct_id, fields } => CompiledValue::Struct {
            struct_id,
            fields: fields.into_iter().map(mark_borrowed).collect(),
        },
        CompiledValue::Array(values) => {
            CompiledValue::Array(values.into_iter().map(mark_borrowed).collect())
        }
        other => other,
    }
}

fn compiled_value_matches_type(value: &CompiledValue, ty: Type) -> bool {
    match (value, ty) {
        (CompiledValue::Integer(_, actual), expected) => *actual == expected,
        (CompiledValue::F32(_), Type::F32)
        | (CompiledValue::F64(_), Type::F64)
        | (CompiledValue::Str { .. }, Type::Str)
        | (CompiledValue::Slice { .. }, Type::Slice(_))
        | (CompiledValue::OwnedString { .. }, Type::OwnedString)
        | (CompiledValue::Vec { .. }, Type::Vec(_))
        | (CompiledValue::Enum { .. }, Type::Enum(_))
        | (CompiledValue::Map { .. }, Type::Map(_) | Type::Set(_))
        | (CompiledValue::Bool(_), Type::Bool) => true,
        (CompiledValue::Struct { struct_id, fields }, Type::Struct(expected)) => {
            *struct_id == expected && !fields.is_empty()
        }
        (CompiledValue::Array(values), Type::Array(id)) => values.len() == array_info(id).1,
        _ => false,
    }
}

fn compiled_value_from_type(
    ty: Type,
    params: &[Value],
    offset: &mut usize,
    structs: &[RynStruct],
) -> Result<CompiledValue, String> {
    let result = match ty {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Char => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: aggregate value is missing a component".to_string()
            })?;
            *offset += 1;
            CompiledValue::Integer(first, ty)
        }
        Type::F32 => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: aggregate value is missing a component".to_string()
            })?;
            *offset += 1;
            CompiledValue::F32(first)
        }
        Type::F64 => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: aggregate value is missing a component".to_string()
            })?;
            *offset += 1;
            CompiledValue::F64(first)
        }
        Type::Bool => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: aggregate value is missing a component".to_string()
            })?;
            *offset += 1;
            CompiledValue::Bool(first)
        }
        Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: pointer value is missing its component".to_string()
            })?;
            *offset += 1;
            CompiledValue::Integer(first, ty)
        }
        Type::Str => {
            let first = *params.get(*offset).ok_or_else(|| {
                "internal error: aggregate string is missing its pointer".to_string()
            })?;
            let len = *params.get(*offset + 1).ok_or_else(|| {
                "internal error: string aggregate is missing its length".to_string()
            })?;
            *offset += 2;
            CompiledValue::Str { ptr: first, len }
        }
        Type::Slice(_) => {
            let first = *params
                .get(*offset)
                .ok_or("internal error: slice aggregate is missing its pointer")?;
            let len = *params
                .get(*offset + 1)
                .ok_or("internal error: slice aggregate is missing its length")?;
            *offset += 2;
            CompiledValue::Slice { ptr: first, len }
        }
        Type::OwnedString => {
            let pointer = *params
                .get(*offset)
                .ok_or("internal error: missing String handle")?;
            *offset += 1;
            CompiledValue::OwnedString {
                ptr: pointer,
                temporary: true,
            }
        }
        Type::Vec(_) => {
            let pointer = *params
                .get(*offset)
                .ok_or("internal error: missing Vec handle")?;
            *offset += 1;
            CompiledValue::Vec {
                ptr: pointer,
                temporary: true,
            }
        }
        Type::Map(_) | Type::Set(_) => {
            let pointer = *params
                .get(*offset)
                .ok_or("internal error: missing Map handle")?;
            *offset += 1;
            CompiledValue::Map {
                ptr: pointer,
                temporary: true,
            }
        }
        Type::Enum(_) => {
            let pointer = *params
                .get(*offset)
                .ok_or("internal error: missing enum handle")?;
            *offset += 1;
            CompiledValue::Enum {
                ptr: pointer,
                temporary: true,
            }
        }
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                fields.push(compiled_value_from_type(field.ty, params, offset, structs)?);
            }
            CompiledValue::Struct { struct_id, fields }
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let mut values = Vec::with_capacity(length);
            for _ in 0..length {
                values.push(compiled_value_from_type(element, params, offset, structs)?);
            }
            CompiledValue::Array(values)
        }
    };
    Ok(result)
}

#[derive(Clone, Copy)]
struct ExprEnv<'a> {
    owned_slots: &'a [(usize, Type)],
    strings: &'a HashMap<String, DataId>,
    calls: &'a [FuncRef],
    vec_struct_callbacks: &'a [Option<(FuncRef, FuncRef)>],
    print_functions: PrintFunctions,
    structs: &'a [RynStruct],
    enums: &'a [RynEnum],
    enum_drop_plans: &'a [Option<DataId>],
    function_returns: &'a [Option<Type>],
    external_functions: &'a [bool],
    function_parameters: &'a [Vec<Type>],
    function_return: Option<Type>,
    sret_pointer: Option<Value>,
    address_slots: &'a [Option<StackSlot>],
    addressed_slot_types: &'a [Option<Type>],
}

fn enum_drop_plan_len(definition: &RynEnum, structs: &[RynStruct]) -> usize {
    let mut bytes = Vec::new();
    for (variant_index, variant) in definition.variants.iter().enumerate() {
        let mut offset = 0;
        for field in &variant.fields {
            append_enum_drop_entries(&mut bytes, variant_index, *field, offset, structs);
            offset += value_width(*field, structs);
        }
    }
    bytes.len()
}

fn encode_enum_word(
    b: &mut FunctionBuilder<'_>,
    value: Value,
    ty: Type,
    pointer_type: types::Type,
) -> Result<Value, String> {
    Ok(match ty {
        Type::I64 | Type::U64 | Type::F64 => {
            if ty == Type::F64 {
                b.ins().bitcast(types::I64, MemFlagsData::new(), value)
            } else {
                value
            }
        }
        Type::F32 => {
            let bits = b.ins().bitcast(types::I32, MemFlagsData::new(), value);
            b.ins().uextend(types::I64, bits)
        }
        Type::OwnedString | Type::Vec(_) | Type::Enum(_) | Type::Map(_) | Type::Set(_) => {
            if pointer_type == types::I64 {
                value
            } else {
                b.ins().uextend(types::I64, value)
            }
        }
        Type::RawPointer(_) | Type::Reference(_, _) | Type::FunctionPointer(_) => {
            if pointer_type == types::I64 {
                value
            } else {
                b.ins().uextend(types::I64, value)
            }
        }
        Type::I8
        | Type::I16
        | Type::I32
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::Bool
        | Type::Char => b.ins().uextend(types::I64, value),
        _ => return Err("internal error: unsupported enum payload layout".into()),
    })
}

fn clif_types(ty: Type, pointer_type: types::Type, structs: &[RynStruct]) -> Vec<types::Type> {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => vec![types::I8],
        Type::I16 | Type::U16 => vec![types::I16],
        Type::I32 | Type::U32 | Type::Char => vec![types::I32],
        Type::I64 | Type::U64 => vec![types::I64],
        Type::F32 => vec![types::F32],
        Type::F64 => vec![types::F64],
        Type::Str | Type::Slice(_) => vec![pointer_type, types::I64],
        Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Reference(_, _)
        | Type::RawPointer(_)
        | Type::FunctionPointer(_) => vec![pointer_type],
        Type::Struct(struct_id) => structs[struct_id]
            .fields
            .iter()
            .flat_map(|field| clif_types(field.ty, pointer_type, structs))
            .collect(),
        Type::Array(id) => {
            let (element, length) = array_info(id);
            (0..length)
                .flat_map(|_| clif_types(element, pointer_type, structs))
                .collect()
        }
    }
}

fn emit_checked_integer_division(
    b: &mut FunctionBuilder<'_>,
    op: BinaryOp,
    operands: [Value; 2],
    ty: Type,
    unsigned: bool,
    runtime: PrintFunctions,
    seal_state: &mut BlockSealState,
) -> Result<Value, String> {
    let [left, right] = operands;
    let value_type = clif_integer_type(ty)
        .ok_or_else(|| "internal error: integer division has a non-integer type".to_string())?;
    let zero = b.ins().icmp_imm_s(IntCC::Equal, right, 0);
    let error_block = b.create_block();
    let divide_block = b.create_block();
    let merge_block = b.create_block();
    b.append_block_param(merge_block, value_type);

    let from = b
        .current_block()
        .ok_or_else(|| "internal error: missing integer division source block".to_string())?;
    b.ins().brif(zero, error_block, &[], divide_block, &[]);
    seal_ready(b, from, seal_state);

    b.switch_to_block(error_block);
    b.ins().call(runtime.integer_division_by_zero, &[]);
    let unreachable_value = b.ins().iconst(value_type, 0);
    b.ins()
        .jump(merge_block, &[BlockArg::Value(unreachable_value)]);
    seal_ready(b, error_block, seal_state);

    b.switch_to_block(divide_block);
    let divisor = if unsigned {
        right
    } else {
        let minimum = match ty {
            Type::I8 => i8::MIN as i64,
            Type::I16 => i16::MIN as i64,
            Type::I32 => i32::MIN as i64,
            Type::I64 => i64::MIN,
            _ => return Err("internal error: signed division has a non-signed type".into()),
        };
        let left_is_minimum = b.ins().icmp_imm_s(IntCC::Equal, left, minimum);
        let right_is_minus_one = b.ins().icmp_imm_s(IntCC::Equal, right, -1);
        let one = b.ins().iconst(value_type, 1);
        let safe_right = b.ins().select(right_is_minus_one, one, right);
        b.ins().select(left_is_minimum, safe_right, right)
    };
    let result = match (op, unsigned) {
        (BinaryOp::Div, true) => b.ins().udiv(left, divisor),
        (BinaryOp::Rem, true) => b.ins().urem(left, divisor),
        (BinaryOp::Div, false) => b.ins().sdiv(left, divisor),
        (BinaryOp::Rem, false) => b.ins().srem(left, divisor),
        _ => return Err("internal error: non-division operator reached integer division".into()),
    };
    let divide_end = b
        .current_block()
        .ok_or_else(|| "internal error: missing integer division block".to_string())?;
    b.ins().jump(merge_block, &[BlockArg::Value(result)]);
    seal_ready(b, divide_end, seal_state);

    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    Ok(b.block_params(merge_block)[0])
}

fn emit_short_circuit(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    op: BinaryOp,
    left: &IrExpression,
    right: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Value, String> {
    let left = as_bool(emit_expr(b, module, left, env, seal_state)?)?;
    let rhs_block = b.create_block();
    let short_block = b.create_block();
    let merge_block = b.create_block();
    b.append_block_param(merge_block, types::I8);
    let from = b
        .current_block()
        .ok_or_else(|| "internal error: missing short-circuit source block".to_string())?;
    if op == BinaryOp::And {
        b.ins().brif(left, rhs_block, &[], short_block, &[]);
    } else {
        b.ins().brif(left, short_block, &[], rhs_block, &[]);
    }
    seal_ready(b, from, seal_state);

    b.switch_to_block(short_block);
    let short_value = b.ins().iconst(types::I8, i64::from(op == BinaryOp::Or));
    b.ins().jump(merge_block, &[BlockArg::Value(short_value)]);
    seal_ready(b, short_block, seal_state);

    b.switch_to_block(rhs_block);
    let right = as_bool(emit_expr(b, module, right, env, seal_state)?)?;
    let rhs_end = b
        .current_block()
        .ok_or_else(|| "internal error: missing short-circuit RHS block".to_string())?;
    b.ins().jump(merge_block, &[BlockArg::Value(right)]);
    seal_ready(b, rhs_end, seal_state);

    b.switch_to_block(merge_block);
    seal_ready(b, merge_block, seal_state);
    Ok(b.block_params(merge_block)[0])
}

fn as_numeric(value: CompiledValue) -> Result<(Value, Type), String> {
    match value {
        CompiledValue::Integer(v, ty) => Ok((v, ty)),
        CompiledValue::F32(v) => Ok((v, Type::F32)),
        CompiledValue::F64(v) => Ok((v, Type::F64)),
        _ => Err("internal error: expected numeric value after semantic analysis".into()),
    }
}
fn as_bool(value: CompiledValue) -> Result<Value, String> {
    match value {
        CompiledValue::Bool(v) => Ok(v),
        _ => Err("internal error: expected bool after semantic analysis".into()),
    }
}

/// Links with the system C compiler driver (`CC`, default `cc`) and the
/// precompiled runtime library; the runtime defines `main`.
#[cfg(unix)]
fn link_with_cc(work: &Path, object: &Path, output: &Path) -> Result<(), BuildError> {
    let library = work.join("libryn_runtime.a");
    fs::write(&library, RUNTIME_LIBRARY).map_err(|error| {
        BuildError::Environment(format!("could not write the runtime library: {error}"))
    })?;
    let cc = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
    let mut command = Command::new(&cc);
    command.arg(object).arg(&library);
    if cfg!(target_os = "linux") {
        command.args([
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]);
    } else {
        command.args(["-lpthread", "-lm"]);
    }
    let output_status = command.arg("-o").arg(output).output().map_err(|error| {
        BuildError::Link(format!(
            "error[R0300]: could not start the C linker `{}`: {error}; install a C toolchain such as gcc or clang, or set CC",
            cc.to_string_lossy()
        ))
    })?;
    if !output_status.status.success() {
        return Err(BuildError::Link(format!(
            "error[R0300]: native link step failed with {}: {}",
            output_status.status,
            String::from_utf8_lossy(&output_status.stderr).trim()
        )));
    }
    Ok(())
}

fn link_with_rust(
    rustc: &OsStr,
    object: &Path,
    wrapper: &Path,
    runtime_object: Option<&Path>,
    output: &Path,
) -> Result<(), BuildError> {
    let use_precompiled_shim = runtime_object.is_some();
    write_linker_wrapper(wrapper, use_precompiled_shim)?;
    let status = run_rust_link(
        rustc,
        object,
        wrapper,
        runtime_object,
        output,
        use_precompiled_shim,
    )?;
    if status.success() {
        return Ok(());
    }
    if !use_precompiled_shim {
        return Err(link_failure(status));
    }

    // A toolchain update can make the embedded std-dependent object incompatible.
    // Retry with the source shim so changing the user's rustc never breaks builds.
    write_linker_wrapper(wrapper, false)?;
    let status = run_rust_link(rustc, object, wrapper, None, output, false)?;
    if !status.success() {
        return Err(link_failure(status));
    }
    Ok(())
}

fn write_linker_wrapper(wrapper: &Path, use_precompiled_shim: bool) -> Result<(), BuildError> {
    let source = if use_precompiled_shim {
        LINKER_ENTRY_SOURCE.to_owned()
    } else {
        let shim = include_str!("runtime_shim.rs")
            .replace(
                "#[path = \"runtime/string.rs\"]\npub mod strings;",
                &format!(
                    "pub mod strings {{ {} }}",
                    include_str!("runtime/string.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/string.rs\"]\r\npub mod strings;",
                &format!(
                    "pub mod strings {{ {} }}",
                    include_str!("runtime/string.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/vector.rs\"]\npub mod vectors;",
                &format!(
                    "pub mod vectors {{ {} }}",
                    include_str!("runtime/vector.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/vector.rs\"]\r\npub mod vectors;",
                &format!(
                    "pub mod vectors {{ {} }}",
                    include_str!("runtime/vector.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/map.rs\"]\npub mod maps;",
                &format!("pub mod maps {{ {} }}", include_str!("runtime/map.rs")),
            )
            .replace(
                "#[path = \"runtime/map.rs\"]\r\npub mod maps;",
                &format!("pub mod maps {{ {} }}", include_str!("runtime/map.rs")),
            )
            .replace(
                "#[path = \"runtime/enum.rs\"]\npub mod enums;",
                &format!("pub mod enums {{ {} }}", include_str!("runtime/enum.rs")),
            )
            .replace(
                "#[path = \"runtime/enum.rs\"]\r\npub mod enums;",
                &format!("pub mod enums {{ {} }}", include_str!("runtime/enum.rs")),
            )
            .replace(
                "#[path = \"runtime/filesystem.rs\"]\npub mod filesystem;",
                &format!(
                    "pub mod filesystem {{ {} }}",
                    include_str!("runtime/filesystem.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/filesystem.rs\"]\r\npub mod filesystem;",
                &format!(
                    "pub mod filesystem {{ {} }}",
                    include_str!("runtime/filesystem.rs")
                ),
            )
            .replace(
                "#[path = \"runtime/system.rs\"]\npub mod system;",
                &format!("pub mod system {{ {} }}", include_str!("runtime/system.rs")),
            )
            .replace(
                "#[path = \"runtime/system.rs\"]\r\npub mod system;",
                &format!("pub mod system {{ {} }}", include_str!("runtime/system.rs")),
            )
            .replace(
                "#[path = \"runtime/input.rs\"]\npub mod input;",
                &format!("pub mod input {{ {} }}", include_str!("runtime/input.rs")),
            )
            .replace(
                "#[path = \"runtime/input.rs\"]\r\npub mod input;",
                &format!("pub mod input {{ {} }}", include_str!("runtime/input.rs")),
            );
        format!(
            "#![allow(unused_unsafe, function_casts_as_integer)]\n{shim}\n{LINKER_ENTRY_SOURCE}"
        )
    };
    fs::write(wrapper, source).map_err(|error| {
        BuildError::Environment(format!("could not create linker wrapper: {error}"))
    })
}

fn run_rust_link(
    rustc: &OsStr,
    object: &Path,
    wrapper: &Path,
    runtime_object: Option<&Path>,
    output: &Path,
    suppress_linker_output: bool,
) -> Result<ExitStatus, BuildError> {
    let mut command = Command::new(rustc);
    command
        .arg("--edition=2024")
        .arg("--crate-name")
        .arg("ryn_wrapper")
        .arg(wrapper)
        .arg("-C")
        .arg("debuginfo=0")
        .arg("-C")
        .arg(format!("link-arg={}", object.display()));
    // Runtime panic locations name the wrapper source; map the per-build
    // temporary directory to a fixed name so builds are reproducible.
    if let Some(directory) = wrapper.parent() {
        command.arg(format!(
            "--remap-path-prefix={}=ryn-build",
            directory.display()
        ));
    }
    if let Some(runtime_object) = runtime_object {
        command
            .arg("-C")
            .arg(format!("link-arg={}", runtime_object.display()));
    }
    #[cfg(target_os = "linux")]
    command.arg("-l").arg("dl");
    if suppress_linker_output {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    command.arg("-o").arg(output);
    if cfg!(target_env = "msvc") {
        command.arg("-C").arg(format!(
            "link-arg=/PDB:{}",
            wrapper.with_extension("pdb").display()
        ));
        // Reproducible images: the same program links to the same bytes.
        command.arg("-C").arg("link-arg=/Brepro");
    }
    command.status().map_err(|error| {
        BuildError::Link(format!(
            "error[R0300]: could not start rustc linker: {error}"
        ))
    })
}

fn link_failure(status: ExitStatus) -> BuildError {
    BuildError::Link(format!(
        "error[R0300]: native link step failed with {status}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        BuildError, BuildTempDir, append_type, emit_object, link_with_rust,
        native_backend_setup_error, string_equals_signature, string_signature,
    };
    use crate::{check, sema::Type};
    use cranelift_codegen::{
        ir::{AbiParam, types},
        isa::CallConv,
    };
    use std::{
        ffi::OsStr,
        fs,
        path::Path,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn internal_codegen_errors_are_identified_separately_from_build_failures() {
        assert_eq!(
            BuildError::Internal("invalid generated signature".into()).to_string(),
            "error[R0900]: internal compiler error during native code generation: invalid generated signature"
        );
        assert_eq!(
            BuildError::Environment("permission denied".into()).to_string(),
            "error[R0302]: native build setup failed: permission denied"
        );
        assert_eq!(
            BuildError::Link("error[R0300]: native link step failed".into()).to_string(),
            "error[R0300]: native link step failed"
        );
    }

    #[test]
    fn unsupported_host_backend_reports_which_component_is_unavailable() {
        assert_eq!(
            native_backend_setup_error("unsupported architecture").to_string(),
            "error[R0302]: native build setup failed: Cranelift native backend is unavailable: unsupported architecture"
        );
    }

    #[test]
    fn string_abi_uses_a_fixed_64_bit_length_for_each_pointer_width() {
        for pointer_type in [types::I32, types::I64] {
            let mut params = Vec::new();
            append_type(&mut params, Type::Str, pointer_type, &[]);

            assert_eq!(
                params,
                [AbiParam::new(pointer_type), AbiParam::new(types::I64)]
            );
        }
    }

    #[test]
    fn string_runtime_imports_keep_64_bit_lengths_with_32_bit_pointers() {
        let print = string_signature(CallConv::SystemV, types::I32);
        assert_eq!(
            print
                .params
                .iter()
                .map(|param| param.value_type)
                .collect::<Vec<_>>(),
            [types::I32, types::I64]
        );
        let equals = string_equals_signature(CallConv::SystemV, types::I32);
        assert_eq!(
            equals
                .params
                .iter()
                .map(|param| param.value_type)
                .collect::<Vec<_>>(),
            [types::I32, types::I64, types::I32, types::I64]
        );
    }

    #[test]
    fn linker_wrapper_io_errors_are_classified_as_environment_failures() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after epoch")
            .as_nanos();
        let wrapper = std::env::temp_dir()
            .join(format!(
                "ryn-missing-build-dir-{}-{unique}",
                std::process::id()
            ))
            .join("wrapper.rs");

        let error = link_with_rust(
            OsStr::new("rustc"),
            Path::new("unused.obj"),
            &wrapper,
            None,
            Path::new("unused.exe"),
        )
        .expect_err("writing into a missing directory must fail");
        assert!(matches!(error, BuildError::Environment(_)));
        assert!(error.to_string().starts_with("error[R0302]:"));
    }

    #[test]
    fn incompatible_precompiled_runtime_falls_back_to_source_shim() {
        let temp_dir = BuildTempDir::create().expect("temporary build directory is created");
        let object = emit_object(
            &check("fun main() { echo 42 echo arg_count() echo arg(0) }").expect("source checks"),
        )
        .expect("Cranelift object is emitted");
        let object_path = temp_dir.path().join(if cfg!(windows) {
            "module.obj"
        } else {
            "module.o"
        });
        let runtime_path = temp_dir.path().join(if cfg!(windows) {
            "bad_runtime.obj"
        } else {
            "bad_runtime.o"
        });
        let wrapper_path = temp_dir.path().join("wrapper.rs");
        let output_path = temp_dir.path().join(if cfg!(windows) {
            "fallback.exe"
        } else {
            "fallback"
        });
        fs::write(&object_path, object).expect("Cranelift object is written");
        fs::write(&runtime_path, b"not a valid object file")
            .expect("invalid runtime object is written");

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        link_with_rust(
            &rustc,
            &object_path,
            &wrapper_path,
            Some(&runtime_path),
            &output_path,
        )
        .expect("linker retries with the source runtime shim");

        let run = Command::new(&output_path)
            .arg("fallback-argument")
            .output()
            .expect("fallback executable runs");
        assert!(run.status.success());
        assert_eq!(run.stdout, b"42\n1\nfallback-argument\n");
    }
}
