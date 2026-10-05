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
        AbiParam, BlockArg, FuncRef, InstBuilder, MemFlagsData, Signature, StackSlotData,
        StackSlotKind, Value,
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
    sema::{
        IrCallTarget, IrExpression, IrPrintPart, IrStatement, LocalBinding, LocalType, RynFunction,
        RynIr, RynStruct, Type,
    },
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
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
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
    for (index, function) in ir.functions.iter().enumerate() {
        let signature = make_signature(&mut module, function, pointer_type, &ir.structs);
        let symbol = format!("ryn_fn_{index}");
        let id = module
            .declare_function(&symbol, Linkage::Export, &signature)
            .map_err(|e| BuildError::Internal(e.to_string()))?;
        signatures.push(signature);
        function_ids.push(id);
    }

    let codegen_env = FunctionCodegenEnv {
        function_ids: &function_ids,
        strings: &string_data,
        print_ids: print_functions,
        structs: &ir.structs,
        return_types: &return_types,
    };
    for (index, function) in ir.functions.iter().enumerate() {
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
    let object = emit_object(ir)?;
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
    Ok(PrintFunctionIds {
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
    })
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
        append_type(&mut signature.params, parameter.ty, pointer_type, structs);
    }
    if let Some(ty) = function.return_type {
        if matches!(ty, Type::Struct(_)) {
            // Aggregate returns use a caller-provided stack buffer because the
            // native ABI may not support the record's flattened result count.
            signature.params.insert(0, AbiParam::new(pointer_type));
        } else {
            append_type(&mut signature.returns, ty, pointer_type, structs);
        }
    }
    signature
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
        Type::I32 | Type::U32 => params.push(AbiParam::new(types::I32)),
        Type::I64 | Type::U64 => params.push(AbiParam::new(types::I64)),
        Type::F32 => params.push(AbiParam::new(types::F32)),
        Type::F64 => params.push(AbiParam::new(types::F64)),
        Type::Bool => params.push(AbiParam::new(types::I8)),
        Type::Str => params.extend([AbiParam::new(pointer_type), AbiParam::new(types::I64)]),
        Type::Struct(struct_id) => {
            for field in &structs[struct_id].fields {
                append_type(params, field.ty, pointer_type, structs);
            }
        }
    }
}

fn clif_integer_type(ty: Type) -> Option<types::Type> {
    match ty {
        Type::I8 | Type::U8 => Some(types::I8),
        Type::I16 | Type::U16 => Some(types::I16),
        Type::I32 | Type::U32 => Some(types::I32),
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
                LocalType::Ptr => pointer_type,
                LocalType::I8 => types::I8,
                LocalType::I16 => types::I16,
            };
            b.declare_var(ty);
        }
        bind_parameters(
            &mut b,
            &function.parameters,
            entry,
            codegen_env.structs,
            usize::from(matches!(function.return_type, Some(Type::Struct(_)))),
        );
        let sret_pointer = if matches!(function.return_type, Some(Type::Struct(_))) {
            Some(b.block_params(entry)[0])
        } else {
            None
        };
        let print_functions = PrintFunctions {
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
        };
        let calls = codegen_env
            .function_ids
            .iter()
            .map(|id| module.declare_func_in_func(*id, b.func))
            .collect::<Vec<_>>();
        let env = ExprEnv {
            strings: codegen_env.strings,
            calls: &calls,
            print_functions,
            structs: codegen_env.structs,
            function_returns: codegen_env.return_types,
            function_return: function.return_type,
            sret_pointer,
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
                if let Some(Type::Struct(struct_id)) = function.return_type {
                    store_struct_return(
                        &mut b,
                        struct_id,
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
            b.ins().return_(&returns);
        }
        seal_ready(&mut b, entry, &mut seal_state);
        b.finalize(module.target_config());
    }
    module
        .define_function(function_id, &mut context)
        .map_err(|e| e.to_string())?;
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
        | Type::Bool => {
            b.def_var(Variable::from_u32(slot as u32), values[*offset]);
            *offset += 1;
        }
        Type::Str => {
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
    }
}

#[derive(Clone, Copy)]
struct PrintFunctionIds {
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
}

struct FunctionCodegenEnv<'a> {
    function_ids: &'a [FuncId],
    strings: &'a HashMap<String, DataId>,
    print_ids: PrintFunctionIds,
    structs: &'a [RynStruct],
    return_types: &'a [Option<Type>],
}

#[derive(Clone, Copy)]
struct PrintFunctions {
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
            IrStatement::Let { slot, ty, value } | IrStatement::Assign { slot, ty, value } => {
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                store_local(b, *slot, *ty, compiled, env.structs)?;
                true
            }
            IrStatement::FieldAssign { slot, ty, value } => {
                let compiled = emit_expr(b, module, value, env, seal_state)?;
                store_local(b, *slot, *ty, compiled, env.structs)?;
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
                    *ty,
                    compiled,
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
                        ty,
                        compiled,
                    )?;
                }
                b.ins().call(env.print_functions.newline, &[]);
                true
            }
            IrStatement::Call { target, arguments } => {
                let _ = emit_call(b, module, *target, arguments, env, seal_state)?;
                true
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
            IrStatement::While { condition, body } => {
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
                let from = b
                    .current_block()
                    .ok_or_else(|| "internal error: missing for-loop predecessor".to_string())?;
                b.ins().jump(header, &[]);
                seal_ready(b, from, seal_state);

                b.switch_to_block(header);
                seal_state.deferred.push(header);
                let current = b.use_var(Variable::from_u32(*slot as u32));
                let end = b.use_var(Variable::from_u32(*end_slot as u32));
                let condition = if matches!(ty, Type::U8 | Type::U16 | Type::U32 | Type::U64) {
                    b.ins().icmp(IntCC::UnsignedLessThan, current, end)
                } else {
                    b.ins().icmp(IntCC::SignedLessThan, current, end)
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
                let current = b.use_var(Variable::from_u32(*slot as u32));
                let value_type = clif_integer_type(*ty)
                    .ok_or_else(|| "internal error: for-loop has a non-integer type".to_string())?;
                let one = b.ins().iconst(value_type, 1);
                let next = b.ins().iadd(current, one);
                b.def_var(Variable::from_u32(*slot as u32), next);
                b.ins().jump(header, &[]);
                seal_ready(b, increment, seal_state);

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
                    if let Some(Type::Struct(struct_id)) = env.function_return {
                        let ptr = env.sret_pointer.ok_or_else(|| {
                            "internal error: missing structure return buffer".to_string()
                        })?;
                        store_struct_return(b, struct_id, compiled, ptr, env.structs)?;
                        Vec::new()
                    } else {
                        flatten_value(compiled)
                    }
                } else {
                    Vec::new()
                };
                b.ins().return_(&values);
                false
            }
        };
        if !falls_through {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy)]
struct LoopBlocks {
    continue_target: cranelift_codegen::ir::Block,
    break_target: cranelift_codegen::ir::Block,
}

fn emit_print_value(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    structs: &[RynStruct],
    ty: Type,
    compiled: CompiledValue,
) -> Result<(), String> {
    match (ty, compiled) {
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
                struct_id,
                &fields,
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

fn emit_print_struct(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    print_functions: PrintFunctions,
    strings: &HashMap<String, DataId>,
    structs: &[RynStruct],
    struct_id: usize,
    fields: &[CompiledValue],
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
            field.ty,
            value.clone(),
        )?;
    }
    emit_print_text(b, module, print_functions, strings, " }")
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
        (Type::Str, CompiledValue::Str { ptr, len }) => {
            b.def_var(Variable::from_u32(slot as u32), ptr);
            b.def_var(Variable::from_u32(slot as u32 + 1), len);
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
        | Type::U64 => CompiledValue::Integer(first, ty),
        Type::F32 => CompiledValue::F32(first),
        Type::F64 => CompiledValue::F64(first),
        Type::Bool => CompiledValue::Bool(first),
        Type::Str => CompiledValue::Str {
            ptr: first,
            len: b.use_var(Variable::from_u32(slot as u32 + 1)),
        },
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                fields.push(load_local(b, slot + field.slot_offset, field.ty, structs)?);
            }
            CompiledValue::Struct { struct_id, fields }
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
    fn print_type(ty: Type, structs: &[RynStruct], out: &mut BTreeSet<String>) {
        let Type::Struct(struct_id) = ty else {
            return;
        };
        let definition = &structs[struct_id];
        out.insert(definition.name.clone());
        out.insert(" { ".into());
        out.insert(", ".into());
        out.insert(": ".into());
        out.insert(" }".into());
        for field in &definition.fields {
            out.insert(field.name.clone());
            print_type(field.ty, structs, out);
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
            | IrExpression::Float(..)
            | IrExpression::Boolean(_)
            | IrExpression::Local { .. } => {}
        }
    }
    fn stmts(values: &[IrStatement], structs: &[RynStruct], out: &mut BTreeSet<String>) {
        for stmt in values {
            match stmt {
                IrStatement::Let { value, .. }
                | IrStatement::Assign { value, .. }
                | IrStatement::FieldAssign { value, .. } => expr(value, out),
                IrStatement::Print { value, ty } => {
                    expr(value, out);
                    print_type(*ty, structs, out);
                }
                IrStatement::PrintTemplate(parts) => {
                    for part in parts {
                        match part {
                            IrPrintPart::Text(text) => {
                                out.insert(text.clone());
                            }
                            IrPrintPart::Value { value, ty } => {
                                expr(value, out);
                                print_type(*ty, structs, out);
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
                    stmts(then_body, structs, out);
                    stmts(else_body, structs, out);
                }
                IrStatement::While { condition, body } => {
                    expr(condition, out);
                    stmts(body, structs, out);
                }
                IrStatement::For {
                    start, end, body, ..
                } => {
                    expr(start, out);
                    expr(end, out);
                    stmts(body, structs, out);
                }
                IrStatement::Return { value } => {
                    if let Some(value) = value {
                        expr(value, out);
                    }
                }
                IrStatement::Break | IrStatement::Continue => {}
            }
        }
    }
    let mut strings = BTreeSet::new();
    for function in &ir.functions {
        stmts(&function.statements, &ir.structs, &mut strings);
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
    Str {
        ptr: Value,
        len: Value,
    },
    Struct {
        struct_id: usize,
        fields: Vec<CompiledValue>,
    },
}

fn flatten_value(value: CompiledValue) -> Vec<Value> {
    match value {
        CompiledValue::Integer(v, _)
        | CompiledValue::F32(v)
        | CompiledValue::F64(v)
        | CompiledValue::Bool(v) => vec![v],
        CompiledValue::Str { ptr, len } => vec![ptr, len],
        CompiledValue::Struct { fields, .. } => {
            fields.into_iter().flat_map(flatten_value).collect()
        }
    }
}

fn emit_expr(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    expr: &IrExpression,
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<CompiledValue, String> {
    Ok(match expr {
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
        IrExpression::Local { slot, ty }
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
            ) =>
        {
            CompiledValue::Integer(b.use_var(Variable::from_u32(*slot as u32)), *ty)
        }
        IrExpression::Local {
            slot,
            ty: Type::F32,
        } => CompiledValue::F32(b.use_var(Variable::from_u32(*slot as u32))),
        IrExpression::Local {
            slot,
            ty: Type::F64,
        } => CompiledValue::F64(b.use_var(Variable::from_u32(*slot as u32))),
        IrExpression::Local {
            slot,
            ty: Type::Bool,
        } => CompiledValue::Bool(b.use_var(Variable::from_u32(*slot as u32))),
        IrExpression::Local {
            slot,
            ty: Type::Str,
        } => CompiledValue::Str {
            ptr: b.use_var(Variable::from_u32(*slot as u32)),
            len: b.use_var(Variable::from_u32(*slot as u32 + 1)),
        },
        IrExpression::Local {
            slot,
            ty: Type::Struct(struct_id),
        } => {
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
            fields
                .get(*field_index)
                .cloned()
                .ok_or_else(|| "internal error: field index is out of range".to_string())?
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
                | Type::U64 => CompiledValue::Integer(values[0], *return_type),
                Type::F32 => CompiledValue::F32(values[0]),
                Type::F64 => CompiledValue::F64(values[0]),
                Type::Bool => CompiledValue::Bool(values[0]),
                Type::Str => CompiledValue::Str {
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
            }
        }
        IrExpression::If { .. } => emit_if_expression(b, module, expr, env, seal_state)?,
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
        IrExpression::Cast {
            value,
            source,
            target,
        } => {
            let (value, actual_source) = as_numeric(emit_expr(b, module, value, env, seal_state)?)?;
            if actual_source != *source {
                return Err("internal error: numeric cast source differs from checked type".into());
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
                            return Err("internal error: numeric cast target is not numeric".into());
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
                        let lower_clamped = b.ins().select(below_minimum, minimum_value, converted);
                        let above_maximum =
                            b.ins()
                                .icmp(IntCC::SignedGreaterThan, lower_clamped, maximum_value);
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
            } else if *ty == Type::Str {
                let CompiledValue::Str {
                    ptr: left_ptr,
                    len: left_len,
                } = left
                else {
                    return Err(
                        "internal error: string comparison has a non-string left operand".into(),
                    );
                };
                let CompiledValue::Str {
                    ptr: right_ptr,
                    len: right_len,
                } = right
                else {
                    return Err(
                        "internal error: string comparison has a non-string right operand".into(),
                    );
                };
                let call = b.ins().call(
                    env.print_functions.string_equals,
                    &[left_ptr, left_len, right_ptr, right_len],
                );
                let equal = b.func.dfg.inst_results(call)[0];
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

fn emit_call(
    b: &mut FunctionBuilder<'_>,
    module: &ObjectModule,
    target: IrCallTarget,
    arguments: &[IrExpression],
    env: &ExprEnv<'_>,
    seal_state: &mut BlockSealState,
) -> Result<Vec<Value>, String> {
    match target {
        IrCallTarget::ArgumentCount => {
            let inst = b.ins().call(env.print_functions.args_count, &[]);
            Ok(b.func.dfg.inst_results(inst).to_vec())
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
    }
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
    let return_slot = if let Some(Type::Struct(struct_id)) = return_type {
        let slot = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (env.structs[struct_id].slot_count * 8) as u32,
            3,
        ));
        let pointer = b
            .ins()
            .stack_addr(module.target_config().pointer_type(), slot, 0);
        args.push(pointer);
        Some((slot, struct_id))
    } else {
        None
    };
    for arg in arguments {
        args.extend(flatten_value(emit_expr(b, module, arg, env, seal_state)?));
    }
    let inst = b.ins().call(callee, &args);
    let results = b.func.dfg.inst_results(inst).to_vec();
    if let Some((slot, struct_id)) = return_slot {
        let pointer = b
            .ins()
            .stack_addr(module.target_config().pointer_type(), slot, 0);
        let mut values = Vec::new();
        let mut components = Vec::new();
        append_sret_components(
            Type::Struct(struct_id),
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
        Ok(values)
    } else {
        Ok(results)
    }
}

fn clif_scalar_type(ty: Type, pointer_type: types::Type) -> Result<types::Type, String> {
    Ok(match ty {
        Type::I8 | Type::U8 | Type::Bool => types::I8,
        Type::I16 | Type::U16 => types::I16,
        Type::I32 | Type::U32 => types::I32,
        Type::I64 | Type::U64 => types::I64,
        Type::F32 => types::F32,
        Type::F64 => types::F64,
        Type::Str => pointer_type,
        Type::Struct(_) => return Err("internal error: nested structure in return layout".into()),
    })
}

fn store_struct_return(
    b: &mut FunctionBuilder<'_>,
    struct_id: usize,
    value: CompiledValue,
    pointer: Value,
    structs: &[RynStruct],
) -> Result<(), String> {
    let CompiledValue::Struct {
        struct_id: actual,
        fields,
    } = value
    else {
        return Err("internal error: structure return expression is not a structure".into());
    };
    if actual != struct_id || fields.len() != structs[struct_id].fields.len() {
        return Err("internal error: structure return value has an incompatible layout".into());
    }
    store_sret_value(
        b,
        Type::Struct(struct_id),
        CompiledValue::Struct { struct_id, fields },
        0,
        pointer,
        structs,
    )?;
    Ok(())
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

fn compiled_value_matches_type(value: &CompiledValue, ty: Type) -> bool {
    match (value, ty) {
        (CompiledValue::Integer(_, actual), expected) => *actual == expected,
        (CompiledValue::F32(_), Type::F32)
        | (CompiledValue::F64(_), Type::F64)
        | (CompiledValue::Str { .. }, Type::Str)
        | (CompiledValue::Bool(_), Type::Bool) => true,
        (CompiledValue::Struct { struct_id, fields }, Type::Struct(expected)) => {
            *struct_id == expected && !fields.is_empty()
        }
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
        | Type::U64 => {
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
        Type::Struct(struct_id) => {
            let mut fields = Vec::with_capacity(structs[struct_id].fields.len());
            for field in &structs[struct_id].fields {
                fields.push(compiled_value_from_type(field.ty, params, offset, structs)?);
            }
            CompiledValue::Struct { struct_id, fields }
        }
    };
    Ok(result)
}

#[derive(Clone, Copy)]
struct ExprEnv<'a> {
    strings: &'a HashMap<String, DataId>,
    calls: &'a [FuncRef],
    print_functions: PrintFunctions,
    structs: &'a [RynStruct],
    function_returns: &'a [Option<Type>],
    function_return: Option<Type>,
    sret_pointer: Option<Value>,
}

fn clif_types(ty: Type, pointer_type: types::Type, structs: &[RynStruct]) -> Vec<types::Type> {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => vec![types::I8],
        Type::I16 | Type::U16 => vec![types::I16],
        Type::I32 | Type::U32 => vec![types::I32],
        Type::I64 | Type::U64 => vec![types::I64],
        Type::F32 => vec![types::F32],
        Type::F64 => vec![types::F64],
        Type::Str => vec![pointer_type, types::I64],
        Type::Struct(struct_id) => structs[struct_id]
            .fields
            .iter()
            .flat_map(|field| clif_types(field.ty, pointer_type, structs))
            .collect(),
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
        format!("{}\n{LINKER_ENTRY_SOURCE}", include_str!("runtime_shim.rs"))
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
    if let Some(runtime_object) = runtime_object {
        command
            .arg("-C")
            .arg(format!("link-arg={}", runtime_object.display()));
    }
    if suppress_linker_output {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    command.arg("-o").arg(output);
    if cfg!(target_env = "msvc") {
        command.arg("-C").arg(format!(
            "link-arg=/PDB:{}",
            wrapper.with_extension("pdb").display()
        ));
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
            &check("fn main() { print(42) print(arg_count()) print(arg(0)) }")
                .expect("source checks"),
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
