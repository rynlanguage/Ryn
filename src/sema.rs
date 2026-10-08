use std::collections::{HashMap, HashSet};

use crate::{
    ast::{
        BinaryOp, EnumDef, Expression, Function, PrintPart, Program, ShapeDef, Statement,
        StructDef, TypeName, VariantDef,
    },
    filesystem_ops::FilesystemOp,
    map_ops::MapOp,
    source::{Diagnostic, Span},
    string_ops::StringOp,
    system_ops::SystemOp,
    vector_ops::VecOp,
};

const MAX_STRUCT_NESTING: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    OwnedString,
    Char,
    Bool,
    Struct(usize),
    Vec(usize),
    Map(usize),
    Set(usize),
    Enum(usize),
    Array(usize),
    Slice(usize),
    Reference(usize, bool),
    RawPointer(usize),
    FunctionPointer(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionPointerSignature {
    pub parameters: Vec<Type>,
    pub result: Option<Type>,
    pub extern_c: bool,
}

static FUNCTION_POINTER_TYPES: Mutex<Vec<FunctionPointerSignature>> = Mutex::new(Vec::new());

pub fn intern_function_pointer(signature: FunctionPointerSignature) -> usize {
    let mut signatures = FUNCTION_POINTER_TYPES
        .lock()
        .expect("function pointer type registry");
    if let Some(index) = signatures.iter().position(|known| *known == signature) {
        index
    } else {
        signatures.push(signature);
        signatures.len() - 1
    }
}

pub fn function_pointer_info(id: usize) -> FunctionPointerSignature {
    FUNCTION_POINTER_TYPES
        .lock()
        .expect("function pointer type registry")[id]
        .clone()
}

fn is_c_abi_scalar(ty: Type) -> bool {
    matches!(
        ty,
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
    )
}

fn is_c_abi_parameter(ty: Type, structs: &[RynStruct]) -> bool {
    is_c_abi_scalar(ty)
        || matches!(ty, Type::Char | Type::RawPointer(_))
        || is_c_abi_record(ty, structs)
}

fn signature_has_record(signature: &FunctionPointerSignature) -> bool {
    signature
        .parameters
        .iter()
        .chain(signature.result.iter())
        .any(|ty| matches!(ty, Type::Struct(_)))
}

fn is_c_abi_record(ty: Type, structs: &[RynStruct]) -> bool {
    matches!(ty, Type::Struct(id)
        if structs[id].repr_c
            && (structs[id].fields.len() == 1
                && is_c_abi_record_member(structs[id].fields[0].ty)
                || c_abi_packed_record_layout(id, structs).is_some()))
}

fn c_abi_packed_record_layout(
    id: usize,
    structs: &[RynStruct],
) -> Option<(usize, Vec<(usize, Type)>)> {
    let fields = &structs[id].fields;
    if fields.is_empty() {
        return None;
    }
    let mut offset = 0_usize;
    let mut aggregate_align = 1_usize;
    let mut layout = Vec::with_capacity(fields.len());
    for field in fields {
        let (size, align) = c_abi_scalar_field_layout(field.ty)?;
        offset = align_up(offset, align);
        layout.push((offset, field.ty));
        offset = offset.checked_add(size)?;
        aggregate_align = aggregate_align.max(align);
    }
    let size = align_up(offset, aggregate_align);
    matches!(size, 1 | 2 | 4 | 8).then_some((size, layout))
}

fn c_abi_scalar_field_layout(ty: Type) -> Option<(usize, usize)> {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => Some((1, 1)),
        Type::I16 | Type::U16 => Some((2, 2)),
        Type::I32 | Type::U32 | Type::Char => Some((4, 4)),
        Type::F32 => Some((4, 4)),
        Type::I64 | Type::U64 | Type::F64 => Some((8, 8)),
        _ => None,
    }
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.saturating_add(alignment - 1) / alignment * alignment
}

fn is_c_abi_record_member(ty: Type) -> bool {
    matches!(
        ty,
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
    )
}

use std::sync::Mutex;

static VEC_ELEMS: Mutex<Vec<Type>> = Mutex::new(Vec::new());

pub fn intern_vec_elem(elem: Type) -> usize {
    let mut elems = VEC_ELEMS.lock().expect("vec element registry");
    if let Some(index) = elems.iter().position(|known| *known == elem) {
        index
    } else {
        elems.push(elem);
        elems.len() - 1
    }
}

pub fn vec_elem(id: usize) -> Type {
    VEC_ELEMS
        .lock()
        .expect("vec element registry")
        .get(id)
        .copied()
        .expect("vec element id")
}

static ARRAYS: Mutex<Vec<(Type, usize)>> = Mutex::new(Vec::new());

pub fn intern_array(element: Type, length: usize) -> usize {
    let mut arrays = ARRAYS.lock().expect("array type registry");
    if let Some(index) = arrays.iter().position(|known| *known == (element, length)) {
        index
    } else {
        arrays.push((element, length));
        arrays.len() - 1
    }
}

pub fn array_info(id: usize) -> (Type, usize) {
    ARRAYS.lock().expect("array type registry")[id]
}

static MAP_TYPES: Mutex<Vec<(Type, Type)>> = Mutex::new(Vec::new());

static POINTER_TYPES: Mutex<Vec<Type>> = Mutex::new(Vec::new());

pub fn intern_pointer_target(target: Type) -> usize {
    let mut targets = POINTER_TYPES.lock().expect("pointer type registry");
    if let Some(index) = targets.iter().position(|known| *known == target) {
        index
    } else {
        targets.push(target);
        targets.len() - 1
    }
}

pub fn pointer_target(id: usize) -> Type {
    POINTER_TYPES.lock().expect("pointer type registry")[id]
}

pub fn intern_map(key: Type, value: Type) -> usize {
    let mut maps = MAP_TYPES.lock().expect("Map type registry");
    if let Some(index) = maps.iter().position(|known| *known == (key, value)) {
        index
    } else {
        maps.push((key, value));
        maps.len() - 1
    }
}

pub fn map_info(id: usize) -> (Type, Type) {
    MAP_TYPES.lock().expect("Map type registry")[id]
}

fn contains_custom_drop(
    ty: Type,
    structs: &[RynStruct],
    enums: &[RynEnum],
    seen_structs: &mut HashSet<usize>,
    seen_enums: &mut HashSet<usize>,
    depth: usize,
) -> bool {
    if depth > MAX_STRUCT_NESTING {
        return false;
    }
    match ty {
        Type::Struct(id) => {
            if structs[id].drop_function.is_some() {
                return true;
            }
            seen_structs.insert(id)
                && structs[id].fields.iter().any(|field| {
                    contains_custom_drop(
                        field.ty,
                        structs,
                        enums,
                        seen_structs,
                        seen_enums,
                        depth + 1,
                    )
                })
        }
        Type::Array(id) => contains_custom_drop(
            array_info(id).0,
            structs,
            enums,
            seen_structs,
            seen_enums,
            depth + 1,
        ),
        Type::Vec(id) => contains_custom_drop(
            vec_elem(id),
            structs,
            enums,
            seen_structs,
            seen_enums,
            depth + 1,
        ),
        Type::Map(id) => {
            let (key, value) = map_info(id);
            contains_custom_drop(key, structs, enums, seen_structs, seen_enums, depth + 1)
                || contains_custom_drop(value, structs, enums, seen_structs, seen_enums, depth + 1)
        }
        Type::Set(id) => contains_custom_drop(
            map_info(id).0,
            structs,
            enums,
            seen_structs,
            seen_enums,
            depth + 1,
        ),
        Type::Enum(id) => {
            seen_enums.insert(id)
                && enums[id].variants.iter().any(|variant| {
                    variant.fields.iter().any(|field| {
                        contains_custom_drop(
                            *field,
                            structs,
                            enums,
                            seen_structs,
                            seen_enums,
                            depth + 1,
                        )
                    })
                })
        }
        _ => false,
    }
}

fn custom_drop_struct_fields_only(
    ty: Type,
    structs: &[RynStruct],
    enums: &[RynEnum],
    depth: usize,
) -> bool {
    if depth > MAX_STRUCT_NESTING {
        return false;
    }
    let Type::Struct(id) = ty else {
        return false;
    };
    if structs[id].drop_function.is_some() {
        return true;
    }
    structs[id].fields.iter().all(|field| {
        let nested_custom_drop = contains_custom_drop(
            field.ty,
            structs,
            enums,
            &mut HashSet::new(),
            &mut HashSet::new(),
            depth + 1,
        );
        if !nested_custom_drop {
            return true;
        }
        match field.ty {
            Type::Struct(_) => custom_drop_struct_fields_only(field.ty, structs, enums, depth + 1),
            Type::Vec(_) => custom_drop_vec_only(field.ty, structs, enums),
            Type::Map(_) => custom_drop_map_only(field.ty, structs, enums),
            _ => false,
        }
    })
}

fn custom_drop_array_only(ty: Type, structs: &[RynStruct], enums: &[RynEnum]) -> bool {
    match ty {
        Type::Array(id) => {
            let element = array_info(id).0;
            custom_drop_array_only(element, structs, enums)
                || custom_drop_struct_fields_only(element, structs, enums, 0)
        }
        _ => false,
    }
}

fn custom_drop_vec_only(ty: Type, structs: &[RynStruct], enums: &[RynEnum]) -> bool {
    let Type::Vec(id) = ty else {
        return false;
    };
    let element = vec_elem(id);
    contains_custom_drop(
        element,
        structs,
        enums,
        &mut HashSet::new(),
        &mut HashSet::new(),
        0,
    ) && custom_drop_struct_fields_only(element, structs, enums, 0)
}

fn custom_drop_map_only(ty: Type, structs: &[RynStruct], enums: &[RynEnum]) -> bool {
    let Type::Map(id) = ty else {
        return false;
    };
    let value = map_info(id).1;
    contains_custom_drop(
        value,
        structs,
        enums,
        &mut HashSet::new(),
        &mut HashSet::new(),
        0,
    ) && custom_drop_struct_fields_only(value, structs, enums, 0)
}

fn map_value_struct(ty: Type) -> Option<usize> {
    let Type::Map(id) = ty else {
        return None;
    };
    match map_info(id).1 {
        Type::Struct(struct_id) => Some(struct_id),
        _ => None,
    }
}

fn map_payload_type_supported(ty: Type, structs: &[RynStruct], depth: usize) -> bool {
    if depth > MAX_STRUCT_NESTING {
        return false;
    }
    match ty {
        Type::Str | Type::Slice(_) => false,
        Type::Struct(id) => {
            structs[id].drop_function.is_none()
                && structs[id]
                    .fields
                    .iter()
                    .all(|field| map_payload_type_supported(field.ty, structs, depth + 1))
        }
        Type::Array(id) => map_payload_type_supported(array_info(id).0, structs, depth + 1),
        Type::Vec(id) => map_payload_type_supported(vec_elem(id), structs, depth + 1),
        Type::Map(id) => {
            let (key, value) = map_info(id);
            map_payload_type_supported(key, structs, depth + 1)
                && map_payload_type_supported(value, structs, depth + 1)
        }
        Type::Set(id) => map_payload_type_supported(map_info(id).0, structs, depth + 1),
        _ => true,
    }
}

fn enum_payload_struct_supported(ty: Type, structs: &[RynStruct], depth: usize) -> bool {
    if depth > MAX_STRUCT_NESTING {
        return false;
    }
    match ty {
        Type::Str | Type::Slice(_) => false,
        Type::Struct(id) => {
            structs[id].drop_function.is_none()
                && structs[id]
                    .fields
                    .iter()
                    .all(|field| enum_payload_struct_supported(field.ty, structs, depth + 1))
        }
        Type::Array(id) => enum_payload_struct_supported(array_info(id).0, structs, depth + 1),
        Type::Vec(id) => enum_payload_struct_supported(vec_elem(id), structs, depth + 1),
        Type::Map(id) => {
            let (key, value) = map_info(id);
            enum_payload_struct_supported(key, structs, depth + 1)
                && enum_payload_struct_supported(value, structs, depth + 1)
        }
        Type::Set(id) => enum_payload_struct_supported(map_info(id).0, structs, depth + 1),
        _ => true,
    }
}

fn map_struct_value_supported(ty: Type, structs: &[RynStruct], enums: &[RynEnum]) -> bool {
    map_value_struct(ty).is_none_or(|struct_id| {
        let value_type = Type::Struct(struct_id);
        let has_custom_drop = contains_custom_drop(
            value_type,
            structs,
            enums,
            &mut HashSet::new(),
            &mut HashSet::new(),
            0,
        );
        structs[struct_id].drop_function.is_some()
            || (has_custom_drop && custom_drop_struct_fields_only(value_type, structs, enums, 0))
            || map_payload_type_supported(value_type, structs, 0)
    })
}

#[derive(Clone, Debug)]
pub struct RynArmBinding {
    pub slot: usize,
    pub ty: Type,
    pub field_index: usize,
}

#[derive(Clone, Debug)]
pub struct RynMatchArm {
    pub variant: Option<usize>,
    pub bindings: Vec<RynArmBinding>,
    pub body: IrExpression,
}

#[derive(Clone, Debug)]
pub enum IrExpression {
    Integer(i128, Type),
    Float(f64, Type),
    String(String),
    Character(char),
    Boolean(bool),
    Local {
        slot: usize,
        ty: Type,
        span: Span,
    },
    Move {
        slot: usize,
        ty: Type,
    },
    StructValue {
        struct_id: usize,
        fields: Vec<(usize, IrExpression)>,
    },
    Field {
        value: Box<IrExpression>,
        struct_id: usize,
        field_index: usize,
        ty: Type,
        span: Span,
    },
    ReferenceField {
        pointer: Box<IrExpression>,
        struct_id: usize,
        field_index: usize,
        ty: Type,
        span: Span,
        /// Load owned handles without cloning them; the caller only reads or
        /// mutates the stored value in place and never takes ownership.
        borrowed: bool,
    },
    ValueAddress {
        value: Box<IrExpression>,
        ty: Type,
        pointer_type: Type,
        span: Span,
    },
    Call {
        target: IrCallTarget,
        arguments: Vec<IrExpression>,
        return_type: Type,
    },
    If {
        condition: Box<IrExpression>,
        then_value: Box<IrExpression>,
        else_value: Box<IrExpression>,
        ty: Type,
    },
    EnumMatch {
        value: Box<IrExpression>,
        enum_id: usize,
        arms: Vec<RynMatchArm>,
        ty: Type,
    },
    Propagate {
        value: Box<IrExpression>,
        input_enum: usize,
        output_enum: usize,
        success_type: Type,
        failure_type: Option<Type>,
        success_variant: usize,
        failure_variant: usize,
        output_failure_variant: usize,
    },
    ArrayValue(Vec<IrExpression>),
    /// One evaluated element, replicated `length` times. The element is copyable.
    ArrayRepeat {
        value: Box<IrExpression>,
        length: usize,
    },
    ArrayAsSlice {
        array: Box<IrExpression>,
        slice_id: usize,
        length: usize,
    },
    ArrayIndex {
        array: Box<IrExpression>,
        index: Box<IrExpression>,
        ty: Type,
        length: usize,
    },
    SliceIndex {
        slice: Box<IrExpression>,
        index: Box<IrExpression>,
        ty: Type,
    },
    SliceElementAddress {
        slice: Box<IrExpression>,
        index: Box<IrExpression>,
        ty: Type,
        span: Span,
    },
    AddressOf {
        slot: usize,
        ty: Type,
        pointer_type: Type,
        span: Span,
    },
    Dereference {
        pointer: Box<IrExpression>,
        ty: Type,
    },
    StringFindOption {
        value: Box<IrExpression>,
        enum_id: usize,
    },
    StringAsStr(Box<IrExpression>),
    Binary {
        op: BinaryOp,
        left: Box<IrExpression>,
        right: Box<IrExpression>,
        ty: Type,
    },
    Negate(Box<IrExpression>, Type),
    Not(Box<IrExpression>),
    BitNot(Box<IrExpression>, Type),
    FunctionAddress {
        function: usize,
        ty: Type,
    },
    Cast {
        value: Box<IrExpression>,
        source: Type,
        target: Type,
    },
}

#[derive(Clone, Debug)]
pub enum IrStatement {
    Block(Vec<IrStatement>),
    Drop {
        slots: Vec<(usize, Type)>,
    },
    Let {
        slot: usize,
        ty: Type,
        value: IrExpression,
        span: Span,
    },
    Assign {
        slot: usize,
        ty: Type,
        value: IrExpression,
        span: Span,
    },
    ArrayAssign {
        slot: usize,
        element: Type,
        length: usize,
        index: IrExpression,
        value: IrExpression,
        span: Span,
    },
    DereferenceAssign {
        pointer: IrExpression,
        ty: Type,
        value: IrExpression,
    },
    FieldAssign {
        slot: usize,
        ty: Type,
        value: IrExpression,
    },
    ReferenceFieldAssign {
        pointer: IrExpression,
        struct_id: usize,
        field_index: usize,
        ty: Type,
        value: IrExpression,
    },
    Print {
        value: IrExpression,
        ty: Type,
    },
    PrintTemplate(Vec<IrPrintPart>),
    Call {
        target: IrCallTarget,
        arguments: Vec<IrExpression>,
    },
    If {
        condition: IrExpression,
        then_body: Vec<IrStatement>,
        else_body: Vec<IrStatement>,
    },
    While {
        /// Temporaries the condition needs, evaluated on every iteration.
        setup: Vec<IrStatement>,
        condition: IrExpression,
        body: Vec<IrStatement>,
    },
    For {
        slot: usize,
        end_slot: usize,
        ty: Type,
        start: IrExpression,
        end: IrExpression,
        // `start..=end` includes `end`; codegen stops before incrementing past it.
        inclusive: bool,
        body: Vec<IrStatement>,
    },
    Break,
    Continue,
    Return {
        value: Option<IrExpression>,
    },
}

/// Runs `setup` (temporaries hoisted out of an expression) before `statement`.
fn with_setup(mut setup: Vec<IrStatement>, statement: IrStatement) -> IrStatement {
    if setup.is_empty() {
        statement
    } else {
        setup.push(statement);
        IrStatement::Block(setup)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum IrCallTarget {
    Function(usize),
    ArgumentCount,
    Argument,
    String(StringOp),
    Filesystem(FilesystemOp),
    TryReadFile(usize),
    ReadFileResult(usize),
    System(SystemOp),
    IndirectFunctionPointer(usize),
    Vec(VecOp, usize),
    VecSlice(usize),
    SliceLen,
    Map(MapOp, usize),
    Set(MapOp, usize),
    EnumNew {
        enum_id: usize,
        tag: usize,
    },
    EnumPredicate(EnumPredicate, usize),
    EnumUnwrapOr {
        enum_id: usize,
        value_type: Type,
    },
    EnumUnwrap {
        enum_id: usize,
        value_type: Type,
        message: bool,
        success_tag: usize,
        failure: SystemOp,
    },
    VecGetOption {
        elem_id: usize,
        option_id: usize,
        pop: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumPredicate {
    IsSome,
    IsNone,
    IsOk,
    IsErr,
}

#[derive(Clone, Debug)]
pub enum IrPrintPart {
    Text(String),
    Value { value: IrExpression, ty: Type },
}

#[derive(Clone, Copy, Debug)]
pub enum LocalType {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Ptr,
    OwnedPtr,
}

#[derive(Clone, Copy, Debug)]
pub struct LocalBinding {
    pub slot: usize,
    pub ty: Type,
}

#[derive(Debug)]
pub struct RynFunction {
    pub parameters: Vec<LocalBinding>,
    pub external_symbol: Option<String>,
    pub return_type: Option<Type>,
    pub reference_return_parameter: Option<usize>,
    pub return_value: Option<IrExpression>,
    pub statements: Vec<IrStatement>,
    pub local_types: Vec<LocalType>,
    pub owned_slot_types: Vec<Option<Type>>,
    pub addressed_slot_types: Vec<Option<Type>>,
}

#[derive(Debug)]
pub struct RynIr {
    pub functions: Vec<RynFunction>,
    pub main_index: usize,
    pub structs: Vec<RynStruct>,
    pub enums: Vec<RynEnum>,
    pub instant_drop: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct RynEnum {
    pub name: String,
    pub variants: Vec<RynEnumVariant>,
}

#[derive(Clone, Debug)]
pub struct RynEnumVariant {
    pub name: String,
    pub fields: Vec<Type>,
}

#[derive(Clone, Debug)]
pub struct RynStruct {
    pub name: String,
    pub fields: Vec<RynStructField>,
    pub slot_count: usize,
    pub repr_c: bool,
    pub drop_function: Option<usize>,
    pub derives_clone: bool,
    pub derives_hash: bool,
    pub module_path: String,
}

#[derive(Clone, Debug)]
pub struct RynStructField {
    pub name: String,
    pub ty: Type,
    pub slot_offset: usize,
    pub public: bool,
}

#[derive(Clone, Copy)]
struct Binding {
    slot: usize,
    ty: Type,
    mutable: bool,
}

// A `defer` body with the names that were visible where it was written. It is lowered again
// at every place it runs, always with that same name environment.
#[derive(Clone)]
struct DeferredBody {
    body: Vec<Statement>,
    names: HashMap<String, Binding>,
}

// An exit statement preceded by the `defer` bodies that it leaves; a plain exit when none.
fn exit_after(leaving: Vec<IrStatement>, exit: IrStatement) -> IrStatement {
    if leaving.is_empty() {
        return exit;
    }
    let mut statements = leaving;
    statements.push(exit);
    IrStatement::Block(statements)
}

type LoweredMethod = Result<Option<(IrCallTarget, Vec<IrExpression>, Option<Type>)>, Diagnostic>;

#[derive(Clone)]
struct FunctionSignature {
    target: IrCallTarget,
    parameters: Vec<Type>,
    return_type: Option<Type>,
    is_destructor: bool,
}

// Associated functions and instance methods share the source name inside an
// `extend` block, but are called through different syntactic forms. Keep the
// static function at the ordinary key (`Type::name`) and index receiver
// methods separately so both can coexist without introducing overload
// resolution for ordinary functions.
fn function_signature_key(function: &crate::ast::Function) -> String {
    if function
        .parameters
        .first()
        .is_some_and(|parameter| parameter.name == "self")
        && !function.name.starts_with("String::")
    {
        format!("{}#method", function.name)
    } else {
        function.name.clone()
    }
}

fn enum_extract_payload_supported(ty: Type, structs: &[RynStruct], enums: &[RynEnum]) -> bool {
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
        | Type::Enum(_) => true,
        Type::Struct(_) | Type::Array(_) => {
            let mut seen_structs = HashSet::new();
            let mut seen_enums = HashSet::new();
            !contains_custom_drop(ty, structs, enums, &mut seen_structs, &mut seen_enums, 0)
        }
        _ => false,
    }
}

#[derive(Clone)]
struct ShapeRequirement {
    self_mutable: bool,
    parameters: Vec<Type>,
    return_type: Option<Type>,
    has_default: bool,
}

#[derive(Clone, Default)]
struct ShapeTable {
    shapes: HashMap<String, HashMap<String, ShapeRequirement>>,
    order: Vec<String>,
}

impl ShapeTable {
    fn get(&self, name: &str) -> Option<&HashMap<String, ShapeRequirement>> {
        self.shapes.get(name)
    }
}

pub fn analyze(program: Program) -> Result<RynIr, Diagnostic> {
    match analyze_with_recovery(program, false) {
        Ok(ir) => Ok(ir),
        Err(mut diagnostics) => Err(diagnostics.remove(0)),
    }
}

/// Analyzes a program and gathers independent declaration and function-body
/// errors where later analysis does not depend on the invalid declaration.
pub fn analyze_recovering(program: Program) -> Result<RynIr, Vec<Diagnostic>> {
    analyze_with_recovery(program, true)
}

fn analyze_with_recovery(
    mut program: Program,
    recover_errors: bool,
) -> Result<RynIr, Vec<Diagnostic>> {
    program = crate::generics::monomorphize(program).map_err(|diagnostic| vec![diagnostic])?;
    if !program.enums.iter().any(is_option_u64_ast) {
        program.enums.push(EnumDef {
            name: "$RynOption#FindU64".into(),
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Some".into(),
                    fields: vec![TypeName::U64],
                    span: Span::default(),
                },
                VariantDef {
                    name: "None".into(),
                    fields: Vec::new(),
                    span: Span::default(),
                },
            ],
            public: false,
            module_path: String::new(),
            span: Span::default(),
        });
    }
    if !program.enums.iter().any(is_option_string_ast) {
        program.enums.push(EnumDef {
            name: "$RynOption#FileString".into(),
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Some".into(),
                    fields: vec![TypeName::OwnedString],
                    span: Span::default(),
                },
                VariantDef {
                    name: "None".into(),
                    fields: Vec::new(),
                    span: Span::default(),
                },
            ],
            public: false,
            module_path: String::new(),
            span: Span::default(),
        });
    }
    for (option_name, payload) in [
        ("I8", TypeName::I8),
        ("I16", TypeName::I16),
        ("I32", TypeName::I32),
        ("I64", TypeName::I64),
        ("U8", TypeName::U8),
        ("U16", TypeName::U16),
        ("U32", TypeName::U32),
        ("U64", TypeName::U64),
        ("F32", TypeName::F32),
        ("F64", TypeName::F64),
        ("Bool", TypeName::Bool),
        ("Char", TypeName::Char),
    ] {
        if !program
            .enums
            .iter()
            .any(|definition| is_option_ast_payload(definition, &payload))
        {
            program.enums.push(EnumDef {
                name: format!("$RynOption#TryParse{option_name}"),
                type_parameters: Vec::new(),
                variants: vec![
                    VariantDef {
                        name: "Some".into(),
                        fields: vec![payload],
                        span: Span::default(),
                    },
                    VariantDef {
                        name: "None".into(),
                        fields: Vec::new(),
                        span: Span::default(),
                    },
                ],
                public: false,
                module_path: String::new(),
                span: Span::default(),
            });
        }
    }
    let primitive_types = [
        TypeName::I8,
        TypeName::I16,
        TypeName::I32,
        TypeName::I64,
        TypeName::U8,
        TypeName::U16,
        TypeName::U32,
        TypeName::U64,
        TypeName::F32,
        TypeName::F64,
        TypeName::OwnedString,
    ];
    for (result_index, result_type) in primitive_types.iter().enumerate() {
        for (error_index, error_type) in primitive_types.iter().enumerate() {
            let name = format!("$RynResult#{result_index}#{error_index}");
            if program
                .enums
                .iter()
                .any(|definition| definition.name == name)
            {
                continue;
            }
            program.enums.push(EnumDef {
                name,
                type_parameters: Vec::new(),
                variants: vec![
                    VariantDef {
                        name: "Ok".into(),
                        fields: vec![result_type.clone()],
                        span: Span::default(),
                    },
                    VariantDef {
                        name: "Err".into(),
                        fields: vec![error_type.clone()],
                        span: Span::default(),
                    },
                ],
                public: false,
                module_path: String::new(),
                span: Span::default(),
            });
        }
    }
    // Result specializations for parsing carry an owned error message.
    for (result_index, result_type) in [
        TypeName::I8,
        TypeName::I16,
        TypeName::I32,
        TypeName::I64,
        TypeName::U8,
        TypeName::U16,
        TypeName::U32,
        TypeName::U64,
        TypeName::F32,
        TypeName::F64,
    ]
    .iter()
    .enumerate()
    {
        let name = format!("$RynResult#Parse{result_index}String");
        if program
            .enums
            .iter()
            .any(|definition| definition.name == name)
        {
            continue;
        }
        program.enums.push(EnumDef {
            name,
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Ok".into(),
                    fields: vec![result_type.clone()],
                    span: Span::default(),
                },
                VariantDef {
                    name: "Err".into(),
                    fields: vec![TypeName::OwnedString],
                    span: Span::default(),
                },
            ],
            public: false,
            module_path: String::new(),
            span: Span::default(),
        });
    }
    let mut struct_ids = HashMap::new();
    let mut enum_ids = HashMap::new();
    let mut declaration_errors = Vec::new();
    let mut struct_name_concepts = HashSet::<&str>::new();
    for (index, definition) in program.structs.iter().enumerate() {
        if struct_ids.insert(definition.name.clone(), index).is_some() {
            let diagnostic = diag(
                "R0220",
                format!("duplicate structure `{}`", definition.name),
                definition.span,
            )
            .with_help("give each structure a unique name");
            if recover_errors {
                declaration_errors.push(diagnostic);
            } else {
                return Err(vec![diagnostic]);
            }
        } else {
            struct_name_concepts.insert(definition.name.as_str());
        }
    }
    for (index, definition) in program.enums.iter().enumerate() {
        if enum_ids.insert(definition.name.clone(), index).is_some() {
            let diagnostic = diag(
                "R0238",
                format!("duplicate enum `{}`", definition.name),
                definition.span,
            )
            .with_help("give each enum a unique name");
            if recover_errors {
                declaration_errors.push(diagnostic);
            } else {
                return Err(vec![diagnostic]);
            }
        } else if struct_name_concepts.contains(definition.name.as_str()) {
            let diagnostic = diag(
                "R0238",
                format!(
                    "enum `{}` has the same name as a structure",
                    definition.name
                ),
                definition.span,
            )
            .with_help("give each type a unique name");
            if recover_errors {
                declaration_errors.push(diagnostic);
            } else {
                return Err(vec![diagnostic]);
            }
        }
    }
    // Also reject a struct declared after a same-named enum.
    for definition in &program.structs {
        if enum_ids.contains_key(&definition.name) {
            let diagnostic = diag(
                "R0238",
                format!(
                    "structure `{}` has the same name as an enum",
                    definition.name
                ),
                definition.span,
            )
            .with_help("give each type a unique name");
            if recover_errors {
                declaration_errors.push(diagnostic);
            } else {
                return Err(vec![diagnostic]);
            }
        }
    }
    for import in &program.uses {
        let module_path = import.path.join("::");
        let Some(alias) = import.path.last() else {
            continue;
        };
        for (index, definition) in program
            .structs
            .iter()
            .enumerate()
            .filter(|(_, definition)| definition.public && definition.module_path == module_path)
        {
            let alias_name = format!("{alias}::{}", definition.name);
            if struct_ids
                .insert(alias_name.clone(), index)
                .is_some_and(|existing| existing != index)
            {
                return Err(vec![diag(
                    "R0424",
                    format!("module alias `{alias_name}` refers to multiple types"),
                    import.span,
                )]);
            }
        }
        for (index, definition) in
            program.enums.iter().enumerate().filter(|(_, definition)| {
                definition.public && definition.module_path == module_path
            })
        {
            let alias_name = format!("{alias}::{}", definition.name);
            if enum_ids
                .insert(alias_name.clone(), index)
                .is_some_and(|existing| existing != index)
            {
                return Err(vec![diag(
                    "R0424",
                    format!("module alias `{alias_name}` refers to multiple types"),
                    import.span,
                )]);
            }
        }
    }
    let mut type_names = struct_ids
        .keys()
        .chain(enum_ids.keys())
        .cloned()
        .collect::<HashSet<_>>();
    for alias in &program.type_aliases {
        let builtin = matches!(
            alias.name.as_str(),
            "i8" | "i16"
                | "i32"
                | "i64"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "f32"
                | "f64"
                | "str"
                | "String"
                | "char"
                | "bool"
                | "Vec"
                | "Map"
                | "HashMap"
                | "Set"
                | "Option"
                | "Result"
        );
        if builtin || !type_names.insert(alias.name.clone()) {
            let diagnostic = diag(
                "R0241",
                format!(
                    "type alias `{}` conflicts with an existing type",
                    alias.name
                ),
                alias.span,
            )
            .with_help("give each declared type and alias a unique name");
            if recover_errors {
                declaration_errors.push(diagnostic);
            } else {
                return Err(vec![diagnostic]);
            }
        }
    }
    for alias in &program.type_aliases {
        if !alias.type_parameters.is_empty() {
            continue;
        }
        if let Err(error) = resolve_type_name(&alias.ty, &struct_ids, &enum_ids) {
            if recover_errors {
                declaration_errors.push(error);
            } else {
                return Err(vec![error]);
            }
        }
    }
    if recover_errors {
        let mut function_names = HashSet::new();
        for function in &program.functions {
            if !function_names.insert(function_signature_key(function)) {
                declaration_errors.push(
                    diag(
                        "R0201",
                        format!("duplicate function `{}`", function.name),
                        function.span,
                    )
                    .with_help(
                        "give each function a unique name; function overloading is not supported",
                    ),
                );
            }
        }
        declaration_errors.sort_by_key(|diagnostic| diagnostic.span.start);
        if !declaration_errors.is_empty() {
            return Err(declaration_errors);
        }

        let mut struct_errors =
            collect_struct_declaration_errors(&program.structs, &struct_ids, &enum_ids);
        if !struct_errors.is_empty() {
            struct_errors.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(struct_errors);
        }

        let mut signature_errors =
            collect_function_signature_errors(&program.functions, &struct_ids, &enum_ids);
        if !signature_errors.is_empty() {
            signature_errors.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(signature_errors);
        }

        let mut layout_cycle_errors =
            collect_struct_layout_cycle_errors(&program.structs, &struct_ids);
        if !layout_cycle_errors.is_empty() {
            layout_cycle_errors.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(layout_cycle_errors);
        }
    }
    let mut structs = resolve_struct_layouts(&program.structs, &struct_ids, &enum_ids)
        .map_err(|error| vec![error])?;
    let enums = resolve_enum_layouts(&program.enums, &structs, &struct_ids, &enum_ids)
        .map_err(|error| vec![error])?;
    for definition in &structs {
        for field in &definition.fields {
            validate_array_supported(field.ty, &structs, Span::default())
                .map_err(|error| vec![error])?;
            validate_vec_struct_elements(
                field.ty,
                &structs,
                program
                    .structs
                    .iter()
                    .find(|source| source.name == definition.name)
                    .map_or(Span::default(), |source| source.span),
            )
            .map_err(|error| vec![error])?;
        }
    }
    for definition in &enums {
        for variant in &definition.variants {
            for field in &variant.fields {
                validate_array_supported(*field, &structs, Span::default())
                    .map_err(|error| vec![error])?;
                validate_vec_struct_elements(*field, &structs, Span::default())
                    .map_err(|error| vec![error])?;
            }
        }
    }
    let mut signatures = HashMap::new();
    let mut main_index = None;
    let mut function_module_paths = program
        .functions
        .iter()
        .map(|function| function.module_path.clone())
        .collect::<Vec<_>>();
    let mut function_visibility = program
        .functions
        .iter()
        .map(|function| function.public)
        .collect::<Vec<_>>();
    for (index, function) in program.functions.iter().enumerate() {
        let signature_key = function_signature_key(function);
        if signatures.contains_key(&signature_key) {
            return Err(vec![
                diag(
                    "R0201",
                    format!("duplicate function `{}`", function.name),
                    function.span,
                )
                .with_help(
                    "give each function a unique name; function overloading is not supported",
                ),
            ]);
        }
        let signature_result = (|| {
            let namespace = declaration_namespace(&function.name, &function.module_path);
            let mut parameters = Vec::with_capacity(function.parameters.len());
            for parameter in &function.parameters {
                let ty = resolve_type_name_scoped(
                    &parameter.ty,
                    &struct_ids,
                    &enum_ids,
                    namespace.as_deref(),
                )?;
                validate_array_supported(ty, &structs, parameter.span)?;
                parameters.push(ty);
            }
            let return_type = function
                .return_type
                .as_ref()
                .map(|name| {
                    let ty = resolve_type_name_scoped(
                        name,
                        &struct_ids,
                        &enum_ids,
                        namespace.as_deref(),
                    )?;
                    validate_array_supported(ty, &structs, function.span)?;
                    Ok(ty)
                })
                .transpose()?;
            if function.extern_c
                && (parameters
                    .iter()
                    .any(|ty| !is_c_abi_parameter(*ty, &structs))
                    || return_type.is_some_and(|ty| {
                        !is_c_abi_scalar(ty)
                            && !matches!(ty, Type::RawPointer(_))
                            && !is_c_abi_record(ty, &structs)
                    }))
            {
                return Err(diag(
                    "R0247",
                    "`extern \"C\"` supports scalar values, one-field scalar records, and packed integer/float `#[repr(C)]` records of 1, 2, 4, or 8 bytes",
                    function.span,
                )
                .with_help("use scalar values, a one-field scalar record, or an integer record whose C layout is 1, 2, 4, or 8 bytes"));
            }
            if function.extern_c && function.name == "main" {
                return Err(diag(
                    "R0247",
                    "`main` must be defined by the Ryn program, not imported from C",
                    function.span,
                ));
            }
            if function.name == "main"
                && (!function.parameters.is_empty()
                    || return_type.is_some_and(|return_type| return_type != Type::I32))
            {
                return Err(diag(
                    "R0208",
                    "`main` must take no parameters and return either no value or `i32`",
                    function.span,
                )
                .with_help("use `fun main() { ... }` or `fun main() -> i32 { ... }`; move reusable work into another function"));
            }
            Ok(FunctionSignature {
                target: IrCallTarget::Function(index),
                parameters,
                return_type,
                is_destructor: false,
            })
        })();
        let signature = signature_result.map_err(|diagnostic| vec![diagnostic])?;
        if function.name == "main" {
            main_index = Some(index);
        }
        signatures.insert(signature_key, signature);
    }
    for import in &program.uses {
        let module_path = import.path.join("::");
        let Some(alias) = import.path.last() else {
            continue;
        };
        let prefix = format!("{module_path}::");
        for function in program
            .functions
            .iter()
            .filter(|function| function.public && function.module_path == module_path)
        {
            let Some(suffix) = function.name.strip_prefix(&prefix) else {
                continue;
            };
            let alias_name = format!("{alias}::{suffix}");
            let function_key = function_signature_key(function);
            let signature = signatures[&function_key].clone();
            let alias_key = if function_key.ends_with("#method") {
                format!("{alias_name}#method")
            } else {
                alias_name.clone()
            };
            if let Some(existing) = signatures.get(&alias_key) {
                if !matches!(
                    (existing.target, signature.target),
                    (IrCallTarget::Function(left), IrCallTarget::Function(right)) if left == right
                ) {
                    return Err(vec![diag(
                        "R0424",
                        format!("module alias `{alias_name}` refers to multiple symbols"),
                        import.span,
                    )]);
                }
            } else {
                signatures.insert(alias_key, signature);
            }
        }
    }
    let mut destructor_owners = HashSet::new();
    for (struct_id, definition) in program.structs.iter().enumerate() {
        let Some(drop_name) = &definition.drop_function else {
            continue;
        };
        let Some(signature) = signatures.get(drop_name) else {
            return Err(vec![diag(
                "R0255",
                format!("destructor function `{drop_name}` was not found"),
                definition.span,
            )]);
        };
        let IrCallTarget::Function(function_id) = signature.target else {
            return Err(vec![diag(
                "R0255",
                "a destructor must name a Ryn function",
                definition.span,
            )]);
        };
        let function = &program.functions[function_id];
        if function.public
            || function.extern_c
            || !function.type_parameters.is_empty()
            || signature.parameters != [Type::Struct(struct_id)]
            || signature.return_type.is_some()
            || !destructor_owners.insert(function_id)
        {
            return Err(vec![diag(
                "R0255",
                "a destructor must be a private, non-generic `fun drop(value: ThisStruct)` with no return value, and can belong to only one struct",
                definition.span,
            )]);
        }
        if definition.repr_c
            || structs[struct_id]
                .fields
                .first()
                .is_none_or(|field| !matches!(field.ty, Type::RawPointer(_)))
            || structs[struct_id].fields.iter().any(|field| {
                !matches!(
                    field.ty,
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
                        | Type::RawPointer(_)
                )
            })
        {
            return Err(vec![diag(
                "R0255",
                "custom destructors currently require non-`repr(C)` structs with scalar or raw-pointer fields",
                definition.span,
            )]);
        }
        structs[struct_id].drop_function = Some(function_id);
        signatures.get_mut(drop_name).unwrap().is_destructor = true;
    }
    for definition in &structs {
        for field in &definition.fields {
            if !map_struct_value_supported(field.ty, &structs, &enums) {
                return Err(vec![diag(
                    "R0244",
                    "Map struct values must support a generated clone/drop plan; this value type has no supported Map layout",
                    program
                        .structs
                        .iter()
                        .find(|source| source.name == definition.name)
                        .map_or(Span::default(), |source| source.span),
                )]);
            }
            let mut seen_structs = HashSet::new();
            let mut seen_enums = HashSet::new();
            if contains_custom_drop(
                field.ty,
                &structs,
                &enums,
                &mut seen_structs,
                &mut seen_enums,
                0,
            ) && !custom_drop_struct_fields_only(field.ty, &structs, &enums, 0)
                && !custom_drop_vec_only(field.ty, &structs, &enums)
                && !custom_drop_map_only(field.ty, &structs, &enums)
            {
                return Err(vec![diag(
                    "R0255",
                    "custom-destructor values may be nested through ordinary struct fields, move-only Vec elements, and move-only Map values; arrays and enum payloads remain restricted",
                    program
                        .structs
                        .iter()
                        .find(|source| source.name == definition.name)
                        .map_or(Span::default(), |source| source.span),
                )]);
            }
        }
    }
    for definition in &enums {
        if !definition.name.starts_with("$RynOption#")
            && definition
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .any(|ty| !map_struct_value_supported(*ty, &structs, &enums))
        {
            return Err(vec![diag(
                "R0244",
                "Map struct values must support a generated clone/drop plan; this value type has no supported Map layout",
                Span::default(),
            )]);
        }
        if !definition.name.starts_with("$RynOption#")
            && definition
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .any(|ty| {
                    let mut seen_structs = HashSet::new();
                    let mut seen_enums = HashSet::new();
                    contains_custom_drop(
                        *ty,
                        &structs,
                        &enums,
                        &mut seen_structs,
                        &mut seen_enums,
                        0,
                    )
                })
        {
            return Err(vec![diag(
                "R0255",
                "custom-destructor values cannot be stored in enum payloads yet",
                Span::default(),
            )]);
        }
    }
    for function in &program.functions {
        let Some(signature) = signatures.get(&function_signature_key(function)) else {
            continue;
        };
        for ty in signature
            .parameters
            .iter()
            .copied()
            .chain(signature.return_type)
        {
            let mut seen_structs = HashSet::new();
            let mut seen_enums = HashSet::new();
            if contains_custom_drop(ty, &structs, &enums, &mut seen_structs, &mut seen_enums, 0)
                && !matches!(ty, Type::Struct(id) if structs[id].drop_function.is_some())
                && !custom_drop_array_only(ty, &structs, &enums)
                && !custom_drop_vec_only(ty, &structs, &enums)
                && !custom_drop_map_only(ty, &structs, &enums)
                && !custom_drop_struct_fields_only(ty, &structs, &enums, 0)
            {
                return Err(vec![diag(
                    "R0255",
                    "custom-destructor values may only be nested through ordinary struct fields; collections, arrays, and enum payloads remain unsupported",
                    function.span,
                )]);
            }
            if !map_struct_value_supported(ty, &structs, &enums) {
                return Err(vec![diag(
                    "R0244",
                    "Map struct values must support a generated clone/drop plan; this value type has no supported Map layout",
                    function.span,
                )]);
            }
        }
    }
    let Some(main_index) = main_index else {
        return Err(vec![
            diag("R0200", "program must define `fun main()`", Span::default())
                .with_help("add a top-level `fun main() { ... }` function"),
        ]);
    };
    let shape_table = build_shape_table(&program.shapes, &struct_ids, &enum_ids);
    for extend in &program.extends {
        let Some(shape_name) = &extend.as_shape else {
            continue;
        };
        let TypeName::Named(type_name, type_span) = &extend.type_name else {
            continue;
        };
        let Some(struct_id) = struct_ids.get(type_name).copied() else {
            return Err(vec![diag(
                "R0230",
                format!("`extend as` names an unknown structure `{type_name}`"),
                *type_span,
            )]);
        };
        if let Err(message) = struct_conforms_to_shape(
            &structs[struct_id],
            shape_name,
            &shape_table,
            &signatures,
            &structs,
        ) {
            return Err(vec![diag("R0450", message, extend.span)]);
        }
    }
    materialize_shape_defaults(
        &mut program.functions,
        &mut signatures,
        &mut function_module_paths,
        &mut function_visibility,
        &shape_table,
        &program.shapes,
        &structs,
        &struct_ids,
    );
    materialize_derived_clones(
        &mut program.functions,
        &mut signatures,
        &mut function_module_paths,
        &mut function_visibility,
        &structs,
        &struct_ids,
        &enum_ids,
    );
    let mut functions = Vec::with_capacity(program.functions.len());
    let mut diagnostics = Vec::new();
    for function in program.functions {
        if recover_errors {
            let parameter_errors = collect_duplicate_parameter_errors(&function);
            if !parameter_errors.is_empty() {
                diagnostics.extend(parameter_errors);
                continue;
            }
        }
        let analyzer = Analyzer::new(
            &signatures,
            &mut structs,
            &struct_ids,
            &enums,
            &enum_ids,
            &function_module_paths,
            &function_visibility,
            &shape_table,
        );
        if recover_errors {
            match analyzer.function_recovering(function) {
                Ok(function) => functions.push(function),
                Err(mut function_diagnostics) => diagnostics.append(&mut function_diagnostics),
            }
        } else {
            match analyzer.function(function) {
                Ok(function) => functions.push(function),
                Err(diagnostic) => return Err(vec![diagnostic]),
            }
        }
    }
    if !diagnostics.is_empty() {
        diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
        return Err(diagnostics);
    }
    let mut ir = RynIr {
        functions,
        main_index,
        structs,
        enums,
        instant_drop: None,
    };
    crate::guard::check(&mut ir).map_err(|error| vec![error])?;
    Ok(ir)
}

fn build_shape_table(
    shapes: &[ShapeDef],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
) -> ShapeTable {
    let mut table = ShapeTable::default();
    for shape in shapes {
        if table.shapes.contains_key(&shape.name) {
            continue;
        }
        let namespace = shape
            .module_path
            .rsplit_once("::")
            .map(|(parent, _)| parent)
            .or(Some(shape.module_path.as_str()))
            .filter(|_| !shape.module_path.is_empty());
        let namespace = namespace.map(|namespace| namespace.to_string());
        let namespace = namespace.as_deref();
        let mut requirements = HashMap::new();
        for method in &shape.methods {
            let mut parameter_types = Vec::new();
            let mut self_mutable = false;
            for (index, parameter) in method.parameters.iter().enumerate() {
                let resolved =
                    resolve_type_name_scoped(&parameter.ty, struct_ids, enum_ids, namespace);
                let Ok(resolved) = resolved else {
                    continue;
                };
                if index == 0 && parameter.name == "self" {
                    if let Type::Reference(_, mutable) = resolved {
                        self_mutable = mutable;
                    }
                    continue;
                }
                parameter_types.push(resolved);
            }
            let return_type = method.return_type.as_ref().and_then(|name| {
                resolve_type_name_scoped(name, struct_ids, enum_ids, namespace).ok()
            });
            let has_default = method.default_body.is_some() || method.default_value.is_some();
            requirements.insert(
                method.name.clone(),
                ShapeRequirement {
                    self_mutable,
                    parameters: parameter_types,
                    return_type,
                    has_default,
                },
            );
        }
        table.order.push(shape.name.clone());
        table.shapes.insert(shape.name.clone(), requirements);
    }
    table
}

fn method_signature_candidates(definition: &RynStruct, method: &str) -> Vec<String> {
    let mut candidates = vec![format!("{}::{method}#method", definition.name)];
    if !definition.module_path.is_empty() {
        candidates.push(format!(
            "{}::{}::{method}#method",
            definition.module_path, definition.name
        ));
    }
    candidates
}

fn struct_conforms_to_shape(
    definition: &RynStruct,
    shape_name: &str,
    table: &ShapeTable,
    signatures: &HashMap<String, FunctionSignature>,
    structs: &[RynStruct],
) -> Result<(), String> {
    let Some(requirements) = table.get(shape_name) else {
        return Err(format!("unknown shape `{shape_name}`"));
    };
    for (method_name, requirement) in requirements {
        if requirement.has_default {
            continue;
        }
        let mut found = None;
        for candidate in method_signature_candidates(definition, method_name) {
            if let Some(signature) = signatures.get(&candidate) {
                found = Some(signature.clone());
                break;
            }
        }
        let Some(signature) = found else {
            return Err(format!(
                "`{}` does not satisfy shape `{shape_name}`: missing method `{method_name}`",
                definition.name
            ));
        };
        let Some(self_parameter) = signature.parameters.first() else {
            return Err(format!(
                "`{}` method `{method_name}` must take a receiver",
                definition.name
            ));
        };
        let Type::Reference(target, actual_mutable) = self_parameter else {
            return Err(format!(
                "`{}` method `{method_name}` must take `self`",
                definition.name
            ));
        };
        if !matches!(pointer_target(*target), Type::Struct(id) if structs[id].name == definition.name)
        {
            return Err(format!(
                "`{}` method `{method_name}` must take a `{}` receiver",
                definition.name, definition.name
            ));
        }
        if requirement.self_mutable && !actual_mutable {
            return Err(format!(
                "`{}` method `{method_name}` must take `mut self` to satisfy `{shape_name}`",
                definition.name
            ));
        }
        if signature.parameters.len() - 1 != requirement.parameters.len() {
            return Err(format!(
                "`{}` method `{method_name}` has {} parameter(s) but `{shape_name}` requires {}",
                definition.name,
                signature.parameters.len() - 1,
                requirement.parameters.len()
            ));
        }
        for (index, expected) in requirement.parameters.iter().enumerate() {
            if signature.parameters[index + 1] != *expected {
                return Err(format!(
                    "`{}` method `{method_name}` parameter {} has type `{}` but `{shape_name}` requires `{}`",
                    definition.name,
                    index + 1,
                    type_name(signature.parameters[index + 1]),
                    type_name(*expected)
                ));
            }
        }
        if signature.return_type != requirement.return_type {
            return Err(format!(
                "`{}` method `{method_name}` return type does not match `{shape_name}`",
                definition.name
            ));
        }
    }
    Ok(())
}

fn struct_has_method(
    definition: &RynStruct,
    method: &str,
    signatures: &HashMap<String, FunctionSignature>,
) -> bool {
    method_signature_candidates(definition, method)
        .iter()
        .any(|candidate| signatures.contains_key(candidate))
}

#[allow(clippy::too_many_arguments)]
fn materialize_shape_defaults(
    functions: &mut Vec<crate::ast::Function>,
    signatures: &mut HashMap<String, FunctionSignature>,
    function_module_paths: &mut Vec<String>,
    function_visibility: &mut Vec<bool>,
    table: &ShapeTable,
    shapes: &[ShapeDef],
    structs: &[RynStruct],
    struct_ids: &HashMap<String, usize>,
) {
    let _ = struct_ids;
    for shape in shapes {
        let Some(requirements) = table.get(&shape.name) else {
            continue;
        };
        // Only types that satisfy the required methods receive the defaults.
        for (struct_index, definition) in structs.iter().enumerate() {
            if definition.name.starts_with('$') {
                continue;
            }
            if struct_conforms_to_shape(definition, &shape.name, table, signatures, structs)
                .is_err()
            {
                continue;
            }
            for (method_name, requirement) in requirements {
                if struct_has_method(definition, method_name, signatures) {
                    continue;
                }
                let Some(method) = shape
                    .methods
                    .iter()
                    .find(|method| &method.name == method_name)
                else {
                    continue;
                };
                let (default_body, default_value) =
                    match (&method.default_body, &method.default_value) {
                        (Some(body), _) => (body.clone(), None),
                        (None, Some(value)) => (Vec::new(), Some(value.clone())),
                        (None, None) => continue,
                    };
                let qualified = format!("{}::{}", definition.name, method_name);
                let struct_type = Type::Struct(struct_index);
                let self_type =
                    Type::Reference(intern_pointer_target(struct_type), requirement.self_mutable);
                let index = functions.len();
                functions.push(crate::ast::Function {
                    name: qualified.clone(),
                    extern_c: false,
                    external_symbol: None,
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    public: true,
                    module_path: definition.module_path.clone(),
                    parameters: vec![crate::ast::Parameter {
                        name: "self".into(),
                        ty: crate::ast::TypeName::Reference(
                            Box::new(crate::ast::TypeName::Named(
                                definition.name.clone(),
                                method.span,
                            )),
                            requirement.self_mutable,
                            method.span,
                        ),
                        span: method.span,
                    }],
                    return_type: method.return_type.clone(),
                    body: default_body,
                    return_value: default_value,
                    span: method.span,
                });
                signatures.insert(
                    format!("{qualified}#method"),
                    FunctionSignature {
                        target: IrCallTarget::Function(index),
                        parameters: vec![self_type],
                        return_type: requirement.return_type,
                        is_destructor: false,
                    },
                );
                function_module_paths.push(definition.module_path.clone());
                function_visibility.push(true);
            }
        }
    }
}

/// Synthesizes `Type::clone` methods for `#[derive(Clone)]` structs. Field
/// reads through the borrowed receiver already deep-copy owned leaves, so the
/// body is a plain struct literal of field reads.
#[allow(clippy::too_many_arguments)]
fn materialize_derived_clones(
    functions: &mut Vec<crate::ast::Function>,
    signatures: &mut HashMap<String, FunctionSignature>,
    function_module_paths: &mut Vec<String>,
    function_visibility: &mut Vec<bool>,
    structs: &[RynStruct],
    _struct_ids: &HashMap<String, usize>,
    _enum_ids: &HashMap<String, usize>,
) {
    for (struct_index, definition) in structs.iter().enumerate() {
        if !definition.derives_clone || definition.drop_function.is_some() {
            continue;
        }
        if definition.name.starts_with('$') {
            continue;
        }
        let method_name = format!("{}::clone", definition.name);
        if signatures.contains_key(&format!("{method_name}#method")) {
            continue;
        }
        let span = Span::default();
        let mut fields = Vec::new();
        for field in &definition.fields {
            fields.push((
                field.name.clone(),
                Expression::Field {
                    value: Box::new(Expression::Name("self".into(), span)),
                    name: field.name.clone(),
                    name_span: span,
                    span,
                },
                span,
            ));
        }
        let index = functions.len();
        functions.push(crate::ast::Function {
            name: method_name.clone(),
            extern_c: false,
            external_symbol: None,
            type_parameters: Vec::new(),
            type_parameter_bounds: Vec::new(),
            public: true,
            module_path: definition.module_path.clone(),
            parameters: vec![crate::ast::Parameter {
                name: "self".into(),
                ty: crate::ast::TypeName::Reference(
                    Box::new(crate::ast::TypeName::Named(definition.name.clone(), span)),
                    false,
                    span,
                ),
                span,
            }],
            return_type: Some(crate::ast::TypeName::Named(definition.name.clone(), span)),
            body: Vec::new(),
            return_value: Some(Expression::StructLiteral {
                name: definition.name.clone(),
                fields,
                span,
            }),
            span,
        });
        let struct_type = Type::Struct(struct_index);
        signatures.insert(
            format!("{method_name}#method"),
            FunctionSignature {
                target: IrCallTarget::Function(index),
                parameters: vec![Type::Reference(intern_pointer_target(struct_type), false)],
                return_type: Some(struct_type),
                is_destructor: false,
            },
        );
        function_module_paths.push(definition.module_path.clone());
        function_visibility.push(true);
    }
}

fn is_option_u64_ast(definition: &EnumDef) -> bool {
    definition.name.starts_with("$RynOption")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Some"
        && definition.variants[0].fields == [TypeName::U64]
        && definition.variants[1].name == "None"
        && definition.variants[1].fields.is_empty()
}

fn is_option_string_ast(definition: &EnumDef) -> bool {
    definition.name.starts_with("$RynOption")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Some"
        && definition.variants[0].fields == [TypeName::OwnedString]
        && definition.variants[1].name == "None"
        && definition.variants[1].fields.is_empty()
}

fn is_option_ast_payload(definition: &EnumDef, payload: &TypeName) -> bool {
    definition.name.starts_with("$RynOption")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Some"
        && definition.variants[0].fields == [payload.clone()]
        && definition.variants[1].name == "None"
        && definition.variants[1].fields.is_empty()
}

/// Builtin collection and String methods operate on the stored handle in
/// place, so a receiver read through a reference (`self.items.push(x)`) must
/// not clone the field first: the mutation would land on the copy.
fn borrow_reference_receiver(receiver: &mut IrExpression, ty: Type) {
    if !matches!(
        ty,
        Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::OwnedString
    ) {
        return;
    }
    let mut current = receiver;
    loop {
        match current {
            IrExpression::ReferenceField { borrowed, .. } => {
                *borrowed = true;
                return;
            }
            IrExpression::Field { value, .. } => current = value,
            _ => return,
        }
    }
}

fn is_option_u64(definition: &RynEnum) -> bool {
    definition.name.starts_with("$RynOption")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Some"
        && definition.variants[0].fields == [Type::U64]
        && definition.variants[1].name == "None"
        && definition.variants[1].fields.is_empty()
}

fn is_option_of(definition: &RynEnum, value: Type) -> bool {
    definition.name.starts_with("$RynOption#")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Some"
        && definition.variants[0].fields == [value]
        && definition.variants[1].name == "None"
        && definition.variants[1].fields.is_empty()
}

fn is_result_of(definition: &RynEnum, value: Type, error: Type) -> bool {
    definition.name.starts_with("$RynResult#")
        && definition.variants.len() == 2
        && definition.variants[0].name == "Ok"
        && definition.variants[0].fields == [value]
        && definition.variants[1].name == "Err"
        && definition.variants[1].fields == [error]
}

struct Analyzer<'a> {
    names: HashMap<String, Binding>,
    local_types: Vec<LocalType>,
    owned_slot_types: Vec<Option<Type>>,
    addressed_slot_types: Vec<Option<Type>>,
    parameter_slots: HashSet<usize>,
    signatures: &'a HashMap<String, FunctionSignature>,
    structs: &'a mut Vec<RynStruct>,
    struct_ids: &'a HashMap<String, usize>,
    enums: &'a [RynEnum],
    enum_ids: &'a HashMap<String, usize>,
    function_module_paths: &'a [String],
    function_visibility: &'a [bool],
    shapes: &'a ShapeTable,
    loop_depth: usize,
    // `defer` blocks waiting for their enclosing block to end, one frame per open block.
    defer_frames: Vec<Vec<DeferredBody>>,
    // For each open loop, the index of the first frame that belongs to its body.
    loop_defer_bases: Vec<usize>,
    // Non-zero while a `defer` body is being lowered: control may not leave it.
    defer_nesting: usize,
    // True while the function's result expression is lowered, after its `defer` blocks were registered.
    tail_defers_pending: bool,
    function_name: String,
    module_path: String,
    return_type: Option<Type>,
    recover_block_errors: bool,
    recovery_diagnostics: Vec<Diagnostic>,
    /// Lets created while lowering an expression. They run just before the
    /// statement that uses them, or at the top of a `while` header.
    pending_statements: Vec<IrStatement>,
}

impl<'a> Analyzer<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        signatures: &'a HashMap<String, FunctionSignature>,
        structs: &'a mut Vec<RynStruct>,
        struct_ids: &'a HashMap<String, usize>,
        enums: &'a [RynEnum],
        enum_ids: &'a HashMap<String, usize>,
        function_module_paths: &'a [String],
        function_visibility: &'a [bool],
        shapes: &'a ShapeTable,
    ) -> Self {
        Self {
            names: HashMap::new(),
            local_types: Vec::new(),
            owned_slot_types: Vec::new(),
            addressed_slot_types: Vec::new(),
            parameter_slots: HashSet::new(),
            signatures,
            structs,
            struct_ids,
            enums,
            enum_ids,
            function_module_paths,
            function_visibility,
            shapes,
            loop_depth: 0,
            defer_frames: Vec::new(),
            loop_defer_bases: Vec::new(),
            defer_nesting: 0,
            tail_defers_pending: false,
            function_name: String::new(),
            module_path: String::new(),
            return_type: None,
            recover_block_errors: false,
            recovery_diagnostics: Vec::new(),
            pending_statements: Vec::new(),
        }
    }

    fn namespace_prefix(&self) -> Option<String> {
        let local_name = if self.module_path.is_empty() {
            self.function_name.as_str()
        } else {
            self.function_name
                .strip_prefix(&format!("{}::", self.module_path))?
        };
        local_name
            .rsplit_once("::")
            .map(|(namespace, _)| namespace.to_string())
    }

    fn scoped_struct_id(&self, name: &str) -> Option<usize> {
        self.struct_ids.get(name).copied().or_else(|| {
            self.namespace_prefix()
                .map(|namespace| format!("{namespace}::{name}"))
                .and_then(|qualified| self.struct_ids.get(&qualified).copied())
        })
    }

    fn scoped_enum_id(&self, name: &str) -> Option<usize> {
        self.enum_ids.get(name).copied().or_else(|| {
            self.namespace_prefix()
                .map(|namespace| format!("{namespace}::{name}"))
                .and_then(|qualified| self.enum_ids.get(&qualified).copied())
        })
    }

    fn resolve_type_name(&self, name: &TypeName) -> Result<Type, Diagnostic> {
        let namespace = self.namespace_prefix();
        let ty =
            resolve_type_name_scoped(name, self.struct_ids, self.enum_ids, namespace.as_deref())?;
        let mut seen_structs = HashSet::new();
        let mut seen_enums = HashSet::new();
        if contains_custom_drop(
            ty,
            self.structs,
            self.enums,
            &mut seen_structs,
            &mut seen_enums,
            0,
        ) && !matches!(ty, Type::Struct(id) if self.structs[id].drop_function.is_some())
            && !custom_drop_array_only(ty, self.structs, self.enums)
            && !custom_drop_vec_only(ty, self.structs, self.enums)
            && !custom_drop_map_only(ty, self.structs, self.enums)
            && !custom_drop_struct_fields_only(ty, self.structs, self.enums, 0)
        {
            return Err(diag(
                "R0255",
                "custom-destructor values may only be nested through ordinary struct fields; collections, arrays, and enum payloads remain unsupported",
                name_span(name),
            ));
        }
        if !map_struct_value_supported(ty, self.structs, self.enums) {
            return Err(diag(
                "R0244",
                "Map struct values must support a generated clone/drop plan; this value type has no supported Map layout",
                name_span(name),
            ));
        }
        validate_array_supported(ty, self.structs, name_span(name))?;
        validate_vec_struct_elements(ty, self.structs, name_span(name))?;
        Ok(ty)
    }

    fn function(mut self, mut function: Function) -> Result<RynFunction, Diagnostic> {
        if function.extern_c {
            let signature = &self.signatures[&function_signature_key(&function)];
            return Ok(RynFunction {
                parameters: signature
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(slot, ty)| LocalBinding { slot, ty: *ty })
                    .collect(),
                external_symbol: function.external_symbol,
                return_type: signature.return_type,
                reference_return_parameter: None,
                return_value: None,
                statements: Vec::new(),
                local_types: Vec::new(),
                owned_slot_types: Vec::new(),
                addressed_slot_types: Vec::new(),
            });
        }
        let (parameters, body_always_returns) = self.prepare_function(&function)?;
        let (statements, deferred) = self.lower_statements(std::mem::take(&mut function.body))?;
        self.finish_function(
            function,
            parameters,
            body_always_returns,
            statements,
            deferred,
        )
    }

    fn function_recovering(
        mut self,
        mut function: Function,
    ) -> Result<RynFunction, Vec<Diagnostic>> {
        if function.extern_c {
            return self.function(function).map_err(|error| vec![error]);
        }
        let (parameters, body_always_returns) = self
            .prepare_function(&function)
            .map_err(|error| vec![error])?;
        self.recover_block_errors = true;
        let (statements, top_level_declaration_failed, deferred) =
            self.recover_statements(std::mem::take(&mut function.body), true);
        let mut diagnostics = std::mem::take(&mut self.recovery_diagnostics);
        if top_level_declaration_failed {
            diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(diagnostics);
        }
        let finished =
            self.finish_function(function, parameters, body_always_returns, statements, deferred);
        diagnostics.extend(std::mem::take(&mut self.recovery_diagnostics));
        match finished {
            Ok(function) if diagnostics.is_empty() => Ok(function),
            Ok(_) => {
                diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
                Err(diagnostics)
            }
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
                Err(diagnostics)
            }
        }
    }

    fn prepare_function(
        &mut self,
        function: &Function,
    ) -> Result<(Vec<LocalBinding>, bool), Diagnostic> {
        self.function_name.clone_from(&function.name);
        self.module_path.clone_from(&function.module_path);
        let signature_key = function_signature_key(function);
        self.return_type = self.signatures[&signature_key].return_type;
        if matches!(self.return_type, Some(Type::Slice(_))) {
            return Err(diag(
                "R0240",
                "a borrowed slice cannot be returned from a function",
                function.span,
            )
            .with_help("return an owning `Vec<T>` or consume the slice inside the function"));
        }
        let body_always_returns = block_always_returns(&function.body);
        let mut parameters = Vec::with_capacity(function.parameters.len());
        for (parameter, ty) in function
            .parameters
            .iter()
            .zip(&self.signatures[&signature_key].parameters)
        {
            if self.names.contains_key(&parameter.name) {
                return Err(diag(
                    "R0202",
                    format!("duplicate parameter `{}`", parameter.name),
                    parameter.span,
                )
                .with_help("choose a unique name for each parameter in the function"));
            }
            let ty = *ty;
            let slot = self.allocate(ty);
            self.parameter_slots.insert(slot);
            self.names.insert(
                parameter.name.clone(),
                Binding {
                    slot,
                    ty,
                    mutable: false,
                },
            );
            parameters.push(LocalBinding { slot, ty });
        }
        for (type_name, bounds) in &function.type_parameter_bounds {
            if bounds.is_empty() {
                continue;
            }
            let Some(struct_id) = self.scoped_struct_id(type_name) else {
                continue;
            };
            for shape_name in bounds {
                if let Err(message) = struct_conforms_to_shape(
                    &self.structs[struct_id],
                    shape_name,
                    self.shapes,
                    self.signatures,
                    self.structs,
                ) {
                    return Err(diag("R0450", message, function.span).with_help(format!(
                        "structural conformance checks `{type_name}` against `{shape_name}` at the call site"
                    )));
                }
            }
        }
        Ok((parameters, body_always_returns))
    }

    fn finish_function(
        &mut self,
        function: Function,
        parameters: Vec<LocalBinding>,
        body_always_returns: bool,
        mut statements: Vec<IrStatement>,
        deferred: Vec<DeferredBody>,
    ) -> Result<RynFunction, Diagnostic> {
        let return_type = self.return_type;
        self.tail_defers_pending = !deferred.is_empty();
        let mut return_value = match (return_type, function.return_value) {
            (Some(expected), Some(value)) => {
                let span = value.span();
                let (value, actual) = self.expression(value, Some(expected))?;
                statements.extend(std::mem::take(&mut self.pending_statements));
                self.validate_reference_return(&value, function.span)?;
                if expected != actual {
                    return Err(diag(
                        "R0205",
                        format!(
                            "function `{}` returns `{}` but its declared result is `{}`",
                            function.name,
                            type_name(actual),
                            type_name(expected)
                        ),
                        span,
                    )
                    .with_help(format!(
                        "return a `{}` value or change the function's declared result type",
                        type_name(expected)
                    )));
                }
                Some(value)
            }
            (Some(_), None) if body_always_returns => None,
            (Some(expected), None) => {
                return Err(diag(
                    "R0213",
                    format!(
                        "function `{}` must return a `{}` value",
                        function.name,
                        type_name(expected)
                    ),
                    function.span,
                )
                .with_help(format!(
                    "return a `{}` value from this function",
                    type_name(expected)
                )));
            }
            (None, Some(value)) => {
                let error = diag(
                    "R0214",
                    format!(
                        "function `{}` has a result expression but no `->` type",
                        function.name
                    ),
                    function.span,
                )
                .with_help(format!(
                    "add a `-> TYPE` result type to `{}` or remove its result expression",
                    function.name
                ));
                self.check_discarded_expression(value);
                return Err(error);
            }
            (None, None) => None,
        };
        self.tail_defers_pending = false;
        if !deferred.is_empty() {
            // The result is computed first, so the deferred bodies run after it and cannot change it.
            let leaving = self.lower_frame(deferred)?;
            return_value = match return_value {
                Some(value) => {
                    let ty = return_type.expect("a function result has a declared type");
                    let slot = self.allocate(ty);
                    statements.push(IrStatement::Let {
                        slot,
                        ty,
                        value,
                        span: function.span,
                    });
                    Some(IrExpression::Local {
                        slot,
                        ty,
                        span: function.span,
                    })
                }
                None => None,
            };
            statements.extend(leaving);
        }
        let mut returned_reference_parameters = Vec::new();
        collect_reference_returns(&statements, &parameters, &mut returned_reference_parameters);
        if let Some(value) = &return_value {
            returned_reference_parameters.push(reference_parameter_index(value, &parameters));
        }
        let reference_return_parameter = returned_reference_parameters
            .first()
            .copied()
            .flatten()
            .filter(|parameter| {
                returned_reference_parameters
                    .iter()
                    .all(|returned| *returned == Some(*parameter))
            });
        Ok(RynFunction {
            parameters,
            external_symbol: None,
            return_type,
            reference_return_parameter,
            return_value,
            statements,
            local_types: std::mem::take(&mut self.local_types),
            owned_slot_types: std::mem::take(&mut self.owned_slot_types),
            addressed_slot_types: std::mem::take(&mut self.addressed_slot_types),
        })
    }

    fn allocate(&mut self, ty: Type) -> usize {
        let slot = self.local_types.len();
        self.allocate_storage(ty);
        slot
    }

    fn allocate_storage(&mut self, ty: Type) {
        match ty {
            Type::Struct(struct_id) => {
                let field_types = self.structs[struct_id]
                    .fields
                    .iter()
                    .map(|field| field.ty)
                    .collect::<Vec<_>>();
                for field_ty in field_types {
                    self.allocate_storage(field_ty);
                }
            }
            Type::Array(id) => {
                let (element, length) = array_info(id);
                for _ in 0..length {
                    self.allocate_storage(element);
                }
            }
            _ => self.allocate_scalar(ty),
        }
    }

    fn allocate_scalar(&mut self, ty: Type) {
        self.addressed_slot_types.push(None);
        match ty {
            Type::I8 | Type::U8 => {
                self.local_types.push(LocalType::I8);
                self.owned_slot_types.push(None);
            }
            Type::I16 | Type::U16 => {
                self.local_types.push(LocalType::I16);
                self.owned_slot_types.push(None);
            }
            Type::I32 => {
                self.local_types.push(LocalType::I32);
                self.owned_slot_types.push(None);
            }
            Type::I64 => {
                self.local_types.push(LocalType::I64);
                self.owned_slot_types.push(None);
            }
            Type::U32 => {
                self.local_types.push(LocalType::I32);
                self.owned_slot_types.push(None);
            }
            Type::U64 => {
                self.local_types.push(LocalType::I64);
                self.owned_slot_types.push(None);
            }
            Type::F32 => {
                self.local_types.push(LocalType::F32);
                self.owned_slot_types.push(None);
            }
            Type::F64 => {
                self.local_types.push(LocalType::F64);
                self.owned_slot_types.push(None);
            }
            Type::Str => {
                self.local_types.push(LocalType::Ptr);
                self.owned_slot_types.push(None);
                self.local_types.push(LocalType::I64);
                self.owned_slot_types.push(None);
            }
            Type::Slice(_) => {
                self.local_types.push(LocalType::Ptr);
                self.owned_slot_types.push(None);
                self.local_types.push(LocalType::I64);
                self.owned_slot_types.push(None);
            }
            Type::Reference(_, _) | Type::RawPointer(_) | Type::FunctionPointer(_) => {
                self.local_types.push(LocalType::Ptr);
                self.owned_slot_types.push(None);
            }
            Type::OwnedString => {
                self.local_types.push(LocalType::OwnedPtr);
                self.owned_slot_types.push(Some(Type::OwnedString));
            }
            Type::Vec(_) | Type::Map(_) | Type::Set(_) => {
                self.local_types.push(LocalType::OwnedPtr);
                self.owned_slot_types.push(Some(ty));
            }
            Type::Enum(_) => {
                self.local_types.push(LocalType::OwnedPtr);
                self.owned_slot_types.push(Some(ty));
            }
            Type::Char => {
                self.local_types.push(LocalType::I32);
                self.owned_slot_types.push(None);
            }
            Type::Bool => {
                self.local_types.push(LocalType::I8);
                self.owned_slot_types.push(None);
            }
            Type::Struct(_) => unreachable!("structure storage is allocated field by field"),
            Type::Array(_) => unreachable!("array storage is allocated element by element"),
        }
    }

    fn enter_loop(&mut self) {
        self.loop_depth += 1;
        self.loop_defer_bases.push(self.defer_frames.len());
    }

    fn exit_loop(&mut self) {
        self.loop_depth -= 1;
        self.loop_defer_bases.pop();
    }

    fn block(&mut self, body: Vec<Statement>) -> Result<Vec<IrStatement>, Diagnostic> {
        if !self.recover_block_errors {
            let (mut statements, frame) = self.lower_statements(body)?;
            statements.extend(self.lower_frame(frame)?);
            return Ok(statements);
        }

        Ok(self.recover_block(body, false).0)
    }

    // Lowers a block's statements inside a new `defer` frame and returns that frame unlowered,
    // so the caller decides where its deferred bodies run.
    fn lower_statements(
        &mut self,
        body: Vec<Statement>,
    ) -> Result<(Vec<IrStatement>, Vec<DeferredBody>), Diagnostic> {
        self.defer_frames.push(Vec::new());
        let lowered: Result<Vec<IrStatement>, Diagnostic> =
            body.into_iter().map(|stmt| self.statement(stmt)).collect();
        let frame = self.defer_frames.pop().unwrap_or_default();
        Ok((lowered?, frame))
    }

    fn recover_block(
        &mut self,
        body: Vec<Statement>,
        function_body: bool,
    ) -> (Vec<IrStatement>, bool) {
        let (mut statements, stopped_on_declaration, frame) =
            self.recover_statements(body, function_body);
        match self.lower_frame(frame) {
            Ok(deferred) => statements.extend(deferred),
            Err(diagnostic) => self.recovery_diagnostics.push(diagnostic),
        }
        (statements, stopped_on_declaration)
    }

    fn recover_statements(
        &mut self,
        body: Vec<Statement>,
        function_body: bool,
    ) -> (Vec<IrStatement>, bool, Vec<DeferredBody>) {
        self.defer_frames.push(Vec::new());
        let mut statements = Vec::with_capacity(body.len());
        let mut stopped_on_declaration = false;
        for statement in body {
            let names_before = self.names.clone();
            let local_type_count = self.local_types.len();
            let loop_depth = self.loop_depth;
            let loop_bases = self.loop_defer_bases.len();
            let failed_declaration_blocks_recovery = matches!(
                &statement,
                Statement::Let { name, .. } if !self.names.contains_key(name)
            );
            match self.statement(statement) {
                Ok(statement) => statements.push(statement),
                Err(diagnostic) => {
                    self.names = names_before;
                    self.local_types.truncate(local_type_count);
                    self.owned_slot_types.truncate(local_type_count);
                    self.loop_depth = loop_depth;
                    self.loop_defer_bases.truncate(loop_bases);
                    self.recovery_diagnostics.push(diagnostic);
                    if failed_declaration_blocks_recovery {
                        stopped_on_declaration = true;
                        break;
                    }
                }
            }
        }
        let frame = self.defer_frames.pop().unwrap_or_default();
        (
            statements,
            function_body && stopped_on_declaration,
            frame,
        )
    }

    // Lowers the deferred bodies of one frame in reverse registration order, which is the
    // order they run when their block ends.
    fn lower_frame(&mut self, frame: Vec<DeferredBody>) -> Result<Vec<IrStatement>, Diagnostic> {
        let mut statements = Vec::new();
        for deferred in frame.into_iter().rev() {
            statements.extend(self.lower_deferred_body(deferred)?);
        }
        Ok(statements)
    }

    // The deferred bodies that run when control leaves the frames from `first_frame` onward:
    // innermost frame first, and within a frame the most recently registered body first.
    fn leaving_defers(&mut self, first_frame: usize) -> Result<Vec<IrStatement>, Diagnostic> {
        let mut leaving = Vec::new();
        for frame in self.defer_frames[first_frame.min(self.defer_frames.len())..]
            .iter()
            .rev()
        {
            leaving.extend(frame.iter().rev().cloned());
        }
        let mut statements = Vec::new();
        for deferred in leaving {
            statements.extend(self.lower_deferred_body(deferred)?);
        }
        Ok(statements)
    }

    fn lower_deferred_body(
        &mut self,
        deferred: DeferredBody,
    ) -> Result<Vec<IrStatement>, Diagnostic> {
        let outer_names = std::mem::replace(&mut self.names, deferred.names);
        let outer_loop_depth = std::mem::replace(&mut self.loop_depth, 0);
        let outer_loop_bases = std::mem::take(&mut self.loop_defer_bases);
        self.defer_nesting += 1;
        let result = self.block(deferred.body);
        self.defer_nesting -= 1;
        self.loop_defer_bases = outer_loop_bases;
        self.loop_depth = outer_loop_depth;
        self.names = outer_names;
        result
    }

    fn check_discarded_expression(&mut self, expression: Expression) {
        if self.recover_block_errors
            && let Err(diagnostic) = self.expression(expression, None)
        {
            self.recovery_diagnostics.push(diagnostic);
        }
    }

    fn recover_for_body(&mut self, name: &str, ty: Type, body: Vec<Statement>) {
        let outer_names = self.names.clone();
        let slot = self.allocate(ty);
        self.allocate(ty);
        self.names.insert(
            name.to_owned(),
            Binding {
                slot,
                ty,
                mutable: false,
            },
        );
        self.enter_loop();
        let _ = self.block(body);
        self.exit_loop();
        self.names = outer_names;
    }

    fn recover_for_after_invalid_start(
        &mut self,
        name: &str,
        name_span: Span,
        end: Expression,
        body: Vec<Statement>,
    ) {
        let end_span = end.span();
        match self.expression(end, None) {
            Ok((_, ty)) if is_integer(ty) => self.recover_for_body(name, ty, body),
            Ok((_, ty)) => self.recovery_diagnostics.push(
                diag(
                    "R0206",
                    format!(
                        "`for` range bounds must be integers, found `{}`",
                        type_name(ty)
                    ),
                    if end_span == Span::default() {
                        name_span
                    } else {
                        end_span
                    },
                )
                .with_help("use matching signed or unsigned integer bounds for the range"),
            ),
            Err(error) => self.recovery_diagnostics.push(error),
        }
    }

    fn recover_for_after_duplicate_name(
        &mut self,
        name: &str,
        name_span: Span,
        start: Expression,
        end: Expression,
        body: Vec<Statement>,
    ) {
        let (_, ty) = match self.expression(start, None) {
            Ok(value) => value,
            Err(error) => {
                self.recovery_diagnostics.push(error);
                self.recover_for_after_invalid_start(name, name_span, end, body);
                return;
            }
        };
        if !is_integer(ty) {
            self.recovery_diagnostics.push(
                diag(
                    "R0206",
                    format!(
                        "`for` range bounds must be integers, found `{}`",
                        type_name(ty)
                    ),
                    name_span,
                )
                .with_help("use matching signed or unsigned integer bounds for the range"),
            );
            self.recover_for_after_invalid_start(name, name_span, end, body);
            return;
        }

        match self.expression(end, Some(ty)) {
            Ok((_, end_ty)) => {
                if end_ty != ty {
                    self.recovery_diagnostics.push(
                        diag(
                            "R0206",
                            format!(
                                "`for` range bounds must have the same integer type, found `{}` and `{}`",
                                type_name(ty),
                                type_name(end_ty)
                            ),
                            name_span,
                        )
                        .with_help("use the same integer type for both range bounds"),
                    );
                }
            }
            Err(error) => self.recovery_diagnostics.push(error),
        }
        self.recover_for_body(name, ty, body);
    }

    fn check_let_initializer_for_recovery(
        &mut self,
        annotation: Option<&TypeName>,
        value: Expression,
        value_span: Span,
    ) {
        let expected = match annotation
            .map(|name| self.resolve_type_name(name))
            .transpose()
        {
            Ok(expected) => expected,
            Err(diagnostic) => {
                self.recovery_diagnostics.push(diagnostic);
                None
            }
        };
        match self.expression(value, expected) {
            Ok((_, actual)) if expected.is_some_and(|expected| expected != actual) => {
                let expected = expected.expect("mismatched initializer has an expected type");
                self.recovery_diagnostics.push(
                    diag(
                        "R0205",
                        format!(
                            "declared type `{}` does not match initializer type `{}`",
                            type_name(expected),
                            type_name(actual)
                        ),
                        value_span,
                    )
                    .with_help(format!(
                        "make the initializer a `{}` value or change the type annotation",
                        type_name(expected)
                    )),
                );
            }
            Ok(_) => {}
            Err(diagnostic) => self.recovery_diagnostics.push(diagnostic),
        }
    }

    fn statement(&mut self, statement: Statement) -> Result<IrStatement, Diagnostic> {
        match self.lower_statement(statement) {
            Ok(lowered) => Ok(self.attach_pending(lowered)),
            Err(error) => {
                self.pending_statements.clear();
                Err(error)
            }
        }
    }

    fn attach_pending(&mut self, statement: IrStatement) -> IrStatement {
        let pending = std::mem::take(&mut self.pending_statements);
        with_setup(pending, statement)
    }

    /// Stores a copyable expression in a fresh local so a shared method can borrow it.
    fn materialize_copy_receiver(
        &mut self,
        value: IrExpression,
        ty: Type,
        span: Span,
    ) -> Result<IrExpression, Diagnostic> {
        if !receiver_is_copyable(ty, self.structs) {
            return Err(diag(
                "R0235",
                "method receivers must be a local variable for now",
                span,
            )
            .with_help(
                "assign the value to a local first; only copyable expression receivers are stored in a temporary",
            ));
        }
        let slot = self.allocate(ty);
        self.pending_statements.push(IrStatement::Let {
            slot,
            ty,
            value,
            span,
        });
        Ok(IrExpression::Local { slot, ty, span })
    }

    fn lower_statement(&mut self, statement: Statement) -> Result<IrStatement, Diagnostic> {
        match statement {
            Statement::Let {
                name,
                mutable,
                annotation,
                value,
                span,
            } => {
                // `_ := value` evaluates and keeps the value until the end of
                // the scope without binding a name, so it may repeat.
                let discard = name == "_";
                if !discard && self.names.contains_key(&name) {
                    let value_span = value.span();
                    let error = diag(
                        "R0202",
                        format!("`{name}` is already declared in this scope"),
                        span,
                    )
                    .with_help(format!(
                        "choose a different name or remove the second declaration of `{name}`"
                    ));
                    if self.recover_block_errors {
                        self.check_let_initializer_for_recovery(
                            annotation.as_ref(),
                            value,
                            value_span,
                        );
                    }
                    return Err(error);
                }
                let value_span = value.span();
                let annotation = match annotation
                    .as_ref()
                    .map(|name| self.resolve_type_name(name))
                    .transpose()
                {
                    Ok(annotation) => annotation,
                    Err(error) if self.recover_block_errors => {
                        self.check_discarded_expression(value);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                let (value, inferred) = self.expression(value, annotation)?;
                let ty = annotation.unwrap_or(inferred);
                if matches!(ty, Type::Slice(_)) {
                    return Err(diag(
                        "R0240",
                        "a borrowed slice cannot be stored in a local binding",
                        value_span,
                    )
                    .with_help("use the slice immediately or pass it to a function"));
                }
                if ty != inferred {
                    return Err(diag(
                        "R0205",
                        format!(
                            "declared type `{}` does not match initializer type `{}`",
                            type_name(ty),
                            type_name(inferred)
                        ),
                        value_span,
                    )
                    .with_help(format!(
                        "make the initializer a `{}` value or change the type annotation",
                        type_name(ty)
                    )));
                }
                let slot = self.allocate(ty);
                if !discard {
                    self.names.insert(name, Binding { slot, ty, mutable });
                }
                Ok(IrStatement::Let {
                    slot,
                    ty,
                    value,
                    span,
                })
            }
            Statement::Assign { name, value, span } => {
                let Some(binding) = self.names.get(&name).copied() else {
                    let error = self.unknown_variable(&name, span);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                if !binding.mutable {
                    let error = diag("R0204", format!("`{name}` is immutable"), span).with_help(
                        format!("declare `mut {name} := ...` if this binding must be mutable"),
                    );
                    self.check_discarded_expression(value);
                    return Err(error);
                }
                let value_span = value.span();
                let (value, actual) = self.expression(value, Some(binding.ty))?;
                if binding.ty != actual {
                    return Err(diag(
                        "R0205",
                        format!(
                            "cannot assign `{}` to `{name}` of type `{}`",
                            type_name(actual),
                            type_name(binding.ty)
                        ),
                        value_span,
                    )
                    .with_help(format!(
                        "assign a `{}` value to `{name}`",
                        type_name(binding.ty)
                    )));
                }
                Ok(IrStatement::Assign {
                    slot: binding.slot,
                    ty: binding.ty,
                    value,
                    span,
                })
            }
            Statement::DereferenceAssign {
                pointer,
                value,
                span,
            } => {
                let (pointer, pointer_type) = self.expression(pointer, None)?;
                let (ty, mutable) = match pointer_type {
                    Type::Reference(id, mutable) => (pointer_target(id), mutable),
                    Type::RawPointer(id) => (pointer_target(id), true),
                    _ => {
                        return Err(diag(
                            "R0206",
                            "dereference assignment requires a mutable reference or raw pointer",
                            span,
                        ));
                    }
                };
                if !mutable {
                    return Err(
                        diag("R0206", "cannot assign through a shared reference", span)
                            .with_help("create the reference with `&mut value`"),
                    );
                }
                if !reference_pointee_supported_in(ty, self.structs)
                    || matches!(ty, Type::Struct(_)) && !pointer_record_is_copy(ty, self.structs)
                {
                    return Err(diag(
                        "R0206",
                        "assignment through this pointer type is not supported yet",
                        span,
                    ));
                }
                let value_span = value.span();
                let (value, actual) = self.expression(value, Some(ty))?;
                if actual != ty {
                    return Err(diag(
                        "R0205",
                        format!(
                            "cannot assign `{}` through a pointer to `{}`",
                            type_name(actual),
                            type_name(ty)
                        ),
                        value_span,
                    ));
                }
                Ok(IrStatement::DereferenceAssign { pointer, ty, value })
            }
            Statement::IndexAssign {
                name,
                index,
                value,
                span,
            } => {
                let Some(binding) = self.names.get(&name).copied() else {
                    let error = self.unknown_variable(&name, span);
                    self.check_discarded_expression(index);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                if !binding.mutable {
                    let error = diag("R0204", format!("`{name}` is immutable"), span).with_help(
                        format!("declare `mut {name} := ...` to assign an array element"),
                    );
                    self.check_discarded_expression(index);
                    self.check_discarded_expression(value);
                    return Err(error);
                }
                let Type::Array(array_id) = binding.ty else {
                    let error = diag("R0241", "indexed assignment requires a fixed array", span);
                    self.check_discarded_expression(index);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                let (element, length) = array_info(array_id);
                let mut seen_structs = HashSet::new();
                let mut seen_enums = HashSet::new();
                if contains_custom_drop(
                    element,
                    self.structs,
                    self.enums,
                    &mut seen_structs,
                    &mut seen_enums,
                    0,
                ) {
                    return Err(diag(
                        "R0255",
                        "custom-destructor array elements cannot be replaced by index yet",
                        span,
                    )
                    .with_help(
                        "replace or move the whole array so every owner is cleaned up correctly",
                    ));
                }
                let (index, index_ty) = self.expression(index, None)?;
                if !is_integer(index_ty) {
                    return Err(diag("R0242", "array index must be an integer", span));
                }
                let (value, value_ty) = self.expression(value, Some(element))?;
                if value_ty != element {
                    return Err(diag(
                        "R0205",
                        format!(
                            "array element requires `{}` but has type `{}`",
                            type_name(element),
                            type_name(value_ty)
                        ),
                        span,
                    ));
                }
                Ok(IrStatement::ArrayAssign {
                    slot: binding.slot,
                    element,
                    length,
                    index,
                    value,
                    span,
                })
            }
            Statement::FieldAssign {
                object,
                fields,
                op,
                value,
                span,
            } => {
                let object_span = Span {
                    start: span.start,
                    end: span.start + object.len(),
                };
                let Some(binding) = self.names.get(&object).copied() else {
                    let error = self.unknown_variable(&object, object_span);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                if let Type::Reference(target, reference_mutable) = binding.ty {
                    let pointer = IrExpression::Local {
                        slot: binding.slot,
                        ty: binding.ty,
                        span: object_span,
                    };
                    return self.reference_field_assign(
                        pointer,
                        pointer_target(target),
                        reference_mutable,
                        object.clone(),
                        object_span,
                        &fields,
                        op,
                        value,
                        span,
                    );
                }
                if let Type::RawPointer(target) = binding.ty {
                    let pointee = pointer_target(target);
                    if !matches!(pointee, Type::Struct(id) if self.structs[id].repr_c) {
                        return Err(diag(
                            "R0224",
                            "raw pointer field access requires a #[repr(C)] structure",
                            object_span,
                        ));
                    }
                    return self.reference_field_assign(
                        IrExpression::Local {
                            slot: binding.slot,
                            ty: binding.ty,
                            span: object_span,
                        },
                        pointee,
                        true,
                        object.clone(),
                        object_span,
                        &fields,
                        op,
                        value,
                        span,
                    );
                }
                if !binding.mutable {
                    let error = diag("R0204", format!("`{object}` is immutable"), object_span)
                        .with_help(format!(
                            "declare `mut {object} := ...` if you intend to change its fields"
                        ));
                    self.check_discarded_expression(value);
                    return Err(error);
                }
                let Some(((final_field, final_field_span), parent_fields)) = fields.split_last()
                else {
                    let error = diag(
                        "R0900",
                        "internal error: field assignment has no field path",
                        span,
                    );
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                let mut parent_type = binding.ty;
                let mut slot_offset = 0;
                for (field, field_span) in parent_fields {
                    let Type::Struct(struct_id) = parent_type else {
                        let error = diag(
                            "R0224",
                            "field path passes through a non-structure value",
                            span,
                        )
                        .with_help("use fields that lead through structure-typed values");
                        self.check_discarded_expression(value);
                        return Err(error);
                    };
                    let Some(intermediate) = self.structs[struct_id]
                        .fields
                        .iter()
                        .find(|candidate| candidate.name == *field)
                    else {
                        let error = self.unknown_struct_field(struct_id, field, *field_span);
                        self.check_discarded_expression(value);
                        return Err(error);
                    };
                    slot_offset += intermediate.slot_offset;
                    parent_type = intermediate.ty;
                }
                let Type::Struct(struct_id) = parent_type else {
                    let error = diag(
                        "R0224",
                        "field assignment target is not a structure field",
                        span,
                    )
                    .with_help("the final field path must name a field on a structure");
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                let Some(field_info) = self.structs[struct_id]
                    .fields
                    .iter()
                    .find(|candidate| candidate.name == *final_field)
                    .cloned()
                else {
                    let error =
                        self.unknown_struct_field(struct_id, final_field, *final_field_span);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                slot_offset += field_info.slot_offset;
                let value_span = value.span();
                let (value, actual) = if let Some(op) = op {
                    let mut left = Expression::Name(object.clone(), object_span);
                    for (field, field_span) in &fields {
                        left = Expression::Field {
                            value: Box::new(left),
                            name: field.clone(),
                            name_span: *field_span,
                            span: Span {
                                start: object_span.start,
                                end: field_span.end,
                            },
                        };
                    }
                    self.expression(
                        Expression::Binary {
                            op,
                            left: Box::new(left),
                            right: Box::new(value),
                            span: Span {
                                start: object_span.start,
                                end: value_span.end,
                            },
                        },
                        Some(field_info.ty),
                    )?
                } else {
                    self.expression(value, Some(field_info.ty))?
                };
                if actual != field_info.ty {
                    return Err(diag(
                        "R0205",
                        format!(
                            "cannot assign `{}` to field `{final_field}` of type `{}`",
                            type_name(actual),
                            type_name(field_info.ty)
                        ),
                        value_span,
                    )
                    .with_help(format!(
                        "assign a `{}` value to `{final_field}`",
                        type_name(field_info.ty)
                    )));
                }
                Ok(IrStatement::FieldAssign {
                    slot: binding.slot + slot_offset,
                    ty: field_info.ty,
                    value,
                })
            }
            Statement::CompoundAssign {
                name,
                op,
                value,
                span,
            } => {
                let name_span = Span {
                    start: span.start,
                    end: span.start + name.len(),
                };
                let Some(binding) = self.names.get(&name).copied() else {
                    let error = self.unknown_variable(&name, name_span);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                if !binding.mutable {
                    let error = diag("R0204", format!("`{name}` is immutable"), name_span)
                        .with_help(format!(
                            "declare `mut {name} := ...` if this binding must be mutable"
                        ));
                    self.check_discarded_expression(value);
                    return Err(error);
                }
                let value_span = value.span();
                let expression = Expression::Binary {
                    op,
                    left: Box::new(Expression::Name(name, name_span)),
                    right: Box::new(value),
                    span: Span {
                        start: name_span.start,
                        end: value_span.end,
                    },
                };
                let (value, actual) = self.expression(expression, Some(binding.ty))?;
                if actual != binding.ty {
                    return Err(diag(
                        "R0205",
                        format!(
                            "compound assignment produces `{}` but the variable has type `{}`",
                            type_name(actual),
                            type_name(binding.ty)
                        ),
                        value_span,
                    ));
                }
                Ok(IrStatement::Assign {
                    slot: binding.slot,
                    ty: binding.ty,
                    value,
                    span,
                })
            }
            Statement::Print(expr, span) => {
                let (value, ty) = self.expression(expr, None).map_err(|mut e| {
                    if e.span == Span::default() {
                        e.span = span;
                    }
                    e
                })?;
                if type_contains_vec(ty, self.structs) {
                    return Err(diag("R0234", "`echo` cannot print `Vec` values yet", span)
                        .with_help("iterate the values manually and print each element"));
                }
                Ok(IrStatement::Print { value, ty })
            }
            Statement::PrintTemplate(parts, span) => {
                let mut lowered = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        PrintPart::Text(value) => lowered.push(IrPrintPart::Text(value)),
                        PrintPart::Value(expression) => match self.expression(expression, None) {
                            Ok((value, ty)) => {
                                if type_contains_vec(ty, self.structs) {
                                    return Err(diag(
                                        "R0234",
                                        "`echo` cannot print `Vec` values yet",
                                        span,
                                    )
                                    .with_help(
                                        "iterate the values manually and print each element",
                                    ));
                                }
                                lowered.push(IrPrintPart::Value { value, ty });
                            }
                            Err(mut diagnostic) if self.recover_block_errors => {
                                if diagnostic.span == Span::default() {
                                    diagnostic.span = span;
                                }
                                self.recovery_diagnostics.push(diagnostic);
                            }
                            Err(mut diagnostic) => {
                                if diagnostic.span == Span::default() {
                                    diagnostic.span = span;
                                }
                                return Err(diagnostic);
                            }
                        },
                    }
                }
                Ok(IrStatement::PrintTemplate(lowered))
            }
            Statement::Call {
                name,
                arguments,
                span,
                ..
            } => {
                let (target, arguments, _) = self.lower_call(name, arguments, span)?;
                Ok(IrStatement::Call { target, arguments })
            }
            Statement::MethodCall {
                value,
                name,
                arguments,
                span,
            } => {
                // Flatten `root.m1().m2()` into sequential calls that borrow the
                // same root local, so in-place chains mutate the receiver.
                let mut links = vec![(name, arguments, span)];
                let mut root_expression = *value;
                while let Expression::MethodCall {
                    value,
                    name,
                    arguments,
                    span,
                    ..
                } = root_expression
                {
                    links.push((name, arguments, span));
                    root_expression = *value;
                }
                links.reverse();
                let root_is_local = matches!(root_expression, Expression::Name(_, _))
                    || matches!(root_expression, Expression::Field { .. });
                if links.len() > 1 && !root_is_local {
                    return Err(diag(
                        "R0235",
                        "chained method receivers must be a local variable for now",
                        span,
                    ));
                }
                let root_name_for_mutable = match &root_expression {
                    Expression::Name(name, _) => Some(name.clone()),
                    Expression::Field { value, .. } => {
                        let mut current = &**value;
                        while let Expression::Field { value, .. } = current {
                            current = value;
                        }
                        match current {
                            Expression::Name(name, _) => Some(name.clone()),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let (mut root_ir, mut current_ty) = self.expression(root_expression, None)?;
                borrow_reference_receiver(&mut root_ir, current_ty);
                let mut statements = Vec::with_capacity(links.len());
                let link_count = links.len();
                for (link_index, (link_name, link_arguments, link_span)) in
                    links.into_iter().enumerate()
                {
                    let mutable = root_name_for_mutable
                        .clone()
                        .and_then(|name| self.names.get(&name).copied())
                        .is_none_or(|binding| {
                            binding.mutable || self.parameter_slots.contains(&binding.slot)
                        });
                    let lowered = self.lower_method_for_chain(
                        root_ir.clone(),
                        current_ty,
                        link_name,
                        link_arguments,
                        link_span,
                        mutable,
                    )?;
                    let (target, arguments, result) = lowered;
                    statements.push(IrStatement::Call { target, arguments });
                    if let Some(ty) = result {
                        current_ty = ty;
                        if link_index + 1 != link_count {
                            return Err(diag(
                                "R0235",
                                "only the last method in a chain may return a value for now",
                                link_span,
                            )
                            .with_help("void `mut self` methods keep chaining on the receiver"));
                        }
                    }
                }
                if statements.len() == 1 {
                    let Some(IrStatement::Call { target, arguments }) =
                        statements.into_iter().next()
                    else {
                        unreachable!("single statement is a call");
                    };
                    return Ok(IrStatement::Call { target, arguments });
                }
                Ok(IrStatement::Block(statements))
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                span,
            } => {
                let condition_span = condition.span();
                let condition_span = if condition_span == Span::default() {
                    span
                } else {
                    condition_span
                };
                let (condition, ty) = match self.expression(condition, Some(Type::Bool)) {
                    Ok(result) => result,
                    Err(error) if self.recover_block_errors => {
                        let outer_names = self.names.clone();
                        let _ = self.block(then_body);
                        self.names = outer_names.clone();
                        let _ = self.block(else_body);
                        self.names = outer_names;
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                if ty != Type::Bool {
                    let error = diag(
                        "R0207",
                        "`when` condition must have type `bool`",
                        condition_span,
                    )
                    .with_help("use a boolean expression, such as a comparison, for the condition");
                    if self.recover_block_errors {
                        let outer_names = self.names.clone();
                        let _ = self.block(then_body);
                        self.names = outer_names.clone();
                        let _ = self.block(else_body);
                        self.names = outer_names;
                    }
                    return Err(error);
                }
                // Receiver temporaries from the condition must run before it, not
                // inside the first statement of the branch.
                let setup = std::mem::take(&mut self.pending_statements);
                let outer_names = self.names.clone();
                let then_body = self.block(then_body)?;
                self.names = outer_names.clone();
                let else_body = self.block(else_body)?;
                self.names = outer_names;
                Ok(with_setup(
                    setup,
                    IrStatement::If {
                        condition,
                        then_body,
                        else_body,
                    },
                ))
            }
            Statement::While {
                condition,
                body,
                span,
            } => {
                let condition_span = condition.span();
                let condition_span = if condition_span == Span::default() {
                    span
                } else {
                    condition_span
                };
                let (condition, ty) = match self.expression(condition, Some(Type::Bool)) {
                    Ok(result) => result,
                    Err(error) if self.recover_block_errors => {
                        self.pending_statements.clear();
                        let outer_names = self.names.clone();
                        self.enter_loop();
                        let _ = self.block(body);
                        self.exit_loop();
                        self.names = outer_names;
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                let setup = std::mem::take(&mut self.pending_statements);
                if ty != Type::Bool {
                    let error = diag(
                        "R0207",
                        "`while` condition must have type `bool`",
                        condition_span,
                    )
                    .with_help("use a boolean expression, such as a comparison, for the condition");
                    if self.recover_block_errors {
                        let outer_names = self.names.clone();
                        self.enter_loop();
                        let _ = self.block(body);
                        self.exit_loop();
                        self.names = outer_names;
                    }
                    return Err(error);
                }
                let outer_names = self.names.clone();
                self.enter_loop();
                let body = self.block(body);
                self.exit_loop();
                let body = body?;
                self.names = outer_names;
                Ok(IrStatement::While {
                    setup,
                    condition,
                    body,
                })
            }
            Statement::For {
                name,
                name_span,
                start,
                end,
                inclusive,
                body,
                ..
            } => {
                if self.names.contains_key(&name) {
                    let error = diag(
                        "R0202",
                        format!("`{name}` is already declared in this scope"),
                        name_span,
                    )
                    .with_help("choose a unique name for the range loop variable");
                    if self.recover_block_errors {
                        self.recover_for_after_duplicate_name(&name, name_span, start, end, body);
                    }
                    return Err(error);
                }

                let start_hint = self.integer_type_hint(&end);
                // A bare literal start takes the end's type when the hint cannot see it,
                // as in `0..items.count()`.
                let start_literal = match &start {
                    Expression::Integer(value, span) => Some((*value, *span)),
                    _ => None,
                };
                let (start, mut ty) = match self.expression(start, start_hint) {
                    Ok(value) => value,
                    Err(error) if self.recover_block_errors => {
                        self.recover_for_after_invalid_start(&name, name_span, end, body);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                if !is_integer(ty) {
                    let error = diag(
                        "R0206",
                        format!(
                            "`for` range bounds must be integers, found `{}`",
                            type_name(ty)
                        ),
                        name_span,
                    )
                    .with_help("use matching signed or unsigned integer bounds for the range");
                    if self.recover_block_errors {
                        self.recover_for_after_invalid_start(&name, name_span, end, body);
                    }
                    return Err(error);
                }

                let (end, end_ty) = match self.expression(end, Some(ty)) {
                    Ok(value) => value,
                    Err(error) if self.recover_block_errors => {
                        self.recover_for_body(&name, ty, body);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                let mut start = start;
                if end_ty != ty
                    && is_integer(end_ty)
                    && let Some((value, span)) = start_literal
                    && let Ok((retyped, retyped_ty)) =
                        self.expression(Expression::Integer(value, span), Some(end_ty))
                    && retyped_ty == end_ty
                {
                    start = retyped;
                    ty = end_ty;
                }
                if end_ty != ty {
                    let error = diag(
                        "R0206",
                        format!(
                            "`for` range bounds must have the same integer type, found `{}` and `{}`",
                            type_name(ty),
                            type_name(end_ty)
                        ),
                        name_span,
                    )
                    .with_help("use the same integer type for both range bounds");
                    if self.recover_block_errors {
                        self.recover_for_body(&name, ty, body);
                    }
                    return Err(error);
                }

                let setup = std::mem::take(&mut self.pending_statements);
                let slot = self.allocate(ty);
                let end_slot = self.allocate(ty);
                let outer_names = self.names.clone();
                self.names.insert(
                    name,
                    Binding {
                        slot,
                        ty,
                        mutable: false,
                    },
                );
                self.enter_loop();
                let body = self.block(body);
                self.exit_loop();
                self.names = outer_names;
                let body = body?;
                Ok(with_setup(
                    setup,
                    IrStatement::For {
                        slot,
                        end_slot,
                        ty,
                        start,
                        end,
                        inclusive,
                        body,
                    },
                ))
            }
            Statement::ForEach {
                name,
                name_span,
                collection,
                body,
                ..
            } => {
                if self.names.contains_key(&name) {
                    let error = diag(
                        "R0202",
                        format!("`{name}` is already declared in this scope"),
                        name_span,
                    )
                    .with_help("choose a unique name for the collection loop variable");
                    if self.recover_block_errors {
                        if let Err(error) = self.expression(collection, None) {
                            self.recovery_diagnostics.push(error);
                        }
                        let outer = self.names.clone();
                        self.enter_loop();
                        let _ = self.block(body);
                        self.exit_loop();
                        self.names = outer;
                    }
                    return Err(error);
                }
                let collection_span = collection.span();
                let (collection, collection_type) = self.expression(collection, None)?;
                let setup = std::mem::take(&mut self.pending_statements);
                let elem = match collection_type {
                    Type::Vec(elem_id) => vec_elem(elem_id),
                    Type::Slice(elem_id) => vec_elem(elem_id),
                    Type::OwnedString => Type::Char,
                    Type::Str => Type::Char,
                    _ => {
                        return Err(diag(
                            "R0206",
                            format!(
                                "`for element in ...` requires a Vec, String, or str, found `{}`",
                                type_name(collection_type)
                            ),
                            collection_span,
                        )
                        .with_help("iterate a `Vec<T>`, `String`, or `str` directly, or use a numeric range with `..`"));
                    }
                };
                let vector_slot = self.allocate(collection_type);
                let index_slot = self.allocate(Type::U64);
                let end_slot = self.allocate(Type::U64);
                let item_slot = self.allocate(elem);
                let outer_names = self.names.clone();
                self.names.insert(
                    name,
                    Binding {
                        slot: item_slot,
                        ty: elem,
                        mutable: false,
                    },
                );
                self.enter_loop();
                let body = self.block(body);
                self.exit_loop();
                self.names = outer_names;
                let body = body?;

                let vector_value = IrExpression::Local {
                    slot: vector_slot,
                    ty: collection_type,
                    span: collection_span,
                };
                let (length, item_value) = match collection_type {
                    Type::Vec(elem_id) => (
                        IrExpression::Call {
                            target: IrCallTarget::Vec(VecOp::Len, elem_id),
                            arguments: vec![vector_value.clone()],
                            return_type: Type::U64,
                        },
                        IrExpression::Call {
                            target: IrCallTarget::Vec(VecOp::Take, elem_id),
                            arguments: vec![
                                vector_value.clone(),
                                IrExpression::Integer(0, Type::U64),
                            ],
                            return_type: elem,
                        },
                    ),
                    Type::OwnedString => (
                        IrExpression::Call {
                            target: IrCallTarget::String(StringOp::CharCount),
                            arguments: vec![vector_value.clone()],
                            return_type: Type::U64,
                        },
                        IrExpression::Call {
                            target: IrCallTarget::String(StringOp::CharAt),
                            arguments: vec![
                                vector_value.clone(),
                                IrExpression::Local {
                                    slot: index_slot,
                                    ty: Type::U64,
                                    span: collection_span,
                                },
                            ],
                            return_type: Type::Char,
                        },
                    ),
                    Type::Str => (
                        IrExpression::Call {
                            target: IrCallTarget::String(StringOp::StrCharCount),
                            arguments: vec![vector_value.clone()],
                            return_type: Type::U64,
                        },
                        IrExpression::Call {
                            target: IrCallTarget::String(StringOp::StrCharAt),
                            arguments: vec![
                                vector_value.clone(),
                                IrExpression::Local {
                                    slot: index_slot,
                                    ty: Type::U64,
                                    span: collection_span,
                                },
                            ],
                            return_type: Type::Char,
                        },
                    ),
                    Type::Slice(elem_id) => (
                        IrExpression::Call {
                            target: IrCallTarget::SliceLen,
                            arguments: vec![vector_value.clone()],
                            return_type: Type::U64,
                        },
                        IrExpression::SliceIndex {
                            slice: Box::new(vector_value.clone()),
                            index: Box::new(IrExpression::Local {
                                slot: index_slot,
                                ty: Type::U64,
                                span: collection_span,
                            }),
                            ty: vec_elem(elem_id),
                        },
                    ),
                    _ => unreachable!("iterable type checked above"),
                };
                let item = IrStatement::Let {
                    slot: item_slot,
                    ty: elem,
                    value: item_value,
                    span: name_span,
                };
                let mut loop_body = Vec::with_capacity(body.len() + 1);
                loop_body.push(item);
                loop_body.extend(body);
                Ok(with_setup(
                    setup,
                    IrStatement::Block(vec![
                        IrStatement::Let {
                            slot: vector_slot,
                            ty: collection_type,
                            value: collection,
                            span: collection_span,
                        },
                        IrStatement::For {
                            slot: index_slot,
                            end_slot,
                            ty: Type::U64,
                            start: IrExpression::Integer(0, Type::U64),
                            end: length,
                            inclusive: false,
                            body: loop_body,
                        },
                    ]),
                ))
            }
            Statement::Defer { body, span } => {
                if matches!(self.return_type, Some(Type::Reference(_, _))) {
                    return Err(diag(
                        "R0269",
                        "a function returning a reference cannot use `defer`",
                        span,
                    )
                    .with_help("return an owned value, or move the cleanup to the caller"));
                }
                let deferred = DeferredBody {
                    body,
                    names: self.names.clone(),
                };
                // Lower the body now, so its own errors are reported where it is written.
                self.lower_deferred_body(deferred.clone())?;
                match self.defer_frames.last_mut() {
                    Some(frame) => frame.push(deferred),
                    None => {
                        return Err(diag("R0270", "`defer` must be inside a block", span));
                    }
                }
                Ok(IrStatement::Block(Vec::new()))
            }
            Statement::Break(span) => {
                if self.loop_depth == 0 {
                    if self.defer_nesting > 0 {
                        return Err(diag("R0268", "`break` cannot leave a `defer` block", span)
                            .with_help("move the loop inside the `defer` block"));
                    }
                    return Err(
                        diag("R0016", "`break` can only be used inside a loop", span)
                            .with_help("move `break` into the body of a `while` or `for` loop"),
                    );
                }
                let first_frame = self.loop_defer_bases.last().copied().unwrap_or(0);
                let leaving = self.leaving_defers(first_frame)?;
                Ok(exit_after(leaving, IrStatement::Break))
            }
            Statement::Continue(span) => {
                if self.loop_depth == 0 {
                    if self.defer_nesting > 0 {
                        return Err(diag("R0268", "`continue` cannot leave a `defer` block", span)
                            .with_help("move the loop inside the `defer` block"));
                    }
                    return Err(
                        diag("R0017", "`continue` can only be used inside a loop", span)
                            .with_help("move `continue` into the body of a `while` or `for` loop"),
                    );
                }
                let first_frame = self.loop_defer_bases.last().copied().unwrap_or(0);
                let leaving = self.leaving_defers(first_frame)?;
                Ok(exit_after(leaving, IrStatement::Continue))
            }
            Statement::Return { value, span } => {
                if self.defer_nesting > 0 {
                    return Err(diag("R0267", "`return` cannot leave a `defer` block", span)
                        .with_help("return from the enclosing function after the `defer` block"));
                }
                let value = match (self.return_type, value) {
                    (None, Some(value)) => {
                        let error = diag(
                            "R0013",
                            "a function with a return value must declare its type after `->`",
                            span,
                        )
                        .with_help(format!(
                            "add a `-> TYPE` result type to `{}` before using `return VALUE`",
                            self.function_name
                        ));
                        self.check_discarded_expression(value);
                        return Err(error);
                    }
                    (Some(expected), None) => {
                        return Err(diag(
                            "R0216",
                            format!(
                                "`return` in a function returning `{}` must provide a value",
                                type_name(expected)
                            ),
                            span,
                        )
                        .with_help(format!("return a `{}` value", type_name(expected))));
                    }
                    (None, None) => None,
                    (Some(expected), Some(value)) => {
                        let value_span = value.span();
                        let (value, actual) = self.expression(value, Some(expected))?;
                        self.validate_reference_return(&value, value_span)?;
                        if actual != expected {
                            return Err(diag(
                                "R0205",
                                format!(
                                    "function `{}` returns `{}` but its declared result is `{}`",
                                    self.function_name,
                                    type_name(actual),
                                    type_name(expected)
                                ),
                                value_span,
                            )
                            .with_help(format!(
                                "return a `{}` value or change the function's declared result type",
                                type_name(expected)
                            )));
                        }
                        Some(value)
                    }
                };
                let leaving = self.leaving_defers(0)?;
                let Some(value) = value else {
                    return Ok(exit_after(
                        leaving,
                        IrStatement::Return { value: None },
                    ));
                };
                if leaving.is_empty() {
                    return Ok(IrStatement::Return { value: Some(value) });
                }
                // The value is computed before the `defer` blocks run, so they cannot change it.
                let ty = self
                    .return_type
                    .expect("a returned value has a declared result type");
                let slot = self.allocate(ty);
                let mut statements = vec![IrStatement::Let {
                    slot,
                    ty,
                    value,
                    span,
                }];
                statements.extend(leaving);
                statements.push(IrStatement::Return {
                    value: Some(IrExpression::Local { slot, ty, span }),
                });
                Ok(IrStatement::Block(statements))
            }
        }
    }

    fn validate_reference_return(
        &self,
        value: &IrExpression,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if !matches!(self.return_type, Some(Type::Reference(_, _))) {
            return Ok(());
        }
        let parameter_origin = match value {
            IrExpression::Local {
                slot,
                ty: Type::Reference(_, _),
                ..
            } => self.parameter_slots.contains(slot),
            IrExpression::SliceElementAddress { slice, .. } => matches!(
                &**slice,
                IrExpression::Local {
                    slot,
                    ty: Type::Slice(_),
                    ..
                } if self.parameter_slots.contains(slot)
            ),
            // A call's result provenance is not yet tracked. Treat it as
            // unknown so a callee cannot smuggle a reference to this
            // function's local stack frame through a reference return.
            IrExpression::Call { .. } => false,
            IrExpression::If {
                then_value,
                else_value,
                ..
            } => {
                self.validate_reference_return(then_value, span).is_ok()
                    && self.validate_reference_return(else_value, span).is_ok()
            }
            _ => false,
        };
        if parameter_origin {
            Ok(())
        } else {
            Err(diag(
                "R0248",
                "a reference to a local value cannot escape its stack frame",
                span,
            )
            .with_help("return a reference derived directly from a reference parameter"))
        }
    }

    fn expression(
        &mut self,
        expression: Expression,
        expected: Option<Type>,
    ) -> Result<(IrExpression, Type), Diagnostic> {
        match expression {
            Expression::Integer(n, span) => {
                let ty = expected.filter(|ty| is_integer(*ty)).unwrap_or(Type::I64);
                let (min, max) = integer_bounds(ty);
                if (n as i128) < min || (n as i128) > max {
                    return Err(diag(
                        "R0206",
                        format!("integer literal is outside the `{}` range", type_name(ty)),
                        span,
                    )
                    .with_help(format!(
                        "use a value from `{min}` to `{max}`, or choose a wider integer type"
                    )));
                }
                Ok((IrExpression::Integer(n as i128, ty), ty))
            }
            Expression::Float(n, span) => {
                let ty = expected
                    .filter(|ty| matches!(ty, Type::F32 | Type::F64))
                    .unwrap_or(Type::F64);
                if !n.is_finite() || (ty == Type::F32 && !(n as f32).is_finite()) {
                    return Err(diag(
                        "R0206",
                        format!(
                            "floating-point literal is outside the `{}` range",
                            type_name(ty)
                        ),
                        span,
                    )
                    .with_help(if ty == Type::F32 {
                        "use a smaller finite value or declare it as `f64`"
                    } else {
                        "use a finite value representable by `f64`"
                    }));
                }
                Ok((IrExpression::Float(n, ty), ty))
            }
            Expression::LayoutOf {
                ty,
                alignment,
                span: _,
            } => {
                let ty = self.resolve_type_name(&ty)?;
                let (size, align) = type_layout(ty, self.structs);
                let value = if alignment { align } else { size };
                Ok((IrExpression::Integer(value as i128, Type::U64), Type::U64))
            }
            Expression::String(s, _) => Ok((IrExpression::String(s), Type::Str)),
            Expression::Character(value, _) => Ok((IrExpression::Character(value), Type::Char)),
            Expression::Boolean(value, _) => Ok((IrExpression::Boolean(value), Type::Bool)),
            Expression::Name(name, span) => {
                let binding = self.names.get(&name).copied();
                let Some(binding) = binding else {
                    if let Some(expected_type) = expected
                        && matches!(expected_type, Type::FunctionPointer(_))
                        && let Some(coerced) =
                            self.coerce_function_pointer(&name, &expected_type, span)?
                    {
                        return Ok((coerced, expected_type));
                    }
                    return Err(self.unknown_variable(&name, span));
                };
                Ok((
                    IrExpression::Local {
                        slot: binding.slot,
                        ty: binding.ty,
                        span,
                    },
                    binding.ty,
                ))
            }
            Expression::AddressOf {
                mutable,
                raw,
                value,
                span,
            } => {
                let (name, name_span) = match *value {
                    Expression::Index {
                        value: slice,
                        index,
                        span: index_span,
                    } => {
                        if mutable || raw {
                            return Err(diag(
                                "R0206",
                                "slice elements can only be borrowed through immutable references",
                                span,
                            ));
                        }
                        let (slice, slice_ty) = self.expression(*slice, None)?;
                        let Type::Slice(slice_id) = slice_ty else {
                            return Err(diag(
                                "R0206",
                                "address-of indexing currently requires a borrowed slice",
                                index_span,
                            ));
                        };
                        let element = vec_elem(slice_id);
                        if !reference_pointee_supported(element) {
                            return Err(diag(
                                "R0206",
                                format!(
                                    "references to `{}` are not supported yet",
                                    type_name(element)
                                ),
                                index_span,
                            )
                            .with_help(
                                "borrowed slice references currently support scalar elements",
                            ));
                        }
                        let (index, index_ty) = self.expression(*index, None)?;
                        if !is_integer(index_ty) {
                            return Err(diag(
                                "R0242",
                                "slice index must be an integer",
                                index_span,
                            ));
                        }
                        let pointer_type = Type::Reference(intern_pointer_target(element), false);
                        return Ok((
                            IrExpression::SliceElementAddress {
                                slice: Box::new(slice),
                                index: Box::new(index),
                                ty: element,
                                span,
                            },
                            pointer_type,
                        ));
                    }
                    Expression::Name(name, name_span) => (name, name_span),
                    _ => {
                        return Err(diag(
                            "R0206",
                            "address-of currently requires a local variable or a borrowed slice element",
                            span,
                        ));
                    }
                };
                let binding = *self
                    .names
                    .get(&name)
                    .ok_or_else(|| self.unknown_variable(&name, name_span))?;
                if mutable && !raw && !binding.mutable {
                    return Err(diag(
                        "R0206",
                        "cannot create a mutable reference to an immutable binding",
                        name_span,
                    )
                    .with_help("declare the value with `mut` before borrowing it mutably"));
                }
                if !matches!(binding.ty, Type::Struct(_))
                    && !reference_pointee_supported_in(binding.ty, self.structs)
                {
                    return Err(diag(
                        "R0206",
                        format!(
                            "references to `{}` are not supported yet",
                            type_name(binding.ty)
                        ),
                        name_span,
                    )
                    .with_help(
                        "the current backend supports references to scalars and structures",
                    ));
                }
                self.addressed_slot_types[binding.slot] = Some(binding.ty);
                let target = intern_pointer_target(binding.ty);
                let pointer_type = if raw {
                    Type::RawPointer(target)
                } else {
                    Type::Reference(target, mutable)
                };
                Ok((
                    IrExpression::AddressOf {
                        slot: binding.slot,
                        ty: binding.ty,
                        pointer_type,
                        span,
                    },
                    pointer_type,
                ))
            }
            Expression::Dereference(pointer, span) => {
                let (pointer, pointer_type) = self.expression(*pointer, None)?;
                let pointee = match pointer_type {
                    Type::Reference(id, _) | Type::RawPointer(id) => pointer_target(id),
                    _ => {
                        return Err(diag(
                            "R0206",
                            "dereference requires a reference or raw pointer",
                            span,
                        ));
                    }
                };
                if !reference_pointee_supported_in(pointee, self.structs)
                    || matches!(pointee, Type::Struct(_))
                        && !pointer_record_is_copy(pointee, self.structs)
                {
                    return Err(diag(
                        "R0206",
                        format!(
                            "dereferencing `{}` is not supported yet",
                            type_name(pointee)
                        ),
                        span,
                    ));
                }
                Ok((
                    IrExpression::Dereference {
                        pointer: Box::new(pointer),
                        ty: pointee,
                    },
                    pointee,
                ))
            }
            Expression::Tuple(elements, span) => {
                let expected_fields = match expected {
                    Some(Type::Struct(struct_id))
                        if self.structs[struct_id].name.starts_with("$RynTuple#") =>
                    {
                        let fields = self.structs[struct_id].fields.clone();
                        if fields.len() != elements.len() {
                            return Err(diag(
                                "R0205",
                                format!(
                                    "tuple has {} elements but the expected tuple has {}",
                                    elements.len(),
                                    fields.len()
                                ),
                                span,
                            ));
                        }
                        Some((struct_id, fields))
                    }
                    Some(other) => {
                        return Err(diag(
                            "R0205",
                            format!(
                                "tuple literal does not match expected type `{}`",
                                type_name(other)
                            ),
                            span,
                        ));
                    }
                    None => None,
                };
                let mut values = Vec::with_capacity(elements.len());
                let mut field_types = Vec::with_capacity(elements.len());
                for (index, element) in elements.into_iter().enumerate() {
                    let expected_type =
                        expected_fields.as_ref().map(|(_, fields)| fields[index].ty);
                    let (value, value_type) = self.expression(element, expected_type)?;
                    values.push(value);
                    field_types.push(value_type);
                }
                let struct_id = if let Some((struct_id, _)) = expected_fields {
                    struct_id
                } else if let Some(struct_id) = self.structs.iter().position(|definition| {
                    definition.name.starts_with("$RynTuple#")
                        && definition.fields.len() == field_types.len()
                        && definition
                            .fields
                            .iter()
                            .zip(&field_types)
                            .all(|(field, ty)| field.ty == *ty)
                }) {
                    struct_id
                } else {
                    let mut slot_offset = 0;
                    let fields = field_types
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| {
                            let field = RynStructField {
                                name: format!("_{index}"),
                                ty: *ty,
                                slot_offset,
                                public: false,
                            };
                            slot_offset += storage_slot_width(*ty, self.structs);
                            field
                        })
                        .collect();
                    let struct_id = self.structs.len();
                    self.structs.push(RynStruct {
                        name: format!("$RynTuple#inferred{struct_id}"),
                        fields,
                        slot_count: slot_offset,
                        repr_c: false,
                        drop_function: None,
                        derives_clone: false,
                        derives_hash: false,
                        module_path: String::new(),
                    });
                    struct_id
                };
                let fields = self.structs[struct_id]
                    .fields
                    .iter()
                    .zip(values)
                    .enumerate()
                    .map(|(field_index, (_, value))| (field_index, value))
                    .collect();
                Ok((
                    IrExpression::StructValue { struct_id, fields },
                    Type::Struct(struct_id),
                ))
            }
            Expression::ArrayRepeat {
                value,
                length,
                span,
            } => {
                let length = usize::try_from(length).map_err(|_| {
                    diag("R0012", "array length exceeds the host limit", span)
                        .with_help("use a smaller fixed array length")
                })?;
                let (element, element_ty) = match expected {
                    Some(Type::Array(id)) => {
                        let (element_ty, expected_len) = array_info(id);
                        if length != expected_len {
                            return Err(diag(
                                "R0240",
                                format!(
                                    "array repeat has length {length} but the expected array needs {expected_len}"
                                ),
                                span,
                            ));
                        }
                        let value_span = value.span();
                        let (element, actual) = self.expression(*value, Some(element_ty))?;
                        if actual != element_ty {
                            return Err(diag(
                                "R0205",
                                format!(
                                    "array element requires `{}` but has type `{}`",
                                    type_name(element_ty),
                                    type_name(actual)
                                ),
                                value_span,
                            ));
                        }
                        (element, element_ty)
                    }
                    Some(other) => {
                        return Err(diag(
                            "R0205",
                            format!(
                                "array repeat does not match expected type `{}`",
                                type_name(other)
                            ),
                            span,
                        ));
                    }
                    None => self.expression(*value, None)?,
                };
                validate_array_literal_elem(element_ty, self.structs, span)?;
                if !repeat_element_is_copy(element_ty, self.structs) {
                    return Err(diag(
                        "R0256",
                        format!(
                            "array repeat cannot copy `{}` elements",
                            type_name(element_ty)
                        ),
                        span,
                    )
                    .with_help(
                        "repeat expressions evaluate the element once and copy the bits; write an element list for String, Vec, Map, Set, or enum values",
                    ));
                }
                let ty = Type::Array(intern_array(element_ty, length));
                Ok((
                    IrExpression::ArrayRepeat {
                        value: Box::new(element),
                        length,
                    },
                    ty,
                ))
            }
            Expression::ArrayLiteral(elements, span) => {
                let mut elements = elements.into_iter();
                let (element_ty, length, first_value) = match expected {
                    Some(Type::Array(id)) => {
                        let (element, length) = array_info(id);
                        if elements.len() != length {
                            return Err(diag(
                                "R0240",
                                format!(
                                    "array literal has {} elements but the expected array needs {length}",
                                    elements.len()
                                ),
                                span,
                            ));
                        }
                        (element, length, None)
                    }
                    Some(other) => {
                        return Err(diag(
                            "R0205",
                            format!(
                                "array literal does not match expected type `{}`",
                                type_name(other)
                            ),
                            span,
                        ));
                    }
                    None => {
                        let length = elements.len();
                        let Some(first) = elements.next() else {
                            return Err(diag(
                                "R0240",
                                "cannot infer the type and length of an empty array literal",
                                span,
                            )
                            .with_help(
                                "provide an explicit type such as `values: [i32; 0] = []`",
                            ));
                        };
                        let (first_value, element) = self.expression(first, None)?;
                        (element, length, Some(first_value))
                    }
                };
                validate_array_literal_elem(element_ty, self.structs, span)?;
                let mut values = Vec::with_capacity(length);
                if let Some(first_value) = first_value {
                    values.push(first_value);
                }
                for value in elements {
                    let value_span = value.span();
                    let (value, actual) = self.expression(value, Some(element_ty))?;
                    if actual != element_ty {
                        return Err(diag(
                            "R0205",
                            format!(
                                "array element requires `{}` but has type `{}`",
                                type_name(element_ty),
                                type_name(actual)
                            ),
                            value_span,
                        ));
                    }
                    values.push(value);
                }
                let ty = Type::Array(intern_array(element_ty, length));
                Ok((IrExpression::ArrayValue(values), ty))
            }
            Expression::Index { value, index, span } => {
                let (array, array_ty) = self.expression(*value, None)?;
                if let Type::Slice(slice_id) = array_ty {
                    let (index, index_ty) = self.expression(*index, None)?;
                    if !is_integer(index_ty) {
                        return Err(diag("R0242", "slice index must be an integer", span));
                    }
                    let element = vec_elem(slice_id);
                    return Ok((
                        IrExpression::SliceIndex {
                            slice: Box::new(array),
                            index: Box::new(index),
                            ty: element,
                        },
                        element,
                    ));
                }
                if let Type::Vec(elem_id) = array_ty {
                    // `items[i]` reads like `items.index(i)`, cloning the element.
                    let mut receiver = array;
                    borrow_reference_receiver(&mut receiver, array_ty);
                    let (target, arguments, result) = self.lower_vec_method(
                        receiver,
                        elem_id,
                        "index".into(),
                        vec![*index],
                        span,
                        true,
                    )?;
                    let return_type = result.ok_or_else(|| {
                        diag("R0241", "this Vec element type cannot be indexed", span)
                    })?;
                    return Ok((
                        IrExpression::Call {
                            target,
                            arguments,
                            return_type,
                        },
                        return_type,
                    ));
                }
                let Type::Array(array_id) = array_ty else {
                    return Err(diag(
                        "R0241",
                        "indexing requires a fixed array, slice, or Vec",
                        span,
                    ));
                };
                let (element, length) = array_info(array_id);
                let mut seen_structs = HashSet::new();
                let mut seen_enums = HashSet::new();
                if contains_custom_drop(
                    element,
                    self.structs,
                    self.enums,
                    &mut seen_structs,
                    &mut seen_enums,
                    0,
                ) {
                    return Err(diag(
                        "R0255",
                        "custom-destructor array elements cannot be read by index yet",
                        span,
                    )
                    .with_help(
                        "move or pass the whole array; indexed reads would duplicate an owner",
                    ));
                }
                let (index, index_ty) = self.expression(*index, None)?;
                if !is_integer(index_ty) {
                    return Err(diag("R0242", "array index must be an integer", span));
                }
                Ok((
                    IrExpression::ArrayIndex {
                        array: Box::new(array),
                        index: Box::new(index),
                        ty: element,
                        length,
                    },
                    element,
                ))
            }
            Expression::StructLiteral { name, fields, span } => {
                if self.recover_block_errors {
                    return self.struct_literal_recovering(name, fields, span, expected);
                }
                let struct_id = self.scoped_struct_id(&name).ok_or_else(|| {
                    diag("R0227", format!("unknown structure `{name}`"), span)
                        .with_help("declare the structure before constructing it")
                })?;
                let ty = Type::Struct(struct_id);
                if expected.is_some_and(|expected| expected != ty) {
                    return Err(diag(
                        "R0205",
                        format!(
                            "structure literal `{name}` does not match the expected type `{}`",
                            expected
                                .map(type_name)
                                .unwrap_or_else(|| "unknown".to_owned())
                        ),
                        span,
                    ));
                }
                let definition = self.structs[struct_id].clone();
                let mut initialized = vec![false; definition.fields.len()];
                let mut values = Vec::with_capacity(definition.fields.len());
                for (field_name, value, field_span) in fields {
                    let Some((field_index, field_info)) = definition
                        .fields
                        .iter()
                        .enumerate()
                        .find(|(_, field)| field.name == field_name)
                    else {
                        return Err(self.unknown_struct_field(struct_id, &field_name, field_span));
                    };
                    if initialized[field_index] {
                        return Err(diag(
                            "R0228",
                            format!("field `{field_name}` is initialized more than once"),
                            field_span,
                        )
                        .with_help("remove the duplicate field initializer"));
                    }
                    let value_span = value.span();
                    let (value, actual) = self.expression(value, Some(field_info.ty))?;
                    if actual != field_info.ty {
                        return Err(diag(
                            "R0205",
                            format!(
                                "field `{field_name}` requires `{}` but the value has type `{}`",
                                type_name(field_info.ty),
                                type_name(actual)
                            ),
                            value_span,
                        )
                        .with_help(format!(
                            "provide a `{}` value for `{field_name}`",
                            type_name(field_info.ty)
                        )));
                    }
                    initialized[field_index] = true;
                    values.push((field_index, value));
                }
                for (index, field_info) in definition.fields.iter().enumerate() {
                    if !initialized[index] {
                        return Err(diag(
                            "R0229",
                            format!("missing field `{}` in `{name}` literal", field_info.name),
                            span,
                        )
                        .with_help(format!("provide a value for `{}`", field_info.name)));
                    }
                }
                Ok((
                    IrExpression::StructValue {
                        struct_id,
                        fields: values,
                    },
                    ty,
                ))
            }
            Expression::Field {
                value,
                name,
                name_span,
                span,
            } => {
                let (value, actual) = self.expression(*value, None)?;
                let referenced = match actual {
                    Type::Struct(_) => None,
                    Type::Reference(target, _) | Type::RawPointer(target) => {
                        match pointer_target(target) {
                            Type::Struct(struct_id)
                                if matches!(actual, Type::RawPointer(_))
                                    && !self.structs[struct_id].repr_c =>
                            {
                                return Err(diag(
                                    "R0224",
                                    "raw pointer field access requires a #[repr(C)] structure",
                                    span,
                                ));
                            }
                            Type::Struct(struct_id) => Some(struct_id),
                            _ => {
                                return Err(diag(
                                    "R0224",
                                    "field access requires a structure value",
                                    span,
                                )
                                .with_help(
                                    "access a field on a value declared with a structure type",
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(
                            diag("R0224", "field access requires a structure value", span)
                                .with_help(
                                    "access a field on a value declared with a structure type",
                                ),
                        );
                    }
                };
                let struct_id = referenced.unwrap_or_else(|| match actual {
                    Type::Struct(id) => id,
                    _ => unreachable!("struct receiver checked above"),
                });
                let (field_index, field_info) = self.structs[struct_id]
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.name == name)
                    .ok_or_else(|| self.unknown_struct_field(struct_id, &name, name_span))?;
                self.check_field_visibility(struct_id, field_index, name_span)?;
                if let Some(struct_id) = referenced {
                    return Ok((
                        IrExpression::ReferenceField {
                            pointer: Box::new(value),
                            struct_id,
                            field_index,
                            ty: field_info.ty,
                            span,
                            borrowed: false,
                        },
                        field_info.ty,
                    ));
                }
                Ok((
                    IrExpression::Field {
                        value: Box::new(value),
                        struct_id,
                        field_index,
                        ty: field_info.ty,
                        span,
                    },
                    field_info.ty,
                ))
            }
            Expression::VecConstructor { element, span } => {
                let elem = self.resolve_type_name(&element).map_err(|mut error| {
                    error.span = span;
                    error
                })?;
                validate_vec_elem(&elem, span)?;
                let vec_id = intern_vec_elem(elem);
                let ty = Type::Vec(vec_id);
                let mut seen_structs = HashSet::new();
                let mut seen_enums = HashSet::new();
                if contains_custom_drop(
                    elem,
                    self.structs,
                    self.enums,
                    &mut seen_structs,
                    &mut seen_enums,
                    0,
                ) && !custom_drop_vec_only(ty, self.structs, self.enums)
                {
                    return Err(diag(
                        "R0255",
                        "Vec elements cannot contain nested custom-destructor values yet",
                        span,
                    )
                    .with_help("use custom-drop resources directly as Vec elements, or keep nested resources in a regular struct value"));
                }
                Ok((
                    IrExpression::Call {
                        target: IrCallTarget::Vec(VecOp::New, vec_id),
                        arguments: Vec::new(),
                        return_type: ty,
                    },
                    ty,
                ))
            }
            Expression::MapConstructor { key, value, span } => {
                let key = self.resolve_type_name(&key).map_err(|mut error| {
                    error.span = span;
                    error
                })?;
                let value = self.resolve_type_name(&value).map_err(|mut error| {
                    error.span = span;
                    error
                })?;
                validate_map_types_with_structs(key, value, span, self.structs)?;
                if !map_struct_value_supported(
                    Type::Map(intern_map(key, value)),
                    self.structs,
                    self.enums,
                ) {
                    return Err(diag(
                        "R0244",
                        "Map struct values must support a generated clone/drop plan; this value type has no supported Map layout",
                        span,
                    ));
                }
                let map_id = intern_map(key, value);
                let ty = Type::Map(map_id);
                Ok((
                    IrExpression::Call {
                        target: IrCallTarget::Map(MapOp::New, map_id),
                        arguments: Vec::new(),
                        return_type: ty,
                    },
                    ty,
                ))
            }
            Expression::SetConstructor { element, span } => {
                let element = self.resolve_type_name(&element).map_err(|mut error| {
                    error.span = span;
                    error
                })?;
                validate_map_key_lenient(element, span)?;
                let map_id = intern_map(element, Type::Bool);
                let ty = Type::Set(map_id);
                Ok((
                    IrExpression::Call {
                        target: IrCallTarget::Set(MapOp::New, map_id),
                        arguments: Vec::new(),
                        return_type: ty,
                    },
                    ty,
                ))
            }
            Expression::Call {
                name,
                arguments,
                span,
                ..
            } => {
                if let Some((enum_name, variant)) = name.rsplit_once("::")
                    && (self.scoped_enum_id(enum_name).is_some()
                        || matches!(enum_name, "Option" | "Result")
                        || expected.is_some_and(|expected| {
                            matches!(expected, Type::Enum(id)
                                if is_generic_enum_name(&self.enums[id].name, enum_name))
                        }))
                {
                    return self.expression(
                        Expression::EnumConstruct {
                            enum_name: enum_name.to_owned(),
                            variant: variant.to_owned(),
                            arguments,
                            span,
                        },
                        expected,
                    );
                }
                if name == "Vec" && arguments.is_empty() {
                    return match expected.filter(|ty| matches!(ty, Type::Vec(_))) {
                        Some(ty @ Type::Vec(_)) => Ok((
                            IrExpression::Call {
                                target: match ty {
                                    Type::Vec(id) => IrCallTarget::Vec(VecOp::New, id),
                                    _ => unreachable!(),
                                },
                                arguments: Vec::new(),
                                return_type: ty,
                            },
                            ty,
                        )),
                        _ => Err(diag(
                            "R0234",
                            "cannot infer the element type of an empty `Vec()`",
                            span,
                        )
                        .with_help("use `Vec<T>()` or annotate the variable, e.g. `mut values: Vec<i32> = Vec()`")),
                    };
                }
                let (target, lowered, return_type) =
                    self.lower_call(name.clone(), arguments, span)?;
                let return_type = return_type.ok_or_else(|| {
                    diag(
                        "R0215",
                        format!("function `{name}` does not return a value and cannot be used as an expression"),
                        span,
                    )
                    .with_help(format!(
                        "call `{name}` as a statement or give it a return type and value"
                    ))
                })?;
                Ok((
                    IrExpression::Call {
                        target,
                        arguments: lowered,
                        return_type,
                    },
                    return_type,
                ))
            }
            Expression::EnumConstruct {
                enum_name,
                variant,
                arguments,
                span,
            } => {
                let expected_generic_id = expected.and_then(|expected| match expected {
                    Type::Enum(id) if is_generic_enum_name(&self.enums[id].name, &enum_name) => {
                        Some(id)
                    }
                    _ => None,
                });
                let enum_id = self.scoped_enum_id(&enum_name).or(expected_generic_id);
                let Some(enum_id) = enum_id else {
                    // `Type::CONSTANT` lowers to a zero-argument function; the
                    // parser sees the same `Type::Name` path as enum variants.
                    let constant_name = format!("{enum_name}::{variant}");
                    if arguments.is_empty()
                        && let Some(constant) = self.scoped_zero_argument_function(&constant_name)
                    {
                        return Ok(constant);
                    }
                    return Err(diag("R0230", format!("unknown enum `{enum_name}`"), span)
                        .with_help(if matches!(enum_name.as_str(), "Option" | "Result") {
                            "give this generic variant an expected `Option<T>` or `Result<T, E>` type"
                        } else {
                            "declare the enum before constructing it"
                        }));
                };
                let definition = &self.enums[enum_id];
                let Some(variant_index) = definition
                    .variants
                    .iter()
                    .position(|candidate| candidate.name == *variant)
                else {
                    return Err(diag(
                        "R0233",
                        format!("enum `{enum_name}` has no variant `{variant}`"),
                        span,
                    ));
                };
                let variant_definition = &definition.variants[variant_index];
                if variant_definition.fields.iter().any(|field| {
                    contains_custom_drop(
                        *field,
                        self.structs,
                        self.enums,
                        &mut HashSet::new(),
                        &mut HashSet::new(),
                        0,
                    )
                }) {
                    return Err(diag(
                        "R0255",
                        "custom-destructor resources cannot be constructed inside enum payloads",
                        span,
                    ));
                }
                if arguments.len() != variant_definition.fields.len() {
                    return Err(diag(
                        "R0233",
                        format!(
                            "enum variant `{enum_name}::{variant}` expects {} field value(s), got {}",
                            variant_definition.fields.len(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let mut lowered = Vec::new();
                for (index, argument) in arguments.into_iter().enumerate() {
                    let field_ty = variant_definition.fields[index];
                    let (value, actual) = self.expression(argument, Some(field_ty))?;
                    if actual != field_ty {
                        return Err(diag(
                            "R0205",
                            format!(
                                "variant `{enum_name}::{variant}` field {index} requires `{}` but the value has type `{}`",
                                type_name(field_ty),
                                type_name(actual)
                            ),
                            span,
                        ));
                    }
                    lowered.push(value);
                }
                Ok((
                    IrExpression::Call {
                        target: IrCallTarget::EnumNew {
                            enum_id,
                            tag: variant_index,
                        },
                        arguments: lowered,
                        return_type: Type::Enum(enum_id),
                    },
                    Type::Enum(enum_id),
                ))
            }
            Expression::Propagate(value, span) => {
                if self.defer_nesting > 0 {
                    return Err(diag("R0268", "`?` cannot leave a `defer` block", span)
                        .with_help("handle the error inside the `defer` block"));
                }
                // Early returns from `?` do not run deferred bodies, so they are rejected while any
                // `defer` is pending in this function.
                if self.tail_defers_pending || self.defer_frames.iter().any(|frame| !frame.is_empty()) {
                    return Err(diag(
                        "R0266",
                        "`?` cannot be used in a function with a `defer` block yet",
                        span,
                    )
                    .with_help("move the `?` before the `defer` block, or handle the error with `choose`"));
                }
                let (value, input_type) = self.expression(*value, None)?;
                let Type::Enum(input_enum) = input_type else {
                    return Err(diag(
                        "R0234",
                        "the `?` operator requires an `Option<T>` or `Result<T, E>` value",
                        span,
                    ));
                };
                let input_name = &self.enums[input_enum].name;
                let is_option = input_name.starts_with("$RynOption#");
                let is_result = input_name.starts_with("$RynResult#");
                if !is_option && !is_result {
                    return Err(diag(
                        "R0234",
                        "the `?` operator requires an `Option<T>` or `Result<T, E>` value",
                        span,
                    ));
                }
                let Some(Type::Enum(output_enum)) = self.return_type else {
                    return Err(diag(
                        "R0234",
                        "`?` can only be used in a function returning the matching `Option` or `Result` type",
                        span,
                    ));
                };
                if (is_option && !self.enums[output_enum].name.starts_with("$RynOption#"))
                    || (is_result && !self.enums[output_enum].name.starts_with("$RynResult#"))
                {
                    return Err(diag(
                        "R0234",
                        "the enclosing function must return the same generic type used with `?`",
                        span,
                    ));
                }
                let input = &self.enums[input_enum];
                let output = &self.enums[output_enum];
                let (success_name, failure_name) = if is_option {
                    ("Some", "None")
                } else {
                    ("Ok", "Err")
                };
                let Some(input_success) = input
                    .variants
                    .iter()
                    .find(|variant| variant.name == success_name)
                else {
                    return Err(diag(
                        "R0234",
                        "malformed generic specialization: missing success variant",
                        span,
                    ));
                };
                let Some(input_failure) = input
                    .variants
                    .iter()
                    .find(|variant| variant.name == failure_name)
                else {
                    return Err(diag(
                        "R0234",
                        "malformed generic specialization: missing failure variant",
                        span,
                    ));
                };
                let Some(output_failure) = output
                    .variants
                    .iter()
                    .find(|variant| variant.name == failure_name)
                else {
                    return Err(diag(
                        "R0234",
                        "malformed generic return type: missing failure variant",
                        span,
                    ));
                };
                if input_success.fields.len() != 1 {
                    return Err(diag("R0234", "malformed generic success variant", span));
                }
                let failure_type = if is_option {
                    if !input_failure.fields.is_empty() || !output_failure.fields.is_empty() {
                        return Err(diag("R0234", "malformed Option specialization", span));
                    }
                    None
                } else {
                    if input_failure.fields.len() != 1 || output_failure.fields.len() != 1 {
                        return Err(diag("R0234", "malformed Result specialization", span));
                    }
                    let error_type = input_failure.fields[0];
                    if output_failure.fields[0] != error_type {
                        return Err(diag(
                            "R0205",
                            "the `?` error type must match the enclosing function's `Result` error type",
                            span,
                        ));
                    }
                    Some(error_type)
                };
                let success_type = input_success.fields[0];
                let success_variant = input
                    .variants
                    .iter()
                    .position(|variant| variant.name == success_name)
                    .unwrap();
                let failure_variant = input
                    .variants
                    .iter()
                    .position(|variant| variant.name == failure_name)
                    .unwrap();
                let output_failure_variant = output
                    .variants
                    .iter()
                    .position(|variant| variant.name == failure_name)
                    .unwrap();
                Ok((
                    IrExpression::Propagate {
                        value: Box::new(value),
                        input_enum,
                        output_enum,
                        success_type,
                        failure_type,
                        success_variant,
                        failure_variant,
                        output_failure_variant,
                    },
                    success_type,
                ))
            }
            Expression::Choose { value, arms, span } => {
                if arms
                    .iter()
                    .position(|arm| arm.variant.is_none())
                    .is_some_and(|index| index + 1 != arms.len())
                {
                    return Err(diag("R0235", "the `_` arm in `choose` must be last", span));
                }
                let (scrutinee, scrut_ty) = self.expression(*value, None)?;
                let Type::Enum(enum_id) = scrut_ty else {
                    return Err(diag("R0234", "`choose` requires an enum value", span)
                        .with_help("match one of this enum's variants over a value of enum type"));
                };
                let definition = &self.enums[enum_id];
                let mut covered = vec![false; definition.variants.len()];
                let mut has_wildcard = false;
                for arm in &arms {
                    match (arm.enum_name.as_deref(), arm.variant.as_deref()) {
                        (None, None) => {
                            if has_wildcard {
                                return Err(diag(
                                    "R0235",
                                    "`choose` has more than one `_` arm",
                                    arm.span,
                                ));
                            }
                            has_wildcard = true;
                        }
                        (Some(name), Some(variant_name)) => {
                            if (*name != definition.name
                                && self.scoped_enum_id(name) != Some(enum_id))
                                && !is_generic_enum_name(&definition.name, name)
                            {
                                return Err(diag(
                                    "R0233",
                                    format!(
                                        "pattern names enum `{name}` but the value has type enum `{}`",
                                        definition.name
                                    ),
                                    arm.span,
                                ));
                            }
                            let Some(index) = definition
                                .variants
                                .iter()
                                .position(|candidate| candidate.name == variant_name)
                            else {
                                return Err(diag(
                                    "R0233",
                                    format!("enum `{name}` has no variant `{variant_name}`"),
                                    arm.span,
                                ));
                            };
                            if covered[index] {
                                return Err(diag(
                                    "R0235",
                                    format!(
                                        "`choose` handles variant `{name}::{variant_name}` more than once"
                                    ),
                                    arm.span,
                                ));
                            }
                            covered[index] = true;
                            if arm.bindings.len() != definition.variants[index].fields.len() {
                                return Err(diag(
                                    "R0233",
                                    format!(
                                        "enum variant `{name}::{variant_name}` expects {} binding(s), got {}",
                                        definition.variants[index].fields.len(),
                                        arm.bindings.len()
                                    ),
                                    arm.span,
                                ));
                            }
                        }
                        _ => {
                            return Err(diag(
                                "R0233",
                                "`choose` pattern must be any specific variant or `_`",
                                arm.span,
                            ));
                        }
                    }
                }
                if !has_wildcard {
                    let missing = definition
                        .variants
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| !covered[*index])
                        .map(|(_, variant)| variant.name.as_str())
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        return Err(diag(
                            "R0235",
                            format!(
                                "`choose` is not exhaustive; missing variant(s): {}",
                                missing.join(", ")
                            ),
                            span,
                        )
                        .with_help("add the missing variant arm(s) or a `_` fallback"));
                    }
                }
                let mut common: Option<Type> = None;
                let mut arms_ir = Vec::new();
                for arm in arms {
                    let saved_names = self.names.clone();
                    let mut bindings_ir = Vec::new();
                    if let (Some(_name), Some(variant_name)) =
                        (arm.enum_name.as_deref(), arm.variant.as_deref())
                    {
                        let index = definition
                            .variants
                            .iter()
                            .position(|candidate| candidate.name == variant_name)
                            .unwrap();
                        for (binding_index, binding_name) in arm.bindings.iter().enumerate() {
                            let field_ty = definition.variants[index].fields[binding_index];
                            let slot = self.allocate(field_ty);
                            self.names.insert(
                                binding_name.clone(),
                                Binding {
                                    slot,
                                    ty: field_ty,
                                    mutable: false,
                                },
                            );
                            bindings_ir.push(RynArmBinding {
                                slot,
                                ty: field_ty,
                                field_index: binding_index,
                            });
                        }
                    }
                    let arm_result = self.expression(arm.body, common);
                    let (body_ir, body_ty) = arm_result?;
                    match common {
                        Some(expected) if expected != body_ty => {
                            return Err(diag(
                                "R0205",
                                format!(
                                    "`choose` arm has type `{}` but earlier arms have type `{}`",
                                    type_name(body_ty),
                                    type_name(expected)
                                ),
                                arm.span,
                            ));
                        }
                        None => common = Some(body_ty),
                        _ => {}
                    }
                    let variant_index = match (arm.enum_name.as_deref(), arm.variant.as_deref()) {
                        (Some(_), Some(variant_name)) => Some(
                            definition
                                .variants
                                .iter()
                                .position(|candidate| candidate.name == variant_name)
                                .unwrap(),
                        ),
                        _ => None,
                    };
                    self.names = saved_names;
                    // Pattern bindings need distinct native slots for every arm.
                    // Keep the allocated slots in the function frame; only their
                    // lexical names leave scope here.
                    arms_ir.push(RynMatchArm {
                        variant: variant_index,
                        bindings: bindings_ir,
                        body: body_ir,
                    });
                }
                let common = common
                    .ok_or_else(|| diag("R0235", "`choose` must have at least one arm", span))?;
                Ok((
                    IrExpression::EnumMatch {
                        value: Box::new(scrutinee),
                        enum_id,
                        arms: arms_ir,
                        ty: common,
                    },
                    common,
                ))
            }
            Expression::MethodCall {
                value,
                name,
                type_arguments,
                arguments,
                span,
            } => {
                let (target, arguments, result) =
                    self.lower_method(*value, name.clone(), type_arguments, arguments, span)?;
                if matches!(
                    target,
                    IrCallTarget::String(StringOp::FindStr | StringOp::FindString)
                ) {
                    let raw = IrExpression::Call {
                        target,
                        arguments,
                        return_type: Type::I64,
                    };
                    let enum_id = self.enums.iter().position(is_option_u64).ok_or_else(|| {
                        diag("R0900", "internal Option<u64> type is missing", span)
                    })?;
                    let ty = Type::Enum(enum_id);
                    return Ok((
                        IrExpression::StringFindOption {
                            value: Box::new(raw),
                            enum_id,
                        },
                        ty,
                    ));
                }
                let return_type = result.ok_or_else(|| {
                    diag(
                        "R0215",
                        format!("method `{name}` does not return a value"),
                        span,
                    )
                    .with_help("call this method as a statement")
                })?;
                Ok((
                    IrExpression::Call {
                        target,
                        arguments,
                        return_type,
                    },
                    return_type,
                ))
            }
            Expression::If {
                condition,
                then_value,
                else_value,
                span,
            } => {
                let condition_span = condition.span();
                let condition_span = if condition_span == Span::default() {
                    span
                } else {
                    condition_span
                };
                let (condition, condition_ty) = match self.expression(*condition, Some(Type::Bool))
                {
                    Ok(result) => result,
                    Err(error) if self.recover_block_errors => {
                        if let Err(diagnostic) = self.expression(*then_value, expected) {
                            self.recovery_diagnostics.push(diagnostic);
                        }
                        if let Err(diagnostic) = self.expression(*else_value, expected) {
                            self.recovery_diagnostics.push(diagnostic);
                        }
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                if condition_ty != Type::Bool {
                    let error = diag(
                        "R0207",
                        "`when` expression condition must have type `bool`",
                        condition_span,
                    )
                    .with_help("use a boolean expression, such as a comparison, for the condition");
                    if self.recover_block_errors {
                        if let Err(diagnostic) = self.expression(*then_value, expected) {
                            self.recovery_diagnostics.push(diagnostic);
                        }
                        if let Err(diagnostic) = self.expression(*else_value, expected) {
                            self.recovery_diagnostics.push(diagnostic);
                        }
                    }
                    return Err(error);
                }
                let (then_value, then_ty) = match self.expression(*then_value, expected) {
                    Ok(result) => result,
                    Err(error) if self.recover_block_errors => {
                        if let Err(diagnostic) = self.expression(*else_value, expected) {
                            self.recovery_diagnostics.push(diagnostic);
                        }
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                let else_span = else_value.span();
                let (else_value, else_ty) = self.expression(*else_value, Some(then_ty))?;
                if else_ty != then_ty {
                    return Err(diag(
                        "R0205",
                        format!(
                            "when-expression branches must have the same type, found `{}` and `{}`",
                            type_name(then_ty),
                            type_name(else_ty)
                        ),
                        else_span,
                    )
                    .with_help(format!(
                        "make both branch expressions have type `{}`",
                        type_name(then_ty)
                    )));
                }
                Ok((
                    IrExpression::If {
                        condition: Box::new(condition),
                        then_value: Box::new(then_value),
                        else_value: Box::new(else_value),
                        ty: then_ty,
                    },
                    then_ty,
                ))
            }
            Expression::Negate(inner, span) => {
                if let Expression::Integer(value, literal_span) = *inner {
                    let ty = expected.filter(|ty| is_integer(*ty)).unwrap_or(Type::I64);
                    if !is_signed_integer(ty) {
                        return Err(diag(
                            "R0206",
                            "unary `-` requires a signed integer or floating-point value",
                            span,
                        )
                        .with_help(
                            "declare the value with a signed integer or floating-point type before negating it",
                        ));
                    }
                    let min = integer_bounds(ty).0;
                    if value as i128 == -min {
                        return Ok((IrExpression::Integer(min, ty), ty));
                    }
                    let (inner, inner_ty) =
                        self.expression(Expression::Integer(value, literal_span), Some(ty))?;
                    Ok((IrExpression::Negate(Box::new(inner), ty), inner_ty))
                } else {
                    let (inner, ty) = self.expression(*inner, expected)?;
                    if !is_signed_integer(ty) && !matches!(ty, Type::F32 | Type::F64) {
                        return Err(diag(
                            "R0206",
                            "unary `-` requires a signed integer or floating-point value",
                            span,
                        )
                        .with_help(
                            "declare the value with a signed integer or floating-point type before negating it",
                        ));
                    }
                    Ok((IrExpression::Negate(Box::new(inner), ty), ty))
                }
            }
            Expression::Not(inner, span) => {
                let (inner, ty) = self.expression(*inner, Some(Type::Bool))?;
                if ty != Type::Bool {
                    return Err(diag("R0206", "unary `!` requires a `bool` value", span)
                        .with_help("use `!` with a boolean value or comparison"));
                }
                Ok((IrExpression::Not(Box::new(inner)), Type::Bool))
            }
            Expression::BitNot(inner, span) => {
                let expected = expected
                    .filter(|ty| is_integer(*ty))
                    .or_else(|| self.integer_type_hint(&inner));
                let (inner, ty) = self.expression(*inner, expected)?;
                if !is_integer(ty) {
                    return Err(diag("R0206", "unary `~` requires an integer value", span)
                        .with_help("use `~` with a signed or unsigned integer"));
                }
                Ok((IrExpression::BitNot(Box::new(inner), ty), ty))
            }
            Expression::Cast(inner, target, span) => {
                let target = self.resolve_type_name(&target)?;
                if matches!(target, Type::FunctionPointer(_)) {
                    let inner_span = inner.span();
                    if let Expression::Name(name, name_span) = inner.as_ref()
                        && let Some(coerced) =
                            self.coerce_function_pointer(name, &target, *name_span)?
                    {
                        return Ok((coerced, target));
                    }
                    let (value, source) = self.expression(*inner, None)?;
                    if !matches!(source, Type::RawPointer(_) | Type::FunctionPointer(_)) {
                        return Err(diag(
                            "R0206",
                            format!(
                                "function pointer cast requires a raw pointer, found `{}`",
                                type_name(source)
                            ),
                            inner_span,
                        ));
                    }
                    return Ok((
                        IrExpression::Cast {
                            value: Box::new(value),
                            source,
                            target,
                        },
                        target,
                    ));
                }
                if matches!(target, Type::RawPointer(_)) {
                    let inner_span = inner.span();
                    let (value, source) = self.expression(*inner, None)?;
                    if matches!(source, Type::RawPointer(_) | Type::FunctionPointer(_))
                        || is_integer(source)
                    {
                        return Ok((
                            IrExpression::Cast {
                                value: Box::new(value),
                                source,
                                target,
                            },
                            target,
                        ));
                    }
                    return Err(diag(
                        "R0206",
                        format!(
                            "raw pointer cast requires an integer address, a raw pointer, or a function pointer, found `{}`",
                            type_name(source)
                        ),
                        inner_span,
                    )
                    .with_help(
                        "write `pointer as *u8` to reinterpret a raw pointer, or `32512 as *u8` for a Win32 resource id",
                    ));
                }
                if !is_numeric(target) {
                    return Err(diag(
                        "R0206",
                        format!(
                            "numeric cast requires a numeric target, found `{}`",
                            type_name(target)
                        ),
                        span,
                    )
                    .with_help("cast numeric values only to an integer or floating-point type"));
                }
                let inner_span = inner.span();
                let literal_context =
                    matches!(inner.as_ref(), Expression::Integer(..)) && is_integer(target);
                let (value, source) = self.expression(*inner, literal_context.then_some(target))?;
                if matches!(source, Type::RawPointer(_) | Type::FunctionPointer(_))
                    && matches!(target, Type::U64 | Type::I64)
                {
                    return Ok((
                        IrExpression::Cast {
                            value: Box::new(value),
                            source,
                            target,
                        },
                        target,
                    ));
                }
                // A `char` converts to its Unicode scalar value, like a `u32`.
                let char_to_integer = source == Type::Char && is_integer(target);
                if !is_numeric(source) && !char_to_integer {
                    return Err(diag(
                        "R0206",
                        format!(
                            "numeric cast requires a numeric value, found `{}`",
                            type_name(source)
                        ),
                        inner_span,
                    )
                    .with_help("cast an integer or floating-point value"));
                }
                Ok((
                    IrExpression::Cast {
                        value: Box::new(value),
                        source,
                        target,
                    },
                    target,
                ))
            }
            Expression::Binary {
                op,
                left,
                right,
                span,
            } => {
                let left_span = left.span();
                let right_span = right.span();
                let bitwise = matches!(
                    op,
                    BinaryOp::BitAnd
                        | BinaryOp::BitXor
                        | BinaryOp::BitOr
                        | BinaryOp::ShiftLeft
                        | BinaryOp::ShiftRight
                );
                let shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
                let context = expected
                    .filter(|ty| {
                        if bitwise {
                            is_integer(*ty)
                        } else {
                            is_numeric(*ty)
                        }
                    })
                    .or_else(|| {
                        if bitwise {
                            self.integer_type_hint(&left)
                        } else {
                            self.numeric_type_hint(&left)
                        }
                    })
                    .or_else(|| {
                        if shift {
                            return None;
                        }
                        if bitwise {
                            self.integer_type_hint(&right)
                        } else {
                            self.numeric_type_hint(&right)
                        }
                    });
                let (left, left_ty) = match self.expression(*left, context) {
                    Ok(value) => value,
                    Err(left_error) if self.recover_block_errors => {
                        if let Err(right_error) =
                            self.expression(*right, if shift { Some(Type::U32) } else { context })
                        {
                            self.recovery_diagnostics.push(right_error);
                        }
                        return Err(left_error);
                    }
                    Err(error) => return Err(error),
                };
                let operator_parameter_type = match left_ty {
                    Type::Struct(struct_id) => {
                        let method_name = match op {
                            BinaryOp::Add => "add",
                            BinaryOp::Sub => "sub",
                            BinaryOp::Mul => "mul",
                            BinaryOp::Div => "div",
                            BinaryOp::Rem => "rem",
                            _ => "",
                        };
                        if method_name.is_empty() {
                            None
                        } else {
                            self.operator_method_parameter_type(struct_id, method_name)
                        }
                    }
                    _ => None,
                };
                // Without a syntactic hint, the checked left operand types a
                // literal on the right: `self.byte(i) != 123` compares `u32`s.
                let left_context =
                    (is_numeric(left_ty) && (!bitwise || is_integer(left_ty))).then_some(left_ty);
                let (right, right_ty) = self.expression(
                    *right,
                    if shift {
                        Some(Type::U32)
                    } else {
                        operator_parameter_type.or(context).or(left_context)
                    },
                )?;
                if let Type::Struct(struct_id) = left_ty {
                    let method_name = match op {
                        BinaryOp::Add => "add",
                        BinaryOp::Sub => "sub",
                        BinaryOp::Mul => "mul",
                        BinaryOp::Div => "div",
                        BinaryOp::Rem => "rem",
                        _ => "",
                    };
                    if !method_name.is_empty()
                        && let Some(operator) = self.lower_operator_method(
                            left.clone(),
                            left_ty,
                            struct_id,
                            method_name,
                            right.clone(),
                            right_ty,
                            span,
                        )?
                    {
                        return Ok(operator);
                    }
                }
                // `String == str` compares the String's text through a borrowed view.
                let (left, left_ty, right, right_ty) = match (op, left_ty, right_ty) {
                    (BinaryOp::Eq | BinaryOp::Ne, Type::OwnedString, Type::Str) => (
                        IrExpression::StringAsStr(Box::new(left)),
                        Type::Str,
                        right,
                        right_ty,
                    ),
                    (BinaryOp::Eq | BinaryOp::Ne, Type::Str, Type::OwnedString) => (
                        left,
                        left_ty,
                        IrExpression::StringAsStr(Box::new(right)),
                        Type::Str,
                    ),
                    _ => (left, left_ty, right, right_ty),
                };
                let result_ty = match op {
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Rem => {
                        if op == BinaryOp::Rem && left_ty == right_ty && !is_integer(left_ty) {
                            return Err(diag(
                                "R0206",
                                format!(
                                    "`%` requires integer operands, found `{}` and `{}`",
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                span,
                            )
                            .with_help(
                                "use `%` only with integer operands; use `/` for floating-point division",
                            ));
                        }
                        if !is_numeric(left_ty) || left_ty != right_ty {
                            let bad_operand_span = if !is_numeric(left_ty) {
                                left_span
                            } else {
                                right_span
                            };
                            return Err(diag(
                                "R0206",
                                format!(
                                    "arithmetic operands must have matching numeric types, found `{}` and `{}`",
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                bad_operand_span,
                            )
                            .with_help(if !is_numeric(left_ty) || !is_numeric(right_ty) {
                                "use numeric operands for arithmetic operators"
                            } else {
                                "use matching numeric types; Ryn does not implicitly convert numeric values"
                            }));
                        }
                        left_ty
                    }
                    BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => {
                        if !is_integer(left_ty) || !is_integer(right_ty) || left_ty != right_ty {
                            let bad_operand_span = if !is_integer(left_ty) {
                                left_span
                            } else {
                                right_span
                            };
                            return Err(diag(
                                "R0206",
                                format!(
                                    "bitwise operands must have matching integer types, found `{}` and `{}`",
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                bad_operand_span,
                            )
                            .with_help(
                                "use integer operands of the same type with bitwise operators",
                            ));
                        }
                        left_ty
                    }
                    BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
                        if !is_integer(left_ty) || right_ty != Type::U32 {
                            let bad_operand_span = if !is_integer(left_ty) {
                                left_span
                            } else {
                                right_span
                            };
                            return Err(diag(
                                "R0206",
                                format!(
                                    "shift requires an integer value and a `u32` count, found `{}` and `{}`",
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                bad_operand_span,
                            )
                            .with_help(
                                "use an integer value on the left and a `u32` shift count on the right",
                            ));
                        }
                        left_ty
                    }
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge => {
                        let equality = matches!(op, BinaryOp::Eq | BinaryOp::Ne);
                        let valid = if equality {
                            left_ty == right_ty && is_equality_type(left_ty)
                        } else {
                            (is_numeric(left_ty) || left_ty == Type::Char) && left_ty == right_ty
                        };
                        if !valid {
                            let bad_operand_span = if equality {
                                if !is_equality_type(left_ty) {
                                    left_span
                                } else {
                                    right_span
                                }
                            } else if !is_numeric(left_ty) && left_ty != Type::Char {
                                left_span
                            } else {
                                right_span
                            };
                            return Err(diag(
                                "R0206",
                                format!(
                                    "{} operands are incompatible: found `{}` and `{}`",
                                    if equality { "equality" } else { "ordering" },
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                bad_operand_span,
                            )
                            .with_help(if equality {
                                "compare values with the same type"
                            } else {
                                "use numeric values with ordering operators such as `<` and `>=`"
                            }));
                        }
                        Type::Bool
                    }
                    BinaryOp::And | BinaryOp::Or => {
                        if left_ty != Type::Bool || right_ty != Type::Bool {
                            let bad_operand_span = if left_ty != Type::Bool {
                                left_span
                            } else {
                                right_span
                            };
                            return Err(diag(
                                "R0206",
                                format!(
                                    "logical operators require `bool` operands, found `{}` and `{}`",
                                    type_name(left_ty),
                                    type_name(right_ty)
                                ),
                                bad_operand_span,
                            )
                            .with_help(
                                "use boolean values or comparisons with `&&` and `||`",
                            ));
                        }
                        Type::Bool
                    }
                };
                Ok((
                    IrExpression::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                        ty: if matches!(
                            op,
                            BinaryOp::Eq
                                | BinaryOp::Ne
                                | BinaryOp::Lt
                                | BinaryOp::Le
                                | BinaryOp::Gt
                                | BinaryOp::Ge
                        ) {
                            left_ty
                        } else {
                            result_ty
                        },
                    },
                    result_ty,
                ))
            }
        }
    }

    fn struct_literal_recovering(
        &mut self,
        name: String,
        fields: Vec<(String, Expression, Span)>,
        span: Span,
        expected: Option<Type>,
    ) -> Result<(IrExpression, Type), Diagnostic> {
        let Some(struct_id) = self.scoped_struct_id(&name) else {
            let mut diagnostics = vec![
                diag("R0227", format!("unknown structure `{name}`"), span)
                    .with_help("declare the structure before constructing it"),
            ];
            for (_, value, _) in fields {
                if let Err(diagnostic) = self.expression(value, None) {
                    diagnostics.push(diagnostic);
                }
            }
            return Err(self.record_struct_literal_errors(diagnostics));
        };

        let ty = Type::Struct(struct_id);
        let definition = self.structs[struct_id].clone();
        let mut diagnostics = Vec::new();
        if expected.is_some_and(|expected| expected != ty) {
            diagnostics.push(diag(
                "R0205",
                format!(
                    "structure literal `{name}` does not match the expected type `{}`",
                    expected
                        .map(type_name)
                        .unwrap_or_else(|| "unknown".to_owned())
                ),
                span,
            ));
        }

        let mut initialized = vec![false; definition.fields.len()];
        let mut values = Vec::with_capacity(definition.fields.len());
        for (field_name, value, field_span) in fields {
            let Some((field_index, field_info)) = definition
                .fields
                .iter()
                .enumerate()
                .find(|(_, field)| field.name == field_name)
            else {
                diagnostics.push(self.unknown_struct_field(struct_id, &field_name, field_span));
                if let Err(diagnostic) = self.expression(value, None) {
                    diagnostics.push(diagnostic);
                }
                continue;
            };
            let field_type = field_info.ty;
            let duplicate = initialized[field_index];
            if duplicate {
                diagnostics.push(
                    diag(
                        "R0228",
                        format!("field `{field_name}` is initialized more than once"),
                        field_span,
                    )
                    .with_help("remove the duplicate field initializer"),
                );
            } else {
                initialized[field_index] = true;
            }

            let value_span = value.span();
            match self.expression(value, Some(field_type)) {
                Ok((value, actual)) if actual == field_type => {
                    if !duplicate {
                        values.push((field_index, value));
                    }
                }
                Ok((_, actual)) => diagnostics.push(
                    diag(
                        "R0205",
                        format!(
                            "field `{field_name}` requires `{}` but the value has type `{}`",
                            type_name(field_type),
                            type_name(actual)
                        ),
                        value_span,
                    )
                    .with_help(format!(
                        "provide a `{}` value for `{field_name}`",
                        type_name(field_type)
                    )),
                ),
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }

        for (index, field_info) in definition.fields.iter().enumerate() {
            if !initialized[index] {
                diagnostics.push(
                    diag(
                        "R0229",
                        format!("missing field `{}` in `{name}` literal", field_info.name),
                        span,
                    )
                    .with_help(format!("provide a value for `{}`", field_info.name)),
                );
            }
        }

        if !diagnostics.is_empty() {
            return Err(self.record_struct_literal_errors(diagnostics));
        }
        Ok((
            IrExpression::StructValue {
                struct_id,
                fields: values,
            },
            ty,
        ))
    }

    fn record_struct_literal_errors(&mut self, mut diagnostics: Vec<Diagnostic>) -> Diagnostic {
        diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
        let primary = diagnostics.remove(0);
        self.recovery_diagnostics.extend(diagnostics);
        primary
    }

    fn numeric_type_hint(&self, expression: &Expression) -> Option<Type> {
        match expression {
            Expression::MethodCall { .. } => self
                .value_type_hint(expression)
                .filter(|ty| is_numeric(*ty)),
            Expression::Name(name, _) => self
                .names
                .get(name)
                .map(|binding| binding.ty)
                .filter(|ty| is_numeric(*ty)),
            Expression::Call { name, .. } => {
                if name == "arg_count" {
                    return Some(Type::U32);
                }
                self.signatures
                    .get(name)
                    .and_then(|signature| signature.return_type)
                    .filter(|ty| is_numeric(*ty))
            }
            Expression::Field { value, name, .. } => {
                let receiver_hint = self.value_type_hint(value)?;
                let struct_id = match receiver_hint {
                    Type::Struct(id) => id,
                    Type::Reference(target, _) => match pointer_target(target) {
                        Type::Struct(id) => id,
                        _ => return None,
                    },
                    _ => return None,
                };
                self.structs[struct_id]
                    .fields
                    .iter()
                    .find(|field| field.name == *name)
                    .map(|field| field.ty)
                    .filter(|ty| is_numeric(*ty))
            }
            Expression::Negate(inner, _) | Expression::BitNot(inner, _) => {
                self.numeric_type_hint(inner)
            }
            Expression::Cast(_, target, _) => self
                .resolve_type_name(target)
                .ok()
                .filter(|ty| is_numeric(*ty)),
            Expression::Binary {
                op:
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Rem
                    | BinaryOp::BitAnd
                    | BinaryOp::BitXor
                    | BinaryOp::BitOr,
                left,
                right,
                ..
            } => self
                .numeric_type_hint(left)
                .or_else(|| self.numeric_type_hint(right)),
            Expression::Binary {
                op: BinaryOp::ShiftLeft | BinaryOp::ShiftRight,
                left,
                ..
            } => self.numeric_type_hint(left),
            _ => None,
        }
    }

    fn integer_type_hint(&self, expression: &Expression) -> Option<Type> {
        self.numeric_type_hint(expression)
            .filter(|ty| is_integer(*ty))
    }

    fn value_type_hint(&self, expression: &Expression) -> Option<Type> {
        match expression {
            Expression::Character(_, _) => Some(Type::Char),
            Expression::String(_, _) => Some(Type::Str),
            Expression::Call { name, .. } if name == "String" => Some(Type::OwnedString),
            Expression::MethodCall { value, name, .. } => {
                let receiver_hint = match self.value_type_hint(value) {
                    Some(Type::Reference(target, _)) => Some(pointer_target(target)),
                    other => other,
                };
                match receiver_hint {
                    Some(Type::OwnedString) => match name.as_str() {
                        "clone" | "to_string" | "concat" | "slice" | "slice_chars"
                        | "substring" | "trim" | "trim_start" | "trim_end" | "to_lower"
                        | "to_upper" | "replace" | "repeat" | "reverse" => Some(Type::OwnedString),
                        "lines" => Some(Type::Vec(intern_vec_elem(Type::OwnedString))),
                        "chars" => Some(Type::Vec(intern_vec_elem(Type::Char))),
                        "bytes" => Some(Type::Vec(intern_vec_elem(Type::U8))),
                        "is_empty" => Some(Type::Bool),
                        "to_i8" => Some(Type::I8),
                        "to_i16" => Some(Type::I16),
                        "to_i32" => Some(Type::I32),
                        "to_i64" => Some(Type::I64),
                        "to_u8" => Some(Type::U8),
                        "to_u16" => Some(Type::U16),
                        "to_u32" => Some(Type::U32),
                        "to_u64" => Some(Type::U64),
                        "to_f32" => Some(Type::F32),
                        "to_f64" => Some(Type::F64),
                        "len" | "char_count" => Some(Type::U64),
                        "find" => Some(Type::I64),
                        "split" => Some(Type::Vec(intern_vec_elem(Type::OwnedString))),
                        "char_at" => Some(Type::Char),
                        "byte_at" => Some(Type::U8),
                        "starts_with" | "ends_with" | "contains" => Some(Type::Bool),
                        _ => None,
                    },
                    Some(Type::Vec(id)) => match name.as_str() {
                        "clone" => Some(Type::Vec(id)),
                        "len" | "capacity" => Some(Type::U64),
                        "is_empty" => Some(Type::Bool),
                        "take" | "extract" | "index" => Some(vec_elem(id)),
                        _ => None,
                    },
                    Some(ty) if is_numeric(ty) && name == "to_string" => Some(Type::OwnedString),
                    _ => None,
                }
            }
            Expression::Name(name, _) => self.names.get(name).map(|binding| binding.ty),
            Expression::StructLiteral { name, .. } => self.scoped_struct_id(name).map(Type::Struct),
            Expression::Call { name, .. } => self
                .signatures
                .get(name)
                .and_then(|signature| signature.return_type),
            Expression::Cast(_, target, _) => self.resolve_type_name(target).ok(),
            Expression::Field { value, name, .. } => {
                let receiver_ty = self.value_type_hint(value)?;
                let struct_id = match receiver_ty {
                    Type::Struct(id) => id,
                    Type::Reference(target, _) => match pointer_target(target) {
                        Type::Struct(id) => id,
                        _ => return None,
                    },
                    _ => return None,
                };
                self.structs[struct_id]
                    .fields
                    .iter()
                    .find(|field| field.name == *name)
                    .map(|field| field.ty)
            }
            _ => None,
        }
    }

    fn unknown_variable(&self, name: &str, span: Span) -> Diagnostic {
        let diagnostic = diag("R0203", format!("unknown variable `{name}`"), span);
        match closest_name(name, self.names.keys().map(String::as_str)) {
            Some(suggestion) => diagnostic.with_help(format!("did you mean `{suggestion}`?")),
            None => diagnostic,
        }
    }

    fn unknown_struct_field(&self, struct_id: usize, name: &str, span: Span) -> Diagnostic {
        let definition = &self.structs[struct_id];
        let diagnostic = diag(
            "R0225",
            format!("structure `{}` has no field `{name}`", definition.name),
            span,
        );
        match closest_name(
            name,
            definition.fields.iter().map(|field| field.name.as_str()),
        ) {
            Some(suggestion) => diagnostic.with_help(format!("did you mean `{suggestion}`?")),
            None => diagnostic.with_help("use a field declared by this structure"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn reference_field_assign(
        &mut self,
        pointer: IrExpression,
        pointee: Type,
        reference_mutable: bool,
        object: String,
        object_span: Span,
        fields: &[(String, Span)],
        op: Option<BinaryOp>,
        value: Expression,
        span: Span,
    ) -> Result<IrStatement, Diagnostic> {
        let Type::Struct(struct_id) = pointee else {
            return Err(diag(
                "R0224",
                "field assignment requires a structure value",
                span,
            ));
        };
        if !reference_mutable {
            return Err(
                diag("R0206", "cannot assign through a shared reference", span)
                    .with_help("the receiver must be borrowed with `mut self` to mutate fields"),
            );
        }
        let Some(((final_field, final_field_span), parent_fields)) = fields.split_last() else {
            return Err(diag(
                "R0900",
                "internal error: field assignment has no field path",
                span,
            ));
        };
        if !parent_fields.is_empty() {
            return Err(diag(
                "R0224",
                "assigning through nested field paths on a borrowed receiver is not supported yet",
                span,
            )
            .with_help("assign through a single field level of the borrowed value for now"));
        }
        let Some((field_index, field_info)) = self.structs[struct_id]
            .fields
            .iter()
            .enumerate()
            .find(|(_, candidate)| candidate.name == *final_field)
            .map(|(index, field)| (index, field.clone()))
        else {
            return Err(self.unknown_struct_field(struct_id, final_field, *final_field_span));
        };
        self.check_field_visibility(struct_id, field_index, *final_field_span)?;
        if field_index == 0 && self.structs[struct_id].drop_function.is_some() {
            return Err(diag(
                "R0255",
                "cannot overwrite the resource handle through a borrowed value",
                span,
            ));
        }
        let value_span = value.span();
        let (value, actual) = if let Some(op) = op {
            let mut left = Expression::Name(object, object_span);
            for (field, field_span) in fields {
                left = Expression::Field {
                    value: Box::new(left),
                    name: field.clone(),
                    name_span: *field_span,
                    span: Span {
                        start: object_span.start,
                        end: field_span.end,
                    },
                };
            }
            self.expression(
                Expression::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(value),
                    span: Span {
                        start: object_span.start,
                        end: value_span.end,
                    },
                },
                Some(field_info.ty),
            )?
        } else {
            self.expression(value, Some(field_info.ty))?
        };
        if actual != field_info.ty {
            return Err(diag(
                "R0205",
                format!(
                    "cannot assign `{}` to field `{final_field}` of type `{}`",
                    type_name(actual),
                    type_name(field_info.ty)
                ),
                value_span,
            )
            .with_help(format!(
                "assign a `{}` value to `{final_field}`",
                type_name(field_info.ty)
            )));
        }
        Ok(IrStatement::ReferenceFieldAssign {
            pointer,
            struct_id,
            field_index,
            ty: field_info.ty,
            value,
        })
    }

    fn scoped_zero_argument_function(&self, name: &str) -> Option<(IrExpression, Type)> {
        let resolved = if self.signatures.contains_key(name) {
            name.to_string()
        } else {
            let namespace_name = self
                .function_name
                .rsplit_once("::")
                .map(|(namespace, _)| format!("{namespace}::{name}"));
            namespace_name
                .filter(|candidate| self.signatures.contains_key(candidate))
                .or_else(|| {
                    let local_name = if self.module_path.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}::{name}", self.module_path)
                    };
                    self.signatures
                        .contains_key(&local_name)
                        .then_some(local_name)
                })?
        };
        let signature = self.signatures.get(&resolved)?;
        if !signature.parameters.is_empty() {
            return None;
        }
        let IrCallTarget::Function(_) = signature.target else {
            return None;
        };
        Some((
            IrExpression::Call {
                target: signature.target,
                arguments: Vec::new(),
                return_type: signature.return_type?,
            },
            signature.return_type?,
        ))
    }

    fn check_field_visibility(
        &self,
        struct_id: usize,
        field_index: usize,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let definition = &self.structs[struct_id];
        if definition.name.starts_with('$') || definition.module_path == self.module_path {
            return Ok(());
        }
        if definition.fields[field_index].public {
            return Ok(());
        }
        Err(diag(
            "R0426",
            format!(
                "field `{}` of `{}` is private",
                definition.fields[field_index].name, definition.name
            ),
            span,
        )
        .with_help("mark the field `pub` to read it outside its module"))
    }

    fn lower_method(
        &mut self,
        value: Expression,
        name: String,
        type_arguments: Vec<TypeName>,
        arguments: Vec<Expression>,
        span: Span,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        fn root(value: &Expression) -> Option<&str> {
            match value {
                Expression::Name(name, _) => Some(name),
                Expression::Field { value, .. } => root(value),
                _ => None,
            }
        }
        let mutable = root(&value)
            .and_then(|name| self.names.get(name))
            .is_none_or(|binding| binding.mutable || self.parameter_slots.contains(&binding.slot));
        let (mut receiver, receiver_ty) = self.expression(value, None)?;
        borrow_reference_receiver(&mut receiver, receiver_ty);
        if matches!(name.as_str(), "parse" | "try_parse") {
            if type_arguments.len() != 1 {
                return Err(diag(
                    "R0211",
                    format!("method `{name}` expects one type argument"),
                    span,
                ));
            }
            if !arguments.is_empty() {
                return Err(diag(
                    "R0211",
                    format!("method `{name}` expects no value arguments"),
                    span,
                ));
            }
            if receiver_ty != Type::OwnedString {
                return Err(diag(
                    "R0234",
                    format!("`{name}` is only available on String"),
                    span,
                ));
            }
            let target_name = match (&type_arguments[0], name.as_str()) {
                (TypeName::I8, "parse") => "to_i8",
                (TypeName::I16, "parse") => "to_i16",
                (TypeName::I32, "parse") => "to_i32",
                (TypeName::I64, "parse") => "to_i64",
                (TypeName::U8, "parse") => "to_u8",
                (TypeName::U16, "parse") => "to_u16",
                (TypeName::U32, "parse") => "to_u32",
                (TypeName::U64, "parse") => "to_u64",
                (TypeName::F32, "parse") => "to_f32",
                (TypeName::F64, "parse") => "to_f64",
                (TypeName::I8, "try_parse") => "try_to_i8",
                (TypeName::I16, "try_parse") => "try_to_i16",
                (TypeName::I32, "try_parse") => "try_to_i32",
                (TypeName::I64, "try_parse") => "try_to_i64",
                (TypeName::U8, "try_parse") => "try_to_u8",
                (TypeName::U16, "try_parse") => "try_to_u16",
                (TypeName::U32, "try_parse") => "try_to_u32",
                (TypeName::U64, "try_parse") => "try_to_u64",
                (TypeName::F32, "try_parse") => "try_to_f32",
                (TypeName::F64, "try_parse") => "try_to_f64",
                _ => {
                    return Err(diag(
                        "R0234",
                        "String parsing supports numeric target types",
                        span,
                    ));
                }
            };
            return self.lower_string_method_receiver(
                receiver,
                target_name.to_owned(),
                Vec::new(),
                span,
                mutable,
            );
        } else if !type_arguments.is_empty()
            && !(name == "unwrap_or"
                && type_arguments.len() == 1
                && matches!(receiver_ty, Type::Enum(_)))
        {
            return Err(diag(
                "R0234",
                "this method does not accept type arguments",
                span,
            ));
        }
        if name == "to_string" && arguments.is_empty() {
            if receiver_ty == Type::OwnedString {
                return Ok((
                    IrCallTarget::String(StringOp::Clone),
                    vec![receiver],
                    Some(Type::OwnedString),
                ));
            }
            if let Some(operation) = StringOp::from_numeric_type(receiver_ty) {
                return Ok((
                    IrCallTarget::String(operation),
                    vec![receiver],
                    Some(Type::OwnedString),
                ));
            }
        }
        if let Type::Vec(elem_id) = receiver_ty {
            if matches!(name.as_str(), "as_slice" | "slice") {
                if custom_drop_vec_only(Type::Vec(elem_id), self.structs, self.enums) {
                    return Err(diag(
                        "R0234",
                        "slices of custom-destructor values are not supported because slice reads clone elements",
                        span,
                    ));
                }
                let (start, end) = if name == "slice" {
                    if arguments.len() != 2 {
                        return Err(diag(
                            "R0211",
                            format!(
                                "method `slice` expects 2 arguments but got {}",
                                arguments.len()
                            ),
                            span,
                        ));
                    }
                    let mut arguments = arguments.into_iter();
                    let start_expr = arguments.next().expect("slice arity checked");
                    let end_expr = arguments.next().expect("slice arity checked");
                    let (start, start_ty) = self.expression(start_expr, Some(Type::U64))?;
                    let (end, end_ty) = self.expression(end_expr, Some(Type::U64))?;
                    if start_ty != Type::U64 || end_ty != Type::U64 {
                        return Err(diag("R0205", "Vec slice bounds must be u64", span));
                    }
                    (start, end)
                } else {
                    if !arguments.is_empty() {
                        return Err(diag(
                            "R0211",
                            format!(
                                "method `as_slice` expects no arguments but got {}",
                                arguments.len()
                            ),
                            span,
                        ));
                    }
                    (
                        IrExpression::Integer(0, Type::U64),
                        IrExpression::Call {
                            target: IrCallTarget::Vec(VecOp::Len, elem_id),
                            arguments: vec![receiver.clone()],
                            return_type: Type::U64,
                        },
                    )
                };
                return Ok((
                    IrCallTarget::VecSlice(elem_id),
                    vec![receiver, start, end],
                    Some(Type::Slice(elem_id)),
                ));
            }
            return self.lower_vec_method(
                receiver,
                elem_id,
                name.clone(),
                arguments,
                span,
                mutable,
            );
        }
        if let Type::Slice(_) = receiver_ty {
            if name == "len" && arguments.is_empty() {
                return Ok((IrCallTarget::SliceLen, vec![receiver], Some(Type::U64)));
            }
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        }
        if let Type::Set(map_id) = receiver_ty {
            return self.lower_set_method(receiver, map_id, name.clone(), arguments, span, mutable);
        }
        if let Type::Map(map_id) = receiver_ty {
            return self.lower_map_method(receiver, map_id, name.clone(), arguments, span, mutable);
        }
        if let Type::Enum(enum_id) = receiver_ty {
            let definition = &self.enums[enum_id];
            let is_option = definition
                .variants
                .iter()
                .any(|variant| variant.name == "Some")
                && definition
                    .variants
                    .iter()
                    .any(|variant| variant.name == "None");
            let is_result = definition
                .variants
                .iter()
                .any(|variant| variant.name == "Ok")
                && definition
                    .variants
                    .iter()
                    .any(|variant| variant.name == "Err");
            let predicate = match (is_option, is_result, name.as_str()) {
                (true, _, "is_some") => Some(EnumPredicate::IsSome),
                (true, _, "is_none") => Some(EnumPredicate::IsNone),
                (_, true, "is_ok") => Some(EnumPredicate::IsOk),
                (_, true, "is_err") => Some(EnumPredicate::IsErr),
                _ => None,
            };
            if let Some(predicate) = predicate {
                if !arguments.is_empty() {
                    return Err(diag(
                        "R0211",
                        format!(
                            "method `{name}` expects no arguments but got {}",
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let variants = &definition.variants;
                let expected = match predicate {
                    EnumPredicate::IsSome => "Some",
                    EnumPredicate::IsOk => "Ok",
                    EnumPredicate::IsNone => "None",
                    EnumPredicate::IsErr => "Err",
                };
                if !variants.iter().any(|variant| variant.name == expected) {
                    return Err(diag(
                        "R0900",
                        "malformed Option/Result specialization",
                        span,
                    ));
                }
                return Ok((
                    IrCallTarget::EnumPredicate(predicate, enum_id),
                    vec![receiver],
                    Some(Type::Bool),
                ));
            }
            if (definition.name.starts_with("$RynOption#")
                || definition.name.starts_with("$RynResult#"))
                && name == "unwrap_or"
            {
                if let Some(type_argument) = type_arguments.first() {
                    let requested_type = self.resolve_type_name(type_argument)?;
                    let expected_type = definition.variants.iter().find_map(|variant| {
                        (variant.name
                            == if definition.name.starts_with("$RynResult#") {
                                "Ok"
                            } else {
                                "Some"
                            })
                        .then(|| variant.fields.first().copied())
                        .flatten()
                    });
                    if expected_type != Some(requested_type) {
                        return Err(diag(
                            "R0212",
                            format!(
                                "unwrap_or type argument `{}` does not match the payload type `{}`",
                                type_name(requested_type),
                                expected_type
                                    .map(type_name)
                                    .unwrap_or_else(|| "unknown".into())
                            ),
                            span,
                        ));
                    }
                }
                if arguments.len() != 1 {
                    return Err(diag(
                        "R0211",
                        format!(
                            "method `unwrap_or` expects 1 argument but got {}",
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let success_name = if definition.name.starts_with("$RynResult#") {
                    "Ok"
                } else {
                    "Some"
                };
                let Some(value_type) = definition.variants.iter().find_map(|variant| {
                    (variant.name == success_name)
                        .then(|| variant.fields.first().copied())
                        .flatten()
                }) else {
                    return Err(diag(
                        "R0900",
                        "malformed Option/Result specialization",
                        span,
                    ));
                };
                if !enum_extract_payload_supported(value_type, self.structs, self.enums) {
                    return Err(diag(
                        "R0234",
                        "Option/Result.unwrap_or supports scalar, owned pointer-backed, and non-resource aggregate payloads",
                        span,
                    ));
                }
                let argument_span = arguments[0].span();
                let (mut default, actual) =
                    self.expression(arguments.into_iter().next().unwrap(), Some(value_type))?;
                if actual != value_type {
                    return Err(diag(
                        "R0212",
                        format!(
                            "unwrap_or default has type `{}` but `{}` is required",
                            type_name(actual),
                            type_name(value_type)
                        ),
                        argument_span,
                    ));
                }
                if value_type == Type::OwnedString {
                    default = IrExpression::Call {
                        target: IrCallTarget::String(StringOp::Clone),
                        arguments: vec![default],
                        return_type: Type::OwnedString,
                    };
                }
                return Ok((
                    IrCallTarget::EnumUnwrapOr {
                        enum_id,
                        value_type,
                    },
                    vec![receiver, default],
                    Some(value_type),
                ));
            }
            if (is_option && matches!(name.as_str(), "unwrap" | "expect"))
                || (is_result && matches!(name.as_str(), "unwrap" | "expect" | "unwrap_err"))
            {
                let takes_message = name == "expect";
                let success_name = if name == "unwrap_err" {
                    "Err"
                } else if is_result {
                    "Ok"
                } else {
                    "Some"
                };
                let failure_op = if takes_message {
                    SystemOp::OptionExpectFailed
                } else if is_result {
                    SystemOp::ResultUnwrapFailed
                } else {
                    SystemOp::OptionUnwrapFailed
                };
                if arguments.len() != usize::from(takes_message) {
                    return Err(diag(
                        "R0211",
                        format!(
                            "method `{name}` expects {} argument(s) but got {}",
                            usize::from(takes_message),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let Some((success_tag, value_type)) = definition
                    .variants
                    .iter()
                    .enumerate()
                    .find_map(|(tag, variant)| {
                        (variant.name == success_name)
                            .then(|| variant.fields.first().copied().map(|ty| (tag, ty)))
                            .flatten()
                    })
                else {
                    return Err(diag("R0900", "malformed Option specialization", span));
                };
                if !enum_extract_payload_supported(value_type, self.structs, self.enums) {
                    return Err(diag(
                        "R0234",
                        "Option/Result extraction supports scalar, owned pointer-backed, and non-resource aggregate payloads",
                        span,
                    ));
                }
                let mut lowered = vec![receiver];
                if takes_message {
                    let message_span = arguments[0].span();
                    let (message, actual) =
                        self.expression(arguments.into_iter().next().unwrap(), Some(Type::Str))?;
                    if actual != Type::Str {
                        return Err(diag(
                            "R0212",
                            "Option.expect message must be `str`",
                            message_span,
                        ));
                    }
                    lowered.push(message);
                }
                return Ok((
                    IrCallTarget::EnumUnwrap {
                        enum_id,
                        value_type,
                        message: takes_message,
                        success_tag,
                        failure: failure_op,
                    },
                    lowered,
                    Some(value_type),
                ));
            }
        }
        if let Type::Struct(struct_id) = receiver_ty {
            return self.lower_struct_method(
                receiver,
                receiver_ty,
                struct_id,
                name,
                arguments,
                span,
                mutable,
            );
        }
        if let Type::Reference(target, reference_mutable) = receiver_ty {
            let pointee = pointer_target(target);
            if let Type::Struct(struct_id) = pointee {
                return self.lower_struct_method_borrowed(
                    receiver,
                    receiver_ty,
                    reference_mutable,
                    struct_id,
                    name,
                    arguments,
                    span,
                );
            }
            if pointee == Type::OwnedString {
                // A built-in receiver borrowed through `extend String { ... }`:
                // load the handle from the reference's stack home as a borrowed
                // view; the reference's home keeps ownership.
                let dereferenced = IrExpression::Dereference {
                    pointer: Box::new(receiver),
                    ty: pointee,
                };
                receiver = dereferenced;
                return self.lower_string_method_receiver(
                    receiver,
                    name,
                    arguments,
                    span,
                    reference_mutable,
                );
            }
            return Err(diag(
                "R0234",
                format!("`extend` for `{}` is not supported yet", type_name(pointee)),
                span,
            ));
        }
        if receiver_ty != Type::OwnedString {
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        }
        self.lower_string_method_receiver(receiver, name, arguments, span, mutable)
    }

    fn lower_string_method_receiver(
        &mut self,
        receiver: IrExpression,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let owned_argument = arguments
            .first()
            .and_then(|argument| self.value_type_hint(argument))
            == Some(Type::OwnedString);
        let try_parse = match name.as_str() {
            "try_to_i8" => Some((Type::I8, 0)),
            "try_to_i16" => Some((Type::I16, 1)),
            "try_to_i32" => Some((Type::I32, 2)),
            "try_to_i64" => Some((Type::I64, 3)),
            "try_to_u8" => Some((Type::U8, 4)),
            "try_to_u16" => Some((Type::U16, 5)),
            "try_to_u32" => Some((Type::U32, 6)),
            "try_to_u64" => Some((Type::U64, 7)),
            "try_to_f32" => Some((Type::F32, 8)),
            "try_to_f64" => Some((Type::F64, 9)),
            _ => None,
        };
        let op = match name.as_str() {
            "try_to_i8" | "try_to_i16" | "try_to_i32" | "try_to_i64" | "try_to_u8"
            | "try_to_u16" | "try_to_u32" | "try_to_u64" | "try_to_f32" | "try_to_f64" => {
                StringOp::TryParse
            }
            "clone" => StringOp::Clone,
            "to_i8" => StringOp::ToI8,
            "to_i16" => StringOp::ToI16,
            "to_i32" => StringOp::ToI32,
            "to_i64" => StringOp::ToI64,
            "to_u8" => StringOp::ToU8,
            "to_u16" => StringOp::ToU16,
            "to_u32" => StringOp::ToU32,
            "to_u64" => StringOp::ToU64,
            "to_f32" => StringOp::ToF32,
            "to_f64" => StringOp::ToF64,
            "len" => StringOp::Len,
            "char_count" => StringOp::CharCount,
            "byte_at" => StringOp::ByteAt,
            "char_at" => StringOp::CharAt,
            "slice" => StringOp::Slice,
            "slice_chars" => StringOp::SliceChars,
            "substring" => StringOp::SliceChars,
            "trim" => StringOp::Trim,
            "trim_start" => StringOp::TrimStart,
            "trim_end" => StringOp::TrimEnd,
            "is_empty" => StringOp::IsEmpty,
            "to_lower" => StringOp::ToLower,
            "to_upper" => StringOp::ToUpper,
            "replace" => StringOp::ReplaceStr,
            "lines" => StringOp::Lines,
            "chars" => StringOp::Chars,
            "bytes" => StringOp::Bytes,
            "repeat" => StringOp::Repeat,
            "reverse" => StringOp::Reverse,
            "clear" => StringOp::Clear,
            "push" => StringOp::Push,
            "append" if owned_argument => StringOp::AppendString,
            "append" => StringOp::AppendStr,
            "concat" if owned_argument => StringOp::ConcatString,
            "concat" => StringOp::ConcatStr,
            "starts_with" if owned_argument => StringOp::StartsWithString,
            "starts_with" => StringOp::StartsWithStr,
            "ends_with" if owned_argument => StringOp::EndsWithString,
            "ends_with" => StringOp::EndsWithStr,
            "contains" if owned_argument => StringOp::ContainsString,
            "contains" => StringOp::ContainsStr,
            "find" if owned_argument => StringOp::FindString,
            "find" => StringOp::FindStr,
            "split" if owned_argument => StringOp::SplitString,
            "split" => StringOp::SplitStr,
            _ => {
                let extension = self.lookup_extension_method(
                    &receiver,
                    &name,
                    arguments,
                    Type::OwnedString,
                    span,
                    mutable,
                )?;
                if let Some(result) = extension {
                    return Ok(result);
                }
                return Err(diag(
                    "R0234",
                    format!("String has no method `{name}`"),
                    span,
                ));
            }
        };
        if op.mutates() && !mutable {
            return Err(diag(
                "R0204",
                format!("method `{name}` requires a mutable String"),
                span,
            )
            .with_help("declare the receiver's root binding with `mut`"));
        }
        let mut arguments = arguments;
        if let Some((_, tag)) = try_parse {
            arguments.push(Expression::Integer(tag, span));
        }
        let parameters = op.parameters().into_iter().skip(1).collect();
        let result_type = if let Some((parsed_type, _)) = try_parse {
            let option_id = self
                .enums
                .iter()
                .position(|definition| is_option_of(definition, parsed_type))
                .ok_or_else(|| {
                    diag(
                        "R0900",
                        "internal Option type for String parse is missing",
                        span,
                    )
                })?;
            Some(Type::Enum(option_id))
        } else {
            op.result()
        };
        let (target, mut arguments, result) = self.lower_builtin_call(
            name,
            IrCallTarget::String(op),
            parameters,
            result_type,
            arguments,
            span,
        )?;
        arguments.insert(0, receiver);
        Ok((target, arguments, result))
    }

    /// The right-operand type of the struct's operator method, if defined.
    fn operator_method_parameter_type(&self, struct_id: usize, method_name: &str) -> Option<Type> {
        let definition = self.structs.get(struct_id)?;
        let mut candidates = vec![format!("{}::{method_name}#method", definition.name)];
        if !definition.module_path.is_empty() {
            candidates.push(format!(
                "{}::{}::{method_name}#method",
                definition.module_path, definition.name
            ));
        }
        for candidate in candidates {
            let Some(signature) = self.signatures.get(&candidate) else {
                continue;
            };
            return signature.parameters.get(1).copied();
        }
        None
    }

    /// Lowers `left <op> right` through the struct's operator method.
    #[allow(clippy::too_many_arguments)]
    fn lower_operator_method(
        &mut self,
        left: IrExpression,
        left_ty: Type,
        struct_id: usize,
        method_name: &str,
        right: IrExpression,
        right_ty: Type,
        span: Span,
    ) -> Result<Option<(IrExpression, Type)>, Diagnostic> {
        let definition = self.structs[struct_id].clone();
        let struct_name = definition.name.clone();
        let mut candidates = vec![format!("{struct_name}::{method_name}#method")];
        if !definition.module_path.is_empty() {
            candidates.push(format!(
                "{}::{}::{method_name}#method",
                definition.module_path, struct_name
            ));
        }
        let mut signature = None;
        for candidate in candidates {
            if let Some(found) = self.signatures.get(&candidate) {
                signature = Some(found.clone());
                break;
            }
        }
        let Some(signature) = signature else {
            return Ok(None);
        };
        if signature
            .parameters
            .get(1)
            .is_none_or(|parameter| *parameter != right_ty)
        {
            return Ok(None);
        }
        // Operator methods copy the operands, so owning structs are rejected.
        if type_has_owned_data_in_sema(Type::Struct(struct_id), self.structs) {
            return Err(diag(
                "R0451",
                format!(
                    "operator `{method_name}` on `{struct_name}` requires a struct without owned fields"
                ),
                span,
            )
            .with_help(
                "operator methods copy their operands; keep owned fields out of operator structs for now",
            ));
        }
        let receiver = IrExpression::ValueAddress {
            value: Box::new(left),
            ty: left_ty,
            pointer_type: Type::Reference(intern_pointer_target(left_ty), false),
            span,
        };
        Ok(Some((
            IrExpression::Call {
                target: signature.target,
                arguments: vec![receiver, right],
                return_type: signature.return_type.unwrap_or(left_ty),
            },
            signature.return_type.unwrap_or(left_ty),
        )))
    }

    /// Lowers one chain link against the current receiver value.
    #[allow(clippy::too_many_arguments)]
    fn lower_method_for_chain(
        &mut self,
        receiver: IrExpression,
        receiver_ty: Type,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        match receiver_ty {
            Type::Struct(struct_id) => self.lower_struct_method(
                receiver,
                receiver_ty,
                struct_id,
                name,
                arguments,
                span,
                mutable,
            ),
            Type::Vec(elem_id) => {
                self.lower_vec_method(receiver, elem_id, name, arguments, span, mutable)
            }
            Type::Map(map_id) => {
                self.lower_map_method(receiver, map_id, name, arguments, span, mutable)
            }
            Type::Set(map_id) => {
                self.lower_set_method(receiver, map_id, name, arguments, span, mutable)
            }
            Type::Reference(target, reference_mutable) => {
                if let Type::Struct(struct_id) = pointer_target(target) {
                    return self.lower_struct_method_borrowed(
                        receiver,
                        receiver_ty,
                        reference_mutable,
                        struct_id,
                        name,
                        arguments,
                        span,
                    );
                }
                if pointer_target(target) == Type::OwnedString {
                    return self.lower_string_method_receiver(
                        receiver,
                        name,
                        arguments,
                        span,
                        reference_mutable,
                    );
                }
                Err(diag(
                    "R0234",
                    format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                    span,
                ))
            }
            Type::OwnedString => {
                self.lower_string_method_receiver(receiver, name, arguments, span, mutable)
            }
            other => Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(other)),
                span,
            )),
        }
    }

    /// Resolves an `extend`-defined method on a built-in receiver type.
    #[allow(clippy::too_many_arguments)]
    fn lookup_extension_method(
        &mut self,
        receiver: &IrExpression,
        name: &str,
        arguments: Vec<Expression>,
        receiver_ty: Type,
        span: Span,
        receiver_mutable: bool,
    ) -> LoweredMethod {
        let display_name = match receiver_ty {
            Type::OwnedString => "String".to_string(),
            other => type_name(other),
        };
        let mut candidates = vec![format!("{display_name}::{name}")];
        let suffix = format!("::{display_name}::{name}");
        for signature_name in self.signatures.keys() {
            if signature_name.ends_with(&suffix) {
                candidates.push(signature_name.clone());
            }
        }
        for candidate in candidates {
            let Some(signature) = self.signatures.get(&candidate).cloned() else {
                continue;
            };
            let Some(self_parameter) = signature.parameters.first().copied() else {
                continue;
            };
            let Type::Reference(target, self_mutable) = self_parameter else {
                continue;
            };
            if pointer_target(target) != receiver_ty {
                continue;
            }
            if self_mutable && !receiver_mutable {
                return Err(diag(
                    "R0204",
                    format!("method `{name}` requires a mutable receiver"),
                    span,
                )
                .with_help("declare the receiver's root binding with `mut`"));
            }
            let function_index = match signature.target {
                IrCallTarget::Function(index) => index,
                _ => continue,
            };
            if !self
                .function_visibility
                .get(function_index)
                .copied()
                .unwrap_or(false)
                && self
                    .function_module_paths
                    .get(function_index)
                    .map(String::as_str)
                    != Some(self.module_path.as_str())
            {
                return Err(diag("R0425", format!("method `{name}` is private"), span)
                    .with_help("mark the method `pub` inside its `extend` block"));
            }
            // Borrow the receiver local; a reference receiver passes through.
            let receiver_argument = match &receiver {
                IrExpression::Local {
                    slot,
                    ty,
                    span: local_span,
                } => {
                    if matches!(ty, Type::Reference(_, _)) {
                        (*receiver).clone()
                    } else {
                        self.addressed_slot_types[*slot] = Some(*ty);
                        IrExpression::AddressOf {
                            slot: *slot,
                            ty: *ty,
                            pointer_type: Type::Reference(
                                intern_pointer_target(receiver_ty),
                                self_mutable,
                            ),
                            span: *local_span,
                        }
                    }
                }
                _ => {
                    return Err(diag(
                        "R0235",
                        "method receivers must be a local variable for now",
                        span,
                    )
                    .with_help(
                        "assign the value to a local first; expression receivers are not supported yet",
                    ));
                }
            };
            let mut lowered = vec![receiver_argument];
            for argument in arguments {
                let argument_span = argument.span();
                let parameter_type = signature
                    .parameters
                    .get(lowered.len())
                    .copied()
                    .unwrap_or(Type::Bool);
                let (value, actual) = self.expression(argument, Some(parameter_type))?;
                if actual != parameter_type {
                    return Err(diag(
                        "R0212",
                        format!(
                            "method argument has type `{}` but `{}` is required",
                            type_name(actual),
                            type_name(parameter_type)
                        ),
                        argument_span,
                    ));
                }
                lowered.push(value);
            }
            return Ok(Some((signature.target, lowered, signature.return_type)));
        }
        Ok(None)
    }

    fn lower_vec_method(
        &mut self,
        receiver: IrExpression,
        elem_id: usize,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let elem = vec_elem(elem_id);
        let elem_copyable = match elem {
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
            | Type::Char => true,
            Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) => true,
            Type::Struct(id) => {
                self.structs[id].drop_function.is_none()
                    && !contains_custom_drop(
                        elem,
                        self.structs,
                        self.enums,
                        &mut HashSet::new(),
                        &mut HashSet::new(),
                        0,
                    )
                    && vec_struct_fields_supported(&self.structs[id], self.structs)
            }
            _ => false,
        };
        if matches!(name.as_str(), "get" | "first" | "last") {
            let expected_arity = usize::from(name == "get");
            if arguments.len() != expected_arity {
                return Err(diag(
                    "R0211",
                    format!(
                        "method `{name}` expects {expected_arity} argument(s) but got {}",
                        arguments.len()
                    ),
                    span,
                ));
            }
            if !elem_copyable {
                return Err(diag(
                    "R0234",
                    format!(
                        "`Vec<{}>::{name}` requires cloneable elements",
                        type_name(elem)
                    ),
                    span,
                ));
            }
            let option_id = self
                .enums
                .iter()
                .position(|definition| is_option_of(definition, elem))
                .ok_or_else(|| {
                    diag(
                        "R0234",
                        format!(
                            "`Option<{}>` is not available for this Vec element type",
                            type_name(elem)
                        ),
                        span,
                    )
                })?;
            let index = if name == "get" {
                let argument = arguments.into_iter().next().unwrap();
                let arg_span = argument.span();
                let (value, actual) = self.expression(argument, Some(Type::U64))?;
                if actual != Type::U64 {
                    return Err(diag("R0212", "Vec.get index must be u64", arg_span));
                }
                value
            } else if name == "first" {
                IrExpression::Integer(0, Type::U64)
            } else {
                // The runtime recognizes the maximum u64 sentinel as "last" and
                // checks for an empty vector before choosing an element.
                IrExpression::Integer(u64::MAX.into(), Type::U64)
            };
            return Ok((
                IrCallTarget::VecGetOption {
                    elem_id,
                    option_id,
                    pop: false,
                },
                vec![receiver, index],
                Some(Type::Enum(option_id)),
            ));
        }
        if name == "pop" {
            if !arguments.is_empty() {
                return Err(diag(
                    "R0211",
                    format!(
                        "method `pop` expects no arguments but got {}",
                        arguments.len()
                    ),
                    span,
                ));
            }
            if !mutable {
                return Err(diag("R0204", "method `pop` requires a mutable Vec", span)
                    .with_help("declare the receiver's root binding with `mut`"));
            }
            let option_id = self
                .enums
                .iter()
                .position(|definition| is_option_of(definition, elem))
                .ok_or_else(|| {
                    diag(
                        "R0234",
                        format!(
                            "`Option<{}>` is not available for this Vec element type",
                            type_name(elem)
                        ),
                        span,
                    )
                })?;
            return Ok((
                IrCallTarget::VecGetOption {
                    elem_id,
                    option_id,
                    pop: true,
                },
                vec![receiver],
                Some(Type::Enum(option_id)),
            ));
        }
        let op = match name.as_str() {
            "clone" if elem_copyable => VecOp::Clone,
            "clone" => {
                return Err(diag(
                    "R0234",
                    format!(
                        "`Vec<{}>` contains move-only elements and cannot be cloned",
                        type_name(elem)
                    ),
                    span,
                ));
            }
            "len" => VecOp::Len,
            "is_empty" => VecOp::IsEmpty,
            "capacity" => VecOp::Capacity,
            "reserve" => VecOp::Reserve,
            "clear" => VecOp::Clear,
            "push" => VecOp::Push,
            "insert" => VecOp::Insert,
            "reverse" => VecOp::Reverse,
            "sort" => {
                if !matches!(
                    elem,
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
                        | Type::Char
                        | Type::OwnedString
                ) {
                    return Err(diag(
                        "R0234",
                        format!(
                            "`Vec<{}>::sort` currently supports numeric, char, and String elements",
                            type_name(elem)
                        ),
                        span,
                    ));
                }
                VecOp::Sort
            }
            "contains" => {
                if !matches!(
                    elem,
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
                ) {
                    return Err(diag(
                        "R0234",
                        format!(
                            "`Vec<{}>::contains` currently supports scalar and String elements",
                            type_name(elem)
                        ),
                        span,
                    ));
                }
                VecOp::Contains
            }
            "remove" => VecOp::Take,
            "set" => VecOp::Set,
            "take" => VecOp::Take,
            "extract" if elem_copyable => VecOp::Extract,
            "extract" => {
                return Err(diag(
                    "R0234",
                    format!(
                        "`Vec<{}>` elements must be copyable to read with `extract`",
                        type_name(elem)
                    ),
                    span,
                ));
            }
            "index" if elem_copyable => VecOp::Index,
            "index" => {
                return Err(diag(
                    "R0234",
                    format!(
                        "`Vec<{}>` elements must be copyable to read with `index`",
                        type_name(elem)
                    ),
                    span,
                ));
            }
            _ => {
                return Err(diag("R0234", format!("`Vec` has no method `{name}`"), span));
            }
        };
        if op.mutates() && !mutable {
            return Err(diag(
                "R0204",
                format!("method `{name}` requires a mutable Vec"),
                span,
            )
            .with_help("declare the receiver's root binding with `mut`"));
        }
        let parameters = op.parameters(elem).into_iter().skip(1).collect();
        let (target, mut arguments, result) = self.lower_builtin_call(
            name,
            IrCallTarget::Vec(op, elem_id),
            parameters,
            op.result(elem),
            arguments,
            span,
        )?;
        arguments.insert(0, receiver);
        Ok((target, arguments, result))
    }

    fn lower_map_method(
        &mut self,
        receiver: IrExpression,
        map_id: usize,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let (key, value) = map_info(map_id);
        let op = match name.as_str() {
            "len" => MapOp::Len,
            "is_empty" => MapOp::IsEmpty,
            "clone" => MapOp::Clone,
            "clear" => MapOp::Clear,
            "contains_key" | "contains" => MapOp::ContainsKey,
            "insert" | "add" => MapOp::Insert,
            "get" => MapOp::Get,
            "remove" => MapOp::Remove,
            "keys" => MapOp::Keys,
            "values" => MapOp::Values,
            _ => return Err(diag("R0234", format!("Map has no method `{name}`"), span)),
        };
        if op == MapOp::Get && !map_payload_type_supported(value, self.structs, 0) {
            return Err(diag(
                "R0234",
                "Map.get cannot clone this move-only or unsupported value type",
                span,
            ));
        }
        if op == MapOp::Values && !map_payload_type_supported(value, self.structs, 0) {
            return Err(diag(
                "R0234",
                "Map.values cannot clone this move-only or unsupported value type",
                span,
            ));
        }
        if op == MapOp::Clone && custom_drop_map_only(Type::Map(map_id), self.structs, self.enums) {
            return Err(diag(
                "R0234",
                "Map.clone cannot duplicate move-only custom-destructor values",
                span,
            ));
        }
        if op.mutates() && !mutable {
            return Err(diag(
                "R0204",
                format!("method `{name}` requires a mutable Map"),
                span,
            )
            .with_help("declare the receiver's root binding with `mut`"));
        }
        let mut arguments = arguments;
        if op == MapOp::Insert && value == Type::Bool && arguments.len() == 1 {
            arguments.push(Expression::Boolean(true, span));
        }
        let parameters = op.parameters(key, value).into_iter().skip(1).collect();
        let result_type = if op == MapOp::Get {
            self.enums
                .iter()
                .position(|definition| is_option_of(definition, value))
                .map(Type::Enum)
        } else {
            op.result(key, value)
        };
        let (target, mut arguments, result) = self.lower_builtin_call(
            name,
            IrCallTarget::Map(op, map_id),
            parameters,
            result_type,
            arguments,
            span,
        )?;
        arguments.insert(0, receiver);
        Ok((target, arguments, result))
    }

    fn lower_set_method(
        &mut self,
        receiver: IrExpression,
        map_id: usize,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let (element, _) = map_info(map_id);
        // Set<T> exposes membership operations that take a single element,
        // while the backing Map<T, bool> insert takes a key/value pair.
        // Do not route insert through Map's public method signature: lower it
        // directly and supply the set's marker value internally.
        if name == "insert" {
            if arguments.len() != 1 {
                return Err(diag(
                    "R0211",
                    format!(
                        "function `insert` expects 1 argument but got {}",
                        arguments.len()
                    ),
                    span,
                ));
            }
            let (value, actual) =
                self.expression(arguments.into_iter().next().unwrap(), Some(element))?;
            if actual != element {
                return Err(diag(
                    "R0212",
                    format!(
                        "argument to `insert` has type `{}` but `{}` is required",
                        type_name(actual),
                        type_name(element)
                    ),
                    span,
                ));
            }
            return Ok((
                IrCallTarget::Set(MapOp::Insert, map_id),
                vec![receiver, value, IrExpression::Boolean(true)],
                Some(Type::Bool),
            ));
        }
        let (target, arguments, result) =
            self.lower_map_method(receiver, map_id, name.clone(), arguments, span, mutable)?;
        let operation = match target {
            IrCallTarget::Map(operation, _) => operation,
            _ => return Err(diag("R0900", "internal Set method dispatch mismatch", span)),
        };
        if !matches!(
            operation,
            MapOp::Len
                | MapOp::IsEmpty
                | MapOp::Clear
                | MapOp::ContainsKey
                | MapOp::Insert
                | MapOp::Remove
                | MapOp::New
                | MapOp::Clone
        ) {
            return Err(diag("R0234", format!("Set has no method `{name}`"), span));
        }
        let result = if operation == MapOp::Insert {
            Some(Type::Bool)
        } else {
            result
        };
        let _ = element;
        Ok((IrCallTarget::Set(operation, map_id), arguments, result))
    }

    fn resolve_method_signature(
        &self,
        struct_id: usize,
        struct_name: &str,
        method: &str,
    ) -> Option<FunctionSignature> {
        let mut candidates = vec![format!("{struct_name}::{method}#method")];
        let module = &self.structs[struct_id].module_path;
        if !module.is_empty() {
            candidates.push(format!("{module}::{struct_name}::{method}#method"));
        }
        candidates
            .iter()
            .find_map(|candidate| self.signatures.get(candidate).cloned())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_struct_method(
        &mut self,
        receiver: IrExpression,
        receiver_ty: Type,
        struct_id: usize,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
        mutable: bool,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let struct_name = self.structs[struct_id].name.clone();
        let Some(signature) = self.resolve_method_signature(struct_id, &struct_name, &name) else {
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        };
        let IrCallTarget::Function(function_index) = signature.target else {
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        };
        if !self
            .function_visibility
            .get(function_index)
            .copied()
            .unwrap_or(false)
            && self
                .function_module_paths
                .get(function_index)
                .map(String::as_str)
                != Some(self.module_path.as_str())
        {
            return Err(diag(
                "R0425",
                format!("method `{name}` is private to `{struct_name}`'s module"),
                span,
            )
            .with_help("mark the method `pub` inside its `extend` block"));
        }
        let Some(self_binding) = signature.parameters.first().copied() else {
            return Err(diag(
                "R0900",
                "internal error: method has no receiver parameter",
                span,
            ));
        };
        let Type::Reference(_, self_mutable) = self_binding else {
            return Err(diag(
                "R0900",
                "internal error: method receiver is not a reference",
                span,
            ));
        };
        if self_mutable && !mutable {
            return Err(diag(
                "R0204",
                format!("method `{name}` requires a mutable receiver"),
                span,
            )
            .with_help("declare the receiver's root binding with `mut`"));
        }
        let receiver = if matches!(receiver, IrExpression::Local { .. }) {
            receiver
        } else if self_mutable {
            return Err(diag(
                "R0235",
                "method receivers must be a local variable for now",
                span,
            )
            .with_help("assign the value to a local first; `mut self` cannot borrow a temporary"));
        } else {
            self.materialize_copy_receiver(receiver, receiver_ty, span)?
        };
        let borrow = self.receiver_borrow(receiver, receiver_ty, self_mutable, span)?;
        let mut lowered = vec![borrow];
        for argument in arguments {
            let argument_span = argument.span();
            let parameter_type = signature
                .parameters
                .get(lowered.len())
                .copied()
                .unwrap_or_else(|| signature.parameters.last().copied().unwrap_or(Type::Bool));
            let (value, actual) = self.expression(argument, Some(parameter_type))?;
            if actual != parameter_type {
                return Err(diag(
                    "R0212",
                    format!(
                        "method argument has type `{}` but `{}` is required",
                        type_name(actual),
                        type_name(parameter_type)
                    ),
                    argument_span,
                ));
            }
            lowered.push(value);
        }
        Ok((signature.target, lowered, signature.return_type))
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_struct_method_borrowed(
        &mut self,
        receiver: IrExpression,
        receiver_ty: Type,
        reference_mutable: bool,
        struct_id: usize,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let struct_name = self.structs[struct_id].name.clone();
        let Some(signature) = self.resolve_method_signature(struct_id, &struct_name, &name) else {
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        };
        let IrCallTarget::Function(function_index) = signature.target else {
            return Err(diag(
                "R0234",
                format!("type `{}` has no method `{name}`", type_name(receiver_ty)),
                span,
            ));
        };
        if !self
            .function_visibility
            .get(function_index)
            .copied()
            .unwrap_or(false)
            && self
                .function_module_paths
                .get(function_index)
                .map(String::as_str)
                != Some(self.module_path.as_str())
        {
            return Err(diag(
                "R0425",
                format!("method `{name}` is private to `{struct_name}`'s module"),
                span,
            )
            .with_help("mark the method `pub` inside its `extend` block"));
        }
        let Some(self_binding) = signature.parameters.first().copied() else {
            return Err(diag(
                "R0900",
                "internal error: method has no receiver parameter",
                span,
            ));
        };
        let Type::Reference(_, self_mutable) = self_binding else {
            return Err(diag(
                "R0900",
                "internal error: method receiver is not a reference",
                span,
            ));
        };
        if self_mutable && !reference_mutable {
            return Err(diag(
                "R0204",
                format!("method `{name}` requires a mutable receiver"),
                span,
            )
            .with_help("the receiver borrow must be mutable"));
        }
        let mut lowered = vec![receiver];
        for argument in arguments {
            let argument_span = argument.span();
            let parameter_type = signature
                .parameters
                .get(lowered.len())
                .copied()
                .unwrap_or_else(|| signature.parameters.last().copied().unwrap_or(Type::Bool));
            let (value, actual) = self.expression(argument, Some(parameter_type))?;
            if actual != parameter_type {
                return Err(diag(
                    "R0212",
                    format!(
                        "method argument has type `{}` but `{}` is required",
                        type_name(actual),
                        type_name(parameter_type)
                    ),
                    argument_span,
                ));
            }
            lowered.push(value);
        }
        Ok((signature.target, lowered, signature.return_type))
    }

    /// Builds the receiver borrow for a method call on a struct-typed local.
    fn receiver_borrow(
        &mut self,
        receiver: IrExpression,
        receiver_ty: Type,
        mutable: bool,
        span: Span,
    ) -> Result<IrExpression, Diagnostic> {
        match receiver {
            IrExpression::Local { slot, ty, span: local_span } => {
                if let Type::Reference(_, reference_mutable) = ty {
                    if mutable && !reference_mutable {
                        return Err(diag(
                            "R0204",
                            "cannot borrow an immutable reference as mutable",
                            span,
                        ));
                    }
                    return Ok(IrExpression::Local {
                        slot,
                        ty,
                        span: local_span,
                    });
                }
                let pointer_type =
                    Type::Reference(intern_pointer_target(receiver_ty), mutable);
                self.addressed_slot_types[slot] = Some(receiver_ty);
                Ok(IrExpression::AddressOf {
                    slot,
                    ty: receiver_ty,
                    pointer_type,
                    span,
                })
            }
            _ => Err(diag(
                "R0235",
                "method receivers must be a local variable for now",
                span,
            )
            .with_help(
                "assign the value to a local first; chained receivers on expressions are not supported yet",
            )),
        }
    }

    fn coerce_function_pointer(
        &self,
        name: &str,
        target: &Type,
        span: Span,
    ) -> Result<Option<IrExpression>, Diagnostic> {
        let Type::FunctionPointer(signature_id) = target else {
            return Ok(None);
        };
        let pointer_signature = function_pointer_info(*signature_id);
        let resolved = if self.signatures.contains_key(name) {
            Some(name.to_string())
        } else {
            let namespace_name = self
                .function_name
                .rsplit_once("::")
                .map(|(namespace, _)| format!("{namespace}::{name}"));
            namespace_name
                .filter(|candidate| self.signatures.contains_key(candidate))
                .or_else(|| {
                    let local_name = if self.module_path.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}::{name}", self.module_path)
                    };
                    self.signatures
                        .contains_key(&local_name)
                        .then_some(local_name)
                })
        };
        let Some(resolved) = resolved else {
            return Ok(None);
        };
        let Some(function_signature) = self.signatures.get(&resolved) else {
            return Ok(None);
        };
        if function_signature.is_destructor {
            return Err(diag(
                "R0255",
                "destructor functions are called automatically by Ryn Guard",
                span,
            ));
        }
        let IrCallTarget::Function(function_index) = function_signature.target else {
            return Ok(None);
        };
        if !self
            .function_visibility
            .get(function_index)
            .copied()
            .unwrap_or(false)
            && self
                .function_module_paths
                .get(function_index)
                .map(String::as_str)
                != Some(self.module_path.as_str())
        {
            return Err(diag(
                "R0425",
                format!("function `{resolved}` is private in its module"),
                span,
            )
            .with_help("mark the function `pub` to use it through a module alias"));
        }
        if signature_has_record(&pointer_signature) {
            return Err(diag(
                "R0247",
                "function pointer coercion does not support C-ABI record parameters or results yet",
                span,
            )
            .with_help("use scalar, pointer, or function-pointer parameter types"));
        }
        if function_signature.parameters != pointer_signature.parameters
            || function_signature.return_type != pointer_signature.result
        {
            return Err(diag(
                "R0206",
                format!(
                    "function `{resolved}` does not match the function pointer type `{}`",
                    type_name(*target)
                ),
                span,
            )
            .with_help("the parameter and result types must match exactly"));
        }
        Ok(Some(IrExpression::FunctionAddress {
            function: function_index,
            ty: *target,
        }))
    }

    fn lower_call(
        &mut self,
        name: String,
        mut arguments: Vec<Expression>,
        span: Span,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        if self
            .signatures
            .get(&name)
            .is_some_and(|signature| signature.is_destructor)
        {
            return Err(diag(
                "R0255",
                "destructor functions are called automatically by Ryn Guard",
                span,
            ));
        }
        if let Some(binding) = self.names.get(&name).copied()
            && let Type::FunctionPointer(signature_id) = binding.ty
        {
            let signature = function_pointer_info(signature_id);
            if signature.extern_c
                && signature
                    .parameters
                    .iter()
                    .copied()
                    .chain(signature.result)
                    .any(|ty| {
                        matches!(ty, Type::Struct(id)
                            if c_abi_packed_record_layout(id, self.structs).is_none())
                    })
            {
                return Err(diag(
                    "R0247",
                    "C-ABI function pointer records must use the supported packed 1, 2, 4, or 8 byte layout",
                    span,
                ));
            }
            if arguments.len() != signature.parameters.len() {
                return Err(diag(
                    "R0211",
                    format!(
                        "function pointer expects {} arguments but got {}",
                        signature.parameters.len(),
                        arguments.len()
                    ),
                    span,
                ));
            }
            let mut lowered = vec![IrExpression::Local {
                slot: binding.slot,
                ty: binding.ty,
                span,
            }];
            for (argument, expected) in arguments.drain(..).zip(&signature.parameters) {
                let argument_span = argument.span();
                let (value, actual) = self.expression(argument, Some(*expected))?;
                if actual != *expected {
                    return Err(diag(
                        "R0212",
                        format!(
                            "function pointer argument has type `{}` but `{}` is required",
                            type_name(actual),
                            type_name(*expected)
                        ),
                        argument_span,
                    ));
                }
                lowered.push(value);
            }
            return Ok((
                IrCallTarget::IndirectFunctionPointer(signature_id),
                lowered,
                signature.result,
            ));
        }
        if name == "String" && arguments.len() == 1 {
            let argument = arguments.pop().unwrap();
            let argument_span = argument.span();
            let (value, ty) = self.expression(argument, None)?;
            let operation = match ty {
                Type::Str => StringOp::New,
                Type::OwnedString => StringOp::Clone,
                _ => StringOp::from_numeric_type(ty).ok_or_else(|| {
                    diag(
                        "R0205",
                        format!("String construction does not support `{}`", type_name(ty)),
                        argument_span,
                    )
                    .with_help("construct a String from `str` or a numeric value")
                })?,
            };
            return Ok((
                IrCallTarget::String(operation),
                vec![value],
                Some(Type::OwnedString),
            ));
        }
        if name == "String" && arguments.is_empty() {
            arguments.push(Expression::String(String::new(), span));
        }
        let raw_pointer = Type::RawPointer(intern_pointer_target(Type::U8));
        let builtin = match name.as_str() {
            "String" => Some((
                IrCallTarget::String(StringOp::New),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "char_from_u32" => Some((
                IrCallTarget::String(StringOp::CharFromU32),
                vec![Type::U32],
                Some(Type::Char),
            )),
            "arg_count" => Some((IrCallTarget::ArgumentCount, Vec::new(), Some(Type::U32))),
            "arg" => Some((IrCallTarget::Argument, vec![Type::U32], Some(Type::Str))),
            "read_file" | "__fs_read_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::ReadFile),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "try_read_file" => {
                let option_id = self
                    .enums
                    .iter()
                    .position(|definition| is_option_of(definition, Type::OwnedString))
                    .ok_or_else(|| {
                        diag("R0999", "internal error: Option<String> type missing", span)
                    })?;
                Some((
                    IrCallTarget::TryReadFile(option_id),
                    vec![Type::Str],
                    Some(Type::Enum(option_id)),
                ))
            }
            "read_file_result" => {
                let result_id = self
                    .enums
                    .iter()
                    .position(|definition| is_result_of(definition, Type::OwnedString, Type::I32))
                    .ok_or_else(|| {
                        diag(
                            "R0999",
                            "internal error: Result<String, i32> type missing",
                            span,
                        )
                    })?;
                Some((
                    IrCallTarget::ReadFileResult(result_id),
                    vec![Type::Str],
                    Some(Type::Enum(result_id)),
                ))
            }
            "write_file" | "__fs_write_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::WriteFile),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "append_file" | "__fs_append_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::AppendFile),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "__fs_create_dir" => Some((
                IrCallTarget::Filesystem(FilesystemOp::CreateDir),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "create_dir" => Some((
                IrCallTarget::Filesystem(FilesystemOp::CreateDir),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "create_dir_all" | "__fs_create_dir_all" => Some((
                IrCallTarget::Filesystem(FilesystemOp::CreateDirAll),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "delete_file" | "remove_file" | "__fs_remove_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::DeleteFile),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "delete_dir" | "remove_dir" | "__fs_remove_dir" => Some((
                IrCallTarget::Filesystem(FilesystemOp::DeleteDir),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "file_exists" | "__fs_file_exists" => Some((
                IrCallTarget::Filesystem(FilesystemOp::Exists),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "is_file" | "__fs_is_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::IsFile),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "is_directory" | "__fs_is_directory" => Some((
                IrCallTarget::Filesystem(FilesystemOp::IsDirectory),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "delete_dir_all" | "remove_dir_all" | "__fs_remove_dir_all" => Some((
                IrCallTarget::Filesystem(FilesystemOp::DeleteDirAll),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "read_dir" | "__fs_read_dir" => Some((
                IrCallTarget::Filesystem(FilesystemOp::ReadDir),
                vec![Type::Str],
                Some(Type::Vec(intern_vec_elem(Type::OwnedString))),
            )),
            "path_join" | "__fs_path_join" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathJoin),
                vec![Type::Str, Type::Str],
                Some(Type::OwnedString),
            )),
            "path_parent" | "__fs_path_parent" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathParent),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "path_file_name" | "__fs_path_file_name" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathFileName),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "path_extension" | "__fs_path_extension" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathExtension),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "path_is_absolute" | "__fs_path_is_absolute" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathIsAbsolute),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "path_absolute" | "__fs_path_absolute" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathAbsolute),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "path_canonical" | "__fs_path_canonical" => Some((
                IrCallTarget::Filesystem(FilesystemOp::PathCanonical),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "copy_file" | "__fs_copy_file" => Some((
                IrCallTarget::Filesystem(FilesystemOp::CopyFile),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "move_file" | "rename" | "__fs_rename" => Some((
                IrCallTarget::Filesystem(FilesystemOp::Rename),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "__file_open" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileOpen),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__file_create" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileCreate),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__file_read" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileRead),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__file_read_line" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileReadLine),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__file_write" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileWrite),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__file_flush" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileFlush),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__file_close" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileClose),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__file_drop" => Some((
                IrCallTarget::Filesystem(FilesystemOp::FileDrop),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "env_exists" => Some((
                IrCallTarget::System(SystemOp::EnvExists),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "env_or" => Some((
                IrCallTarget::System(SystemOp::EnvOr),
                vec![Type::Str, Type::Str],
                Some(Type::OwnedString),
            )),
            "__env_set" => Some((
                IrCallTarget::System(SystemOp::SetEnv),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "set_env" => Some((
                IrCallTarget::System(SystemOp::SetEnv),
                vec![Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "remove_env" => Some((
                IrCallTarget::System(SystemOp::RemoveEnv),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "stdin_read" => Some((
                IrCallTarget::System(SystemOp::ReadStdin),
                vec![],
                Some(Type::OwnedString),
            )),
            "stdin_read_line" | "read_line" => Some((
                IrCallTarget::System(SystemOp::ReadStdinLine),
                vec![],
                Some(Type::OwnedString),
            )),
            "ask" => Some((
                IrCallTarget::System(SystemOp::Ask),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "stdout_write" => Some((
                IrCallTarget::System(SystemOp::WriteStdout),
                vec![Type::Str],
                None,
            )),
            "stdout_flush" => Some((IrCallTarget::System(SystemOp::FlushStdout), vec![], None)),
            "stderr_write" => Some((
                IrCallTarget::System(SystemOp::WriteStderr),
                vec![Type::Str],
                None,
            )),
            "stderr_flush" => Some((IrCallTarget::System(SystemOp::FlushStderr), vec![], None)),
            "__io_read_line" => Some((
                IrCallTarget::System(SystemOp::ReadStdinLine),
                vec![],
                Some(Type::OwnedString),
            )),
            "__io_ask" => Some((
                IrCallTarget::System(SystemOp::Ask),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "__io_stdin_read" => Some((
                IrCallTarget::System(SystemOp::ReadStdin),
                vec![],
                Some(Type::OwnedString),
            )),
            "__io_stdout_write" => Some((
                IrCallTarget::System(SystemOp::WriteStdout),
                vec![Type::Str],
                None,
            )),
            "__io_stderr_write" => Some((
                IrCallTarget::System(SystemOp::WriteStderr),
                vec![Type::Str],
                None,
            )),
            "__io_stdout_flush" => {
                Some((IrCallTarget::System(SystemOp::FlushStdout), vec![], None))
            }
            "__io_stderr_flush" => {
                Some((IrCallTarget::System(SystemOp::FlushStderr), vec![], None))
            }
            "__input_key_down" => Some((
                IrCallTarget::System(SystemOp::KeyDown),
                vec![Type::U32],
                Some(Type::Bool),
            )),
            "__input_key_pressed" => Some((
                IrCallTarget::System(SystemOp::KeyPressed),
                vec![Type::U32],
                Some(Type::Bool),
            )),
            "__input_key_released" => Some((
                IrCallTarget::System(SystemOp::KeyReleased),
                vec![Type::U32],
                Some(Type::Bool),
            )),
            "__input_read_key" => Some((
                IrCallTarget::System(SystemOp::ReadKey),
                vec![],
                Some(Type::U32),
            )),
            "abs" | "__math_abs" => Some((
                IrCallTarget::System(SystemOp::MathAbs),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "min" | "__math_min" => Some((
                IrCallTarget::System(SystemOp::MathMin),
                vec![Type::F64, Type::F64],
                Some(Type::F64),
            )),
            "max" | "__math_max" => Some((
                IrCallTarget::System(SystemOp::MathMax),
                vec![Type::F64, Type::F64],
                Some(Type::F64),
            )),
            "clamp" | "__math_clamp" => Some((
                IrCallTarget::System(SystemOp::MathClamp),
                vec![Type::F64, Type::F64, Type::F64],
                Some(Type::F64),
            )),
            "sqrt" | "__math_sqrt" => Some((
                IrCallTarget::System(SystemOp::MathSqrt),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "pow" | "__math_pow" => Some((
                IrCallTarget::System(SystemOp::MathPow),
                vec![Type::F64, Type::F64],
                Some(Type::F64),
            )),
            "floor" | "__math_floor" => Some((
                IrCallTarget::System(SystemOp::MathFloor),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "ceil" | "__math_ceil" => Some((
                IrCallTarget::System(SystemOp::MathCeil),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "round" | "__math_round" => Some((
                IrCallTarget::System(SystemOp::MathRound),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "sin" | "__math_sin" => Some((
                IrCallTarget::System(SystemOp::MathSin),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "cos" | "__math_cos" => Some((
                IrCallTarget::System(SystemOp::MathCos),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "tan" | "__math_tan" => Some((
                IrCallTarget::System(SystemOp::MathTan),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "log" | "__math_log" => Some((
                IrCallTarget::System(SystemOp::MathLog),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "log2" | "__math_log2" => Some((
                IrCallTarget::System(SystemOp::MathLog2),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "log10" | "__math_log10" => Some((
                IrCallTarget::System(SystemOp::MathLog10),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "random" | "__math_random" => Some((
                IrCallTarget::System(SystemOp::Random),
                vec![],
                Some(Type::F64),
            )),
            "random_range" | "__math_random_range" => Some((
                IrCallTarget::System(SystemOp::RandomRange),
                vec![Type::F64, Type::F64],
                Some(Type::F64),
            )),
            "sleep" | "__time_sleep" => {
                Some((IrCallTarget::System(SystemOp::Sleep), vec![Type::U64], None))
            }
            "yield_thread" | "__time_yield_thread" => {
                Some((IrCallTarget::System(SystemOp::YieldThread), vec![], None))
            }
            "env" | "__env_get" => Some((
                IrCallTarget::System(SystemOp::EnvGet),
                vec![Type::Str],
                Some(Type::OwnedString),
            )),
            "current_dir" | "__current_dir" => Some((
                IrCallTarget::System(SystemOp::CurrentDir),
                vec![],
                Some(Type::OwnedString),
            )),
            "set_current_dir" | "__set_current_dir" => Some((
                IrCallTarget::System(SystemOp::SetCurrentDir),
                vec![Type::Str],
                Some(Type::Bool),
            )),
            "home_dir" | "__home_dir" => Some((
                IrCallTarget::System(SystemOp::HomeDir),
                vec![],
                Some(Type::OwnedString),
            )),
            "temp_dir" | "__temp_dir" => Some((
                IrCallTarget::System(SystemOp::TempDir),
                vec![],
                Some(Type::OwnedString),
            )),
            "executable_path" | "__executable_path" => Some((
                IrCallTarget::System(SystemOp::ExecutablePath),
                vec![],
                Some(Type::OwnedString),
            )),
            "os" | "__os" => Some((
                IrCallTarget::System(SystemOp::Os),
                vec![],
                Some(Type::OwnedString),
            )),
            "arch" | "__arch" => Some((
                IrCallTarget::System(SystemOp::Arch),
                vec![],
                Some(Type::OwnedString),
            )),
            "cpu_count" | "__cpu_count" => Some((
                IrCallTarget::System(SystemOp::CpuCount),
                vec![],
                Some(Type::U32),
            )),
            "hostname" | "__hostname" => Some((
                IrCallTarget::System(SystemOp::Hostname),
                vec![],
                Some(Type::OwnedString),
            )),
            "__time_unix" => Some((
                IrCallTarget::System(SystemOp::TimeUnix),
                vec![],
                Some(Type::F64),
            )),
            "__time_monotonic" => Some((
                IrCallTarget::System(SystemOp::TimeMonotonic),
                vec![],
                Some(Type::F64),
            )),
            "__time_instant_elapsed" => Some((
                IrCallTarget::System(SystemOp::TimeElapsed),
                vec![Type::F64],
                Some(Type::F64),
            )),
            "run_process" => Some((
                IrCallTarget::System(SystemOp::RunProcess),
                vec![Type::Str],
                Some(Type::I32),
            )),
            "run_process_args" => Some((
                IrCallTarget::System(SystemOp::RunProcessArgs),
                vec![Type::Str, Type::Vec(intern_vec_elem(Type::OwnedString))],
                Some(Type::I32),
            )),
            "__process_capture" => Some((
                IrCallTarget::System(SystemOp::ProcessCapture),
                vec![Type::Str, Type::Vec(intern_vec_elem(Type::OwnedString))],
                Some(raw_pointer),
            )),
            "__process_capture_exit_code" => Some((
                IrCallTarget::System(SystemOp::ProcessCaptureExitCode),
                vec![raw_pointer],
                Some(Type::I32),
            )),
            "__process_capture_stdout" => Some((
                IrCallTarget::System(SystemOp::ProcessCaptureStdout),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__process_capture_stderr" => Some((
                IrCallTarget::System(SystemOp::ProcessCaptureStderr),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__process_capture_drop" => Some((
                IrCallTarget::System(SystemOp::ProcessCaptureDrop),
                vec![raw_pointer],
                None,
            )),
            "__tcp_connect" => Some((
                IrCallTarget::System(SystemOp::TcpConnect),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__tcp_read" => Some((
                IrCallTarget::System(SystemOp::TcpRead),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__tcp_write" => Some((
                IrCallTarget::System(SystemOp::TcpWrite),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__tcp_close" => Some((
                IrCallTarget::System(SystemOp::TcpClose),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__tcp_drop" => Some((
                IrCallTarget::System(SystemOp::TcpDrop),
                vec![raw_pointer],
                None,
            )),
            "__tcp_listener_bind" => Some((
                IrCallTarget::System(SystemOp::TcpListenerBind),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__tcp_accept" => Some((
                IrCallTarget::System(SystemOp::TcpAccept),
                vec![raw_pointer],
                Some(raw_pointer),
            )),
            "__tcp_listener_close" => Some((
                IrCallTarget::System(SystemOp::TcpListenerClose),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__tcp_listener_drop" => Some((
                IrCallTarget::System(SystemOp::TcpListenerDrop),
                vec![raw_pointer],
                None,
            )),
            "__dns_resolve" => Some((
                IrCallTarget::System(SystemOp::DnsResolve),
                vec![Type::Str],
                Some(Type::Vec(intern_vec_elem(Type::OwnedString))),
            )),
            "__udp_bind" => Some((
                IrCallTarget::System(SystemOp::UdpBind),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__udp_connect" => Some((
                IrCallTarget::System(SystemOp::UdpConnect),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__udp_read" => Some((
                IrCallTarget::System(SystemOp::UdpRead),
                vec![raw_pointer],
                Some(Type::OwnedString),
            )),
            "__udp_write" => Some((
                IrCallTarget::System(SystemOp::UdpWrite),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__udp_close" => Some((
                IrCallTarget::System(SystemOp::UdpClose),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__udp_drop" => Some((
                IrCallTarget::System(SystemOp::UdpDrop),
                vec![raw_pointer],
                None,
            )),
            "__thread_spawn" => Some((
                IrCallTarget::System(SystemOp::ThreadSpawn),
                vec![Type::FunctionPointer(intern_function_pointer(
                    FunctionPointerSignature {
                        parameters: vec![],
                        result: None,
                        extern_c: true,
                    },
                ))],
                Some(raw_pointer),
            )),
            "__thread_join" => Some((
                IrCallTarget::System(SystemOp::ThreadJoin),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__thread_id" => Some((
                IrCallTarget::System(SystemOp::ThreadId),
                vec![raw_pointer],
                Some(Type::U64),
            )),
            "__thread_drop" => Some((
                IrCallTarget::System(SystemOp::ThreadDrop),
                vec![raw_pointer],
                None,
            )),
            "__process_new" => Some((
                IrCallTarget::System(SystemOp::ProcessNew),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "__process_arg" => Some((
                IrCallTarget::System(SystemOp::ProcessArg),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__process_args" => Some((
                IrCallTarget::System(SystemOp::ProcessArgs),
                vec![raw_pointer, Type::Vec(intern_vec_elem(Type::OwnedString))],
                Some(Type::Bool),
            )),
            "__process_env" => Some((
                IrCallTarget::System(SystemOp::ProcessEnv),
                vec![raw_pointer, Type::Str, Type::Str],
                Some(Type::Bool),
            )),
            "__process_cwd" => Some((
                IrCallTarget::System(SystemOp::ProcessCwd),
                vec![raw_pointer, Type::Str],
                Some(Type::Bool),
            )),
            "__process_spawn" => Some((
                IrCallTarget::System(SystemOp::ProcessSpawn),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__process_wait" => Some((
                IrCallTarget::System(SystemOp::ProcessWait),
                vec![raw_pointer],
                Some(Type::I32),
            )),
            "__process_kill" => Some((
                IrCallTarget::System(SystemOp::ProcessKill),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "__process_drop" => Some((
                IrCallTarget::System(SystemOp::ProcessDrop),
                vec![raw_pointer],
                None,
            )),
            "panic" | "__sys_panic" => {
                Some((IrCallTarget::System(SystemOp::Panic), vec![Type::Str], None))
            }
            "exit" | "__sys_exit" => {
                Some((IrCallTarget::System(SystemOp::Exit), vec![Type::I32], None))
            }
            "assert" | "__sys_assert" => Some((
                IrCallTarget::System(SystemOp::Assert),
                vec![Type::Bool],
                None,
            )),
            "assert_message" | "__sys_assert_message" => Some((
                IrCallTarget::System(SystemOp::AssertMessage),
                vec![Type::Bool, Type::Str],
                None,
            )),
            "load_library" => Some((
                IrCallTarget::System(SystemOp::LoadLibrary),
                vec![Type::Str],
                Some(raw_pointer),
            )),
            "load_symbol" => Some((
                IrCallTarget::System(SystemOp::LoadSymbol),
                vec![raw_pointer, Type::Str],
                Some(raw_pointer),
            )),
            "unload_library" => Some((
                IrCallTarget::System(SystemOp::UnloadLibrary),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "pointer_is_null" => Some((
                IrCallTarget::System(SystemOp::PointerIsNull),
                vec![raw_pointer],
                Some(Type::Bool),
            )),
            "call_c_i32_1" => Some((
                IrCallTarget::System(SystemOp::CallCI32One),
                vec![raw_pointer, Type::I32],
                Some(Type::I32),
            )),
            _ => None,
        };
        let resolved_name = if self.signatures.contains_key(&name) {
            name.clone()
        } else {
            let namespace_name = self
                .function_name
                .rsplit_once("::")
                .map(|(namespace, _)| format!("{namespace}::{name}"));
            namespace_name
                .filter(|candidate| self.signatures.contains_key(candidate))
                .or_else(|| {
                    let local_name = if self.module_path.is_empty() {
                        name.clone()
                    } else {
                        format!("{}::{name}", self.module_path)
                    };
                    self.signatures
                        .contains_key(&local_name)
                        .then_some(local_name)
                })
                .unwrap_or_else(|| name.clone())
        };
        let Some(sig) = self.signatures.get(&resolved_name).cloned() else {
            if let Some((target, parameters, return_type)) = builtin {
                return self.lower_builtin_call(
                    name,
                    target,
                    parameters,
                    return_type,
                    arguments,
                    span,
                );
            }
            let diagnostic = diag("R0210", format!("unknown function `{name}`"), span);
            if self.recover_block_errors {
                for argument in arguments {
                    if let Err(argument_error) = self.expression(argument, None) {
                        self.recovery_diagnostics.push(argument_error);
                    }
                }
            }
            let candidates = self.signatures.keys().map(String::as_str).chain(
                ["arg", "arg_count"]
                    .into_iter()
                    .filter(|builtin| !self.signatures.contains_key(*builtin)),
            );
            return Err(match closest_name(&name, candidates) {
                Some(suggestion) => diagnostic.with_help(format!("did you mean `{suggestion}`?")),
                None => diagnostic,
            });
        };
        if let IrCallTarget::Function(index) = sig.target
            && !self
                .function_visibility
                .get(index)
                .copied()
                .unwrap_or(false)
            && self.function_module_paths.get(index).map(String::as_str)
                != Some(self.module_path.as_str())
        {
            return Err(diag(
                "R0425",
                format!("function `{name}` is private to its module"),
                span,
            ));
        }
        let make_arity_error = || {
            diag(
                "R0211",
                format!(
                    "function `{name}` expects {} arguments but got {}",
                    sig.parameters.len(),
                    arguments.len()
                ),
                span,
            )
            .with_help(format!(
                "pass exactly {} argument{} to `{name}`",
                sig.parameters.len(),
                if sig.parameters.len() == 1 { "" } else { "s" }
            ))
        };
        let wrong_arity = arguments.len() != sig.parameters.len();
        if wrong_arity && !self.recover_block_errors {
            return Err(make_arity_error());
        }
        let arity_error = wrong_arity.then(make_arity_error);
        let mut lowered = Vec::with_capacity(arguments.len());
        for (index, argument) in arguments.into_iter().enumerate() {
            let argument_span = argument.span();
            let Some(expected) = sig.parameters.get(index) else {
                if let Err(diagnostic) = self.expression(argument, None) {
                    self.recovery_diagnostics.push(diagnostic);
                }
                continue;
            };
            match self.expression_for_parameter(argument, *expected) {
                Ok((argument, actual)) if actual == *expected => lowered.push(argument),
                // A `str` parameter borrows an owning String for the call.
                Ok((argument, Type::OwnedString)) if *expected == Type::Str => {
                    lowered.push(IrExpression::StringAsStr(Box::new(argument)));
                }
                Ok((argument, actual)) => {
                    let diagnostic = diag(
                        "R0212",
                        format!(
                            "argument to `{name}` has type `{}` but `{}` is required",
                            type_name(actual),
                            type_name(*expected)
                        ),
                        argument_span,
                    )
                    .with_help(format!(
                        "pass a `{}` value to `{name}`",
                        type_name(*expected)
                    ));
                    if self.recover_block_errors {
                        self.recovery_diagnostics.push(diagnostic);
                        lowered.push(argument);
                    } else {
                        return Err(diagnostic);
                    }
                }
                Err(diagnostic) if self.recover_block_errors => {
                    self.recovery_diagnostics.push(diagnostic);
                }
                Err(diagnostic) => return Err(diagnostic),
            }
        }
        if let Some(error) = arity_error {
            return Err(error);
        }
        Ok((sig.target, lowered, sig.return_type))
    }

    fn expression_for_parameter(
        &mut self,
        argument: Expression,
        expected: Type,
    ) -> Result<(IrExpression, Type), Diagnostic> {
        let Type::Slice(slice_id) = expected else {
            return self.expression(argument, Some(expected));
        };
        let hinted_array = self
            .value_type_hint(&argument)
            .filter(|ty| matches!(ty, Type::Array(_)));
        let literal_array = match &argument {
            Expression::ArrayLiteral(values, _) => {
                Some(Type::Array(intern_array(vec_elem(slice_id), values.len())))
            }
            Expression::ArrayRepeat { length, .. } => usize::try_from(*length)
                .ok()
                .map(|length| Type::Array(intern_array(vec_elem(slice_id), length))),
            _ => None,
        };
        let array_context = hinted_array.or(literal_array);
        if array_context.is_none() {
            return self.expression(argument, Some(expected));
        }
        let (value, actual) = self.expression(argument, array_context)?;
        if let Type::Array(array_id) = actual {
            let (element, length) = array_info(array_id);
            if element == vec_elem(slice_id) {
                return Ok((
                    IrExpression::ArrayAsSlice {
                        array: Box::new(value),
                        slice_id,
                        length,
                    },
                    expected,
                ));
            }
        }
        Ok((value, actual))
    }

    fn lower_builtin_call(
        &mut self,
        name: String,
        target: IrCallTarget,
        parameters: Vec<Type>,
        return_type: Option<Type>,
        arguments: Vec<Expression>,
        span: Span,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let arity_error = (arguments.len() != parameters.len()).then(|| {
            diag(
                "R0211",
                format!(
                    "function `{name}` expects {} arguments but got {}",
                    parameters.len(),
                    arguments.len()
                ),
                span,
            )
            .with_help(format!(
                "pass exactly {} argument{} to `{name}`",
                parameters.len(),
                if parameters.len() == 1 { "" } else { "s" }
            ))
        });
        if !self.recover_block_errors
            && let Some(error) = arity_error
        {
            return Err(error);
        }
        let mut lowered = Vec::with_capacity(parameters.len());
        for (index, argument) in arguments.into_iter().enumerate() {
            let argument_span = argument.span();
            let Some(expected) = parameters.get(index).copied() else {
                if let Err(error) = self.expression(argument, None) {
                    self.recovery_diagnostics.push(error);
                }
                continue;
            };
            // Builtins that read text borrow an owning String for the call;
            // the runtime call releases a temporary owner afterwards.
            let accepts_owned_path = expected == Type::Str
                && matches!(
                    target,
                    IrCallTarget::Filesystem(_) | IrCallTarget::String(_) | IrCallTarget::System(_)
                );
            let analyzed = if accepts_owned_path {
                self.expression(argument, None)
            } else {
                self.expression(argument, Some(expected))
            };
            match analyzed {
                Ok((value, actual)) if actual == expected => lowered.push(value),
                Ok((value, Type::OwnedString)) if accepts_owned_path => {
                    lowered.push(IrExpression::StringAsStr(Box::new(value)));
                }
                Ok((value, actual)) => {
                    let error = diag(
                        "R0212",
                        format!(
                            "argument to `{name}` has type `{}` but `{}` is required",
                            type_name(actual),
                            type_name(expected)
                        ),
                        argument_span,
                    )
                    .with_help(format!(
                        "pass a `{}` value to `{name}`",
                        type_name(expected)
                    ));
                    if self.recover_block_errors {
                        self.recovery_diagnostics.push(error);
                        lowered.push(value);
                    } else {
                        return Err(error);
                    }
                }
                Err(error) if self.recover_block_errors => self.recovery_diagnostics.push(error),
                Err(error) => return Err(error),
            }
        }
        if let Some(error) = arity_error {
            return Err(error);
        }
        Ok((target, lowered, return_type))
    }
}

fn reference_parameter_index(value: &IrExpression, parameters: &[LocalBinding]) -> Option<usize> {
    match value {
        IrExpression::Local {
            slot,
            ty: Type::Reference(_, _),
            ..
        } => parameters
            .iter()
            .position(|parameter| parameter.slot == *slot),
        IrExpression::SliceElementAddress { slice, .. } => match &**slice {
            IrExpression::Local {
                slot,
                ty: Type::Slice(_),
                ..
            } => parameters
                .iter()
                .position(|parameter| parameter.slot == *slot),
            _ => None,
        },
        IrExpression::If {
            then_value,
            else_value,
            ..
        } => {
            let then_index = reference_parameter_index(then_value, parameters)?;
            (reference_parameter_index(else_value, parameters) == Some(then_index))
                .then_some(then_index)
        }
        _ => None,
    }
}

fn collect_reference_returns(
    statements: &[IrStatement],
    parameters: &[LocalBinding],
    output: &mut Vec<Option<usize>>,
) {
    for statement in statements {
        match statement {
            IrStatement::Return {
                value: Some(value), ..
            } => {
                output.push(reference_parameter_index(value, parameters));
            }
            IrStatement::Block(body) => collect_reference_returns(body, parameters, output),
            IrStatement::If {
                then_body,
                else_body,
                ..
            } => {
                collect_reference_returns(then_body, parameters, output);
                collect_reference_returns(else_body, parameters, output);
            }
            IrStatement::While { body, .. } | IrStatement::For { body, .. } => {
                collect_reference_returns(body, parameters, output);
            }
            _ => {}
        }
    }
}

fn block_always_returns(statements: &[Statement]) -> bool {
    statements.iter().any(|statement| match statement {
        Statement::Return { .. } => true,
        Statement::If {
            then_body,
            else_body,
            ..
        } => {
            !else_body.is_empty()
                && block_always_returns(then_body)
                && block_always_returns(else_body)
        }
        _ => false,
    })
}

fn resolve_type_name(
    name: &TypeName,
    structs: &HashMap<String, usize>,
    enums: &HashMap<String, usize>,
) -> Result<Type, Diagnostic> {
    resolve_type_name_scoped(name, structs, enums, None)
}

fn declaration_namespace(name: &str, module_path: &str) -> Option<String> {
    let local_name = if module_path.is_empty() {
        name
    } else {
        name.strip_prefix(&format!("{module_path}::"))?
    };
    local_name
        .rsplit_once("::")
        .map(|(namespace, _)| namespace.to_string())
}

fn resolve_type_name_scoped(
    name: &TypeName,
    structs: &HashMap<String, usize>,
    enums: &HashMap<String, usize>,
    namespace: Option<&str>,
) -> Result<Type, Diagnostic> {
    Ok(match name {
        TypeName::I8 => Type::I8,
        TypeName::I16 => Type::I16,
        TypeName::I32 => Type::I32,
        TypeName::I64 => Type::I64,
        TypeName::U8 => Type::U8,
        TypeName::U16 => Type::U16,
        TypeName::U32 => Type::U32,
        TypeName::U64 => Type::U64,
        TypeName::F32 => Type::F32,
        TypeName::F64 => Type::F64,
        TypeName::Str => Type::Str,
        TypeName::OwnedString => Type::OwnedString,
        TypeName::Char => Type::Char,
        TypeName::Bool => Type::Bool,
        TypeName::Parameter(name, span) => {
            return Err(diag(
                "R0260",
                format!("unresolved generic type parameter `{name}`"),
                *span,
            ));
        }
        TypeName::Named(name, span) => {
            let qualified = namespace
                .filter(|_| !name.contains("::"))
                .map(|namespace| format!("{namespace}::{name}"));
            if let Some(id) = qualified.as_ref().and_then(|name| structs.get(name)) {
                Type::Struct(*id)
            } else if let Some(id) = qualified.as_ref().and_then(|name| enums.get(name)) {
                Type::Enum(*id)
            } else {
                match structs.get(name) {
                    Some(id) => Type::Struct(*id),
                    None => match enums.get(name) {
                        Some(id) => Type::Enum(*id),
                        None => {
                            return Err(diag("R0230", format!("unknown type `{name}`"), *span)
                                .with_help(
                                    "use a built-in type or declare this structure or enum",
                                ));
                        }
                    },
                }
            }
        }
        TypeName::Vec(element, span) => {
            let elem = resolve_type_name_scoped(element, structs, enums, namespace)?;
            validate_vec_elem(&elem, *span)?;
            Type::Vec(intern_vec_elem(elem))
        }
        TypeName::Set(element, span) => {
            let element = resolve_type_name_scoped(element, structs, enums, namespace)?;
            validate_map_key_lenient(element, *span)?;
            Type::Set(intern_map(element, Type::Bool))
        }
        TypeName::Map(key, value, span) => {
            let key = resolve_type_name_scoped(key, structs, enums, namespace)?;
            let value = resolve_type_name_scoped(value, structs, enums, namespace)?;
            validate_map_key_lenient(key, *span)?;
            validate_map_value_lenient(value, *span)?;
            Type::Map(intern_map(key, value))
        }
        TypeName::Array(element, length, span) => {
            let element = resolve_type_name_scoped(element, structs, enums, namespace)?;
            validate_array_elem(element, *span)?;
            if *length > i32::MAX as usize / 8 {
                return Err(diag(
                    "R0240",
                    "fixed array is too large for the native backend",
                    *span,
                ));
            }
            Type::Array(intern_array(element, *length))
        }
        TypeName::Slice(element, span) => {
            let element = resolve_type_name_scoped(element, structs, enums, namespace)?;
            validate_slice_elem(element, *span)?;
            Type::Slice(intern_vec_elem(element))
        }
        TypeName::Reference(element, mutable, _span) => {
            let element = resolve_type_name_scoped(element, structs, enums, namespace)?;
            Type::Reference(intern_pointer_target(element), *mutable)
        }
        TypeName::RawPointer(element, _) => {
            let element = resolve_type_name_scoped(element, structs, enums, namespace)?;
            Type::RawPointer(intern_pointer_target(element))
        }
        TypeName::FunctionPointer(parameters, result, extern_c, span) => {
            let parameters = parameters
                .iter()
                .map(|parameter| resolve_type_name_scoped(parameter, structs, enums, namespace))
                .collect::<Result<Vec<_>, _>>()?;
            let result = result
                .as_ref()
                .map(|result| resolve_type_name_scoped(result, structs, enums, namespace))
                .transpose()?;
            let supported = |ty: Type| {
                is_c_abi_scalar(ty)
                    || matches!(
                        ty,
                        Type::Char | Type::RawPointer(_) | Type::FunctionPointer(_)
                    )
                    || (!*extern_c
                        && matches!(
                            ty,
                            Type::OwnedString
                                | Type::Vec(_)
                                | Type::Map(_)
                                | Type::Set(_)
                                | Type::Enum(_)
                        ))
                    || (*extern_c && matches!(ty, Type::Struct(_)))
            };
            if parameters.iter().any(|ty| !supported(*ty))
                || result.is_some_and(|ty| !supported(ty))
            {
                return Err(diag(
                    "R0247",
                    "Ryn function pointers support scalar and owned pointer values; C-ABI records must fit the supported packed 1, 2, 4, or 8 byte layout",
                    *span,
                ));
            }
            let _ = extern_c;
            Type::FunctionPointer(intern_function_pointer(FunctionPointerSignature {
                parameters,
                result,
                extern_c: *extern_c,
            }))
        }
    })
}

fn validate_slice_elem(elem: Type, span: Span) -> Result<(), Diagnostic> {
    if !matches!(elem, Type::Slice(_)) {
        Ok(())
    } else {
        Err(diag(
            "R0240",
            format!(
                "borrowed slices of `{}` are not supported yet",
                type_name(elem)
            ),
            span,
        )
        .with_help("slice elements must be values with generated clone/drop support"))
    }
}

fn validate_array_elem(elem: Type, span: Span) -> Result<(), Diagnostic> {
    if matches!(elem, Type::Struct(_)) {
        return Ok(());
    }
    if matches!(
        elem,
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
            | Type::Struct(_)
            | Type::Enum(_)
            | Type::Str
            | Type::Array(_)
    ) {
        return Ok(());
    }
    Err(diag(
        "R0240",
        format!(
            "fixed arrays do not support `{}` elements yet",
            type_name(elem)
        ),
        span,
    ))
}

fn repeat_element_is_copy(ty: Type, structs: &[RynStruct]) -> bool {
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
        | Type::Str
        | Type::RawPointer(_)
        | Type::FunctionPointer(_)
        | Type::Reference(_, _) => true,
        Type::Array(id) => repeat_element_is_copy(array_info(id).0, structs),
        Type::Struct(id) => {
            structs[id].drop_function.is_none()
                && structs[id]
                    .fields
                    .iter()
                    .all(|field| repeat_element_is_copy(field.ty, structs))
        }
        Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Slice(_) => false,
    }
}

fn validate_array_literal_elem(
    elem: Type,
    structs: &[RynStruct],
    span: Span,
) -> Result<(), Diagnostic> {
    if array_element_clone_supported(elem, structs) {
        Ok(())
    } else if matches!(elem, Type::Struct(_)) {
        Err(diag(
            "R0240",
            format!(
                "fixed arrays cannot clone every field of `{}` elements yet",
                type_name(elem)
            ),
            span,
        )
        .with_help("array elements support recursively nested structures and arrays containing scalar, str, String, Vec, Map, Set, or enum fields; borrowed slices cannot be stored in array elements"))
    } else {
        validate_array_elem(elem, span)
    }
}

fn array_element_clone_supported(ty: Type, structs: &[RynStruct]) -> bool {
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
        | Type::Str
        | Type::OwnedString
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Reference(_, _)
        | Type::RawPointer(_)
        | Type::FunctionPointer(_) => true,
        Type::Vec(id) => array_element_clone_supported(vec_elem(id), structs),
        Type::Array(id) => array_element_clone_supported(array_info(id).0, structs),
        Type::Struct(id) => structs[id]
            .fields
            .iter()
            .all(|field| array_element_clone_supported(field.ty, structs)),
        Type::Slice(_) => false,
    }
}

fn validate_array_supported(ty: Type, structs: &[RynStruct], span: Span) -> Result<(), Diagnostic> {
    if let Type::Array(id) = ty {
        let (element, _) = array_info(id);
        validate_array_literal_elem(element, structs, span)?;
        validate_array_supported(element, structs, span)?;
    }
    Ok(())
}

fn validate_vec_elem(elem: &Type, span: Span) -> Result<(), Diagnostic> {
    match elem {
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
        | Type::Map(_)
        | Type::Struct(_) => Ok(()),
        other => Err(diag(
            "R0234",
            format!("`Vec<{}>` is not supported yet", type_name(*other)),
            span,
        )
        .with_help(
            "supported element types are integers, floats, `bool`, `char`, and `String` for now",
        )),
    }
}

fn validate_vec_struct_elements(
    ty: Type,
    structs: &[RynStruct],
    span: Span,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Vec(id) => {
            let element = vec_elem(id);
            if let Type::Struct(struct_id) = element
                && structs[struct_id].drop_function.is_none()
                && !vec_struct_fields_supported(&structs[struct_id], structs)
            {
                return Err(diag(
                    "R0234",
                    format!("`Vec<{}>` contains a field without generated clone/drop support", structs[struct_id].name),
                    span,
                )
                .with_help("use scalar, String, collection, enum, array, or nested-structure fields whose element types support cloning"));
            }
        }
        Type::Array(id) => validate_vec_struct_elements(array_info(id).0, structs, span)?,
        Type::Struct(id) => {
            for field in &structs[id].fields {
                validate_vec_struct_elements(field.ty, structs, span)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn vec_struct_fields_supported(structure: &RynStruct, structs: &[RynStruct]) -> bool {
    structure.drop_function.is_none()
        && structure
            .fields
            .iter()
            .all(|field| array_element_clone_supported(field.ty, structs))
}

fn name_span(name: &TypeName) -> Span {
    match name {
        TypeName::Named(_, span)
        | TypeName::Parameter(_, span)
        | TypeName::Vec(_, span)
        | TypeName::Map(_, _, span)
        | TypeName::Set(_, span)
        | TypeName::Array(_, _, span)
        | TypeName::Slice(_, span)
        | TypeName::Reference(_, _, span)
        | TypeName::RawPointer(_, span)
        | TypeName::FunctionPointer(_, _, _, span) => *span,
        _ => Span::default(),
    }
}

fn receiver_is_copyable(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Str
        | Type::Slice(_)
        | Type::Reference(_, _)
        | Type::FunctionPointer(_) => false,
        Type::Struct(_) => !type_has_owned_data_in_sema(ty, structs),
        Type::Array(id) => {
            let (element, _) = array_info(id);
            receiver_is_copyable(element, structs)
        }
        _ => true,
    }
}

fn type_has_owned_data_in_sema(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_) => true,
        Type::Struct(id) => {
            structs[id].drop_function.is_some()
                || structs[id]
                    .fields
                    .iter()
                    .any(|field| type_has_owned_data_in_sema(field.ty, structs))
        }
        _ => false,
    }
}

fn struct_key_hash_supported(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Bool
        | Type::Char => true,
        Type::Struct(id) => {
            let Some(definition) = structs.get(id) else {
                return false;
            };
            // Hashing flattens the key into words, so every leaf must be a
            // scalar; handle fields would hash unstable addresses.
            definition.derives_hash
                && definition
                    .fields
                    .iter()
                    .all(|field| struct_key_hash_supported(field.ty, structs))
        }
        _ => false,
    }
}

fn validate_map_key_lenient(key: Type, span: Span) -> Result<(), Diagnostic> {
    let key_supported = matches!(
        key,
        Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Bool
            | Type::Char
            | Type::OwnedString
    ) || matches!(key, Type::Struct(_));
    if !key_supported {
        return Err(diag(
            "R0244",
            format!(
                "Map key type `{}` does not implement Hash and Eq",
                type_name(key)
            ),
            span,
        )
        .with_help(
            "Map keys currently support integer types, bool, char, String, and `#[derive(Hash)]` structs with scalar fields",
        ));
    }
    Ok(())
}

fn validate_map_value_lenient(value: Type, _span: Span) -> Result<(), Diagnostic> {
    let value_supported = matches!(
        value,
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
    );
    let _ = value_supported;
    Ok(())
}

fn validate_map_types_with_structs(
    key: Type,
    value: Type,
    span: Span,
    structs: &[RynStruct],
) -> Result<(), Diagnostic> {
    let key_supported = matches!(
        key,
        Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Bool
            | Type::Char
            | Type::OwnedString
    ) || struct_key_hash_supported(key, structs);
    if !key_supported {
        return Err(diag(
            "R0244",
            format!(
                "Map key type `{}` does not implement Hash and Eq",
                type_name(key)
            ),
            span,
        )
        .with_help(
            "Map keys currently support integer types, bool, char, String, and `#[derive(Hash)]` structs with scalar fields",
        ));
    }
    let value_supported = matches!(
        value,
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
            | Type::Struct(_)
    );
    if !value_supported {
        return Err(diag(
            "R0244",
            format!("Map value type `{}` is not supported", type_name(value)),
            span,
        )
        .with_help("Map values currently support scalar types, String, and Vec<T>"));
    }
    Ok(())
}

fn collect_struct_declaration_errors(
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for definition in definitions {
        if definition.fields.is_empty() {
            diagnostics.push(
                diag(
                    "R0221",
                    "a structure must declare at least one field",
                    definition.span,
                )
                .with_help("add one or more typed fields to the structure"),
            );
            continue;
        }
        let mut names = HashSet::new();
        let namespace = definition.name.rsplit_once("::").map(|(path, _)| path);
        for field in &definition.fields {
            if !names.insert(field.name.as_str()) {
                diagnostics.push(
                    diag(
                        "R0222",
                        format!("duplicate field `{}`", field.name),
                        field.span,
                    )
                    .with_help("give each field in a structure a unique name"),
                );
            }
            match resolve_type_name_scoped(&field.ty, struct_ids, enum_ids, namespace) {
                Ok(Type::Slice(_)) => diagnostics.push(
                    diag(
                        "R0240",
                        "a borrowed slice cannot be stored in a structure field",
                        field.span,
                    )
                    .with_help("use the slice in a function parameter or store an owning `Vec<T>`"),
                ),
                Ok(_) => {}
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
    }
    diagnostics
}

fn collect_function_signature_errors(
    functions: &[Function],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for function in functions {
        let namespace = declaration_namespace(&function.name, &function.module_path);
        for parameter in &function.parameters {
            if let Err(diagnostic) =
                resolve_type_name_scoped(&parameter.ty, struct_ids, enum_ids, namespace.as_deref())
            {
                diagnostics.push(diagnostic);
            }
        }
        if let Some(return_type) = &function.return_type
            && let Err(diagnostic) =
                resolve_type_name_scoped(return_type, struct_ids, enum_ids, namespace.as_deref())
        {
            diagnostics.push(diagnostic);
        }
        if function.name == "main"
            && (!function.parameters.is_empty()
                || function
                    .return_type
                    .as_ref()
                    .and_then(|name| {
                        resolve_type_name_scoped(name, struct_ids, enum_ids, namespace.as_deref())
                            .ok()
                    })
                    .is_some_and(|return_type| return_type != Type::I32))
        {
            diagnostics.push(
                diag(
                    "R0208",
                    "`main` must take no parameters and return either no value or `i32`",
                    function.span,
                )
                .with_help("use `fun main() { ... }` or `fun main() -> i32 { ... }`; move reusable work into another function"),
            );
        }
    }
    diagnostics
}

fn collect_duplicate_parameter_errors(function: &Function) -> Vec<Diagnostic> {
    let mut names = HashSet::new();
    let mut diagnostics = Vec::new();
    for parameter in &function.parameters {
        if !names.insert(parameter.name.as_str()) {
            diagnostics.push(
                diag(
                    "R0202",
                    format!("duplicate parameter `{}`", parameter.name),
                    parameter.span,
                )
                .with_help("choose a unique name for each parameter in the function"),
            );
        }
    }
    diagnostics
}

fn resolve_enum_layouts(
    definitions: &[EnumDef],
    structs: &[RynStruct],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
) -> Result<Vec<RynEnum>, Diagnostic> {
    let mut enums = Vec::new();
    for definition in definitions {
        let namespace = definition.name.rsplit_once("::").map(|(path, _)| path);
        if definition.variants.is_empty() {
            return Err(diag(
                "R0233",
                format!("enum `{}` must have at least one variant", definition.name),
                definition.span,
            )
            .with_help("add a variant declaration inside the enum"));
        }
        let mut variants = Vec::new();
        let mut variant_names = HashSet::new();
        for variant in &definition.variants {
            if !variant_names.insert(variant.name.as_str()) {
                return Err(diag(
                    "R0233",
                    format!(
                        "duplicate variant `{}` in enum `{}`",
                        variant.name, definition.name
                    ),
                    variant.span,
                )
                .with_help("give each variant in the enum a unique name"));
            }
            let mut fields = Vec::new();
            for field in &variant.fields {
                let ty = resolve_type_name_scoped(field, struct_ids, enum_ids, namespace)?;
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
                    | Type::OwnedString => {}
                    Type::Enum(_) | Type::Map(_) => {}
                    Type::Struct(struct_id)
                        if enum_payload_struct_supported(Type::Struct(struct_id), structs, 0) => {}
                    Type::Array(array_id)
                        if enum_payload_struct_supported(Type::Array(array_id), structs, 0) => {}
                    Type::Vec(id) => match vec_elem(id) {
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
                        | Type::OwnedString => {}
                        _ => {
                            return Err(diag(
                                "R0234",
                                format!(
                                    "variant `{}::{}` uses a nested `Vec` element type that is not supported yet",
                                    definition.name, variant.name
                                ),
                                variant.span,
                            ));
                        }
                    },
                    _ => {
                        return Err(diag(
                            "R0234",
                            format!(
                                "variant `{}::{}` uses a field type that is not supported yet",
                                definition.name, variant.name
                            ),
                            variant.span,
                        )
                        .with_help(
                            "currently allowed field types are scalars, `String`, `Vec`, `Map`, arrays, structures without custom destructors, and enums",
                        ));
                    }
                }
                fields.push(ty);
            }
            variants.push(RynEnumVariant {
                name: variant.name.clone(),
                fields,
            });
        }
        enums.push(RynEnum {
            name: definition.name.clone(),
            variants,
        });
    }
    Ok(enums)
}

fn resolve_struct_layouts(
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
) -> Result<Vec<RynStruct>, Diagnostic> {
    let mut states = vec![0; definitions.len()];
    let mut layouts = vec![None; definitions.len()];
    for struct_id in 0..definitions.len() {
        resolve_struct_layout(
            struct_id,
            None,
            0,
            definitions,
            struct_ids,
            enum_ids,
            &mut states,
            &mut layouts,
        )?;
    }
    layouts
        .into_iter()
        .map(|layout| {
            layout.ok_or_else(|| {
                diag(
                    "R0900",
                    "internal error: unresolved structure layout",
                    Span::default(),
                )
            })
        })
        .collect()
}

fn collect_struct_layout_cycle_errors(
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
) -> Vec<Diagnostic> {
    let mut states = vec![0u8; definitions.len()];
    let mut diagnostics = Vec::new();
    for start in 0..definitions.len() {
        if states[start] != 0 {
            continue;
        }
        states[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some((struct_id, field_index)) = stack.last_mut() {
            let definition = &definitions[*struct_id];
            if *field_index == definition.fields.len() {
                states[*struct_id] = 2;
                stack.pop();
                continue;
            }

            let field = &definition.fields[*field_index];
            *field_index += 1;
            let TypeName::Named(name, _) = &field.ty else {
                continue;
            };
            let definition = &definitions[*struct_id];
            let namespace = definition.name.rsplit_once("::").map(|(path, _)| path);
            let qualified_name = namespace
                .filter(|_| !name.contains("::"))
                .map(|namespace| format!("{namespace}::{name}"));
            let Some(&child_id) = qualified_name
                .as_ref()
                .and_then(|name| struct_ids.get(name))
                .or_else(|| struct_ids.get(name))
            else {
                continue;
            };
            match states[child_id] {
                0 => {
                    states[child_id] = 1;
                    stack.push((child_id, 0));
                }
                1 => diagnostics.push(
                    diag(
                        "R0232",
                        format!(
                            "structure `{}` contains itself by value",
                            definitions[child_id].name
                        ),
                        field.span,
                    )
                    .with_help(
                        "break the recursive field chain; Ryn does not support indirect or recursive storage yet",
                    ),
                ),
                _ => {}
            }
        }
    }
    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn resolve_struct_layout(
    struct_id: usize,
    via_field: Option<Span>,
    depth: usize,
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
    states: &mut [u8],
    layouts: &mut [Option<RynStruct>],
) -> Result<(), Diagnostic> {
    if depth > MAX_STRUCT_NESTING {
        return Err(diag(
            "R0233",
            "structure nesting exceeds the compiler limit",
            via_field.unwrap_or(definitions[struct_id].span),
        )
        .with_help("reduce the number of nested by-value structures"));
    }
    if states[struct_id] == 2 {
        return Ok(());
    }
    if states[struct_id] == 1 {
        return Err(diag(
            "R0232",
            format!(
                "structure `{}` contains itself by value",
                definitions[struct_id].name
            ),
            via_field.unwrap_or(definitions[struct_id].span),
        )
        .with_help(
            "break the recursive field chain; Ryn does not support indirect or recursive storage yet",
        ));
    }
    let definition = &definitions[struct_id];
    if definition.fields.is_empty() {
        return Err(diag(
            "R0221",
            "a structure must declare at least one field",
            definition.span,
        )
        .with_help("add one or more typed fields to the structure"));
    }
    states[struct_id] = 1;
    let mut fields = Vec::with_capacity(definition.fields.len());
    let mut names = HashMap::new();
    let mut slot_count = 0usize;
    for field in &definition.fields {
        if names.insert(field.name.clone(), ()).is_some() {
            return Err(diag(
                "R0222",
                format!("duplicate field `{}`", field.name),
                field.span,
            )
            .with_help("give each field in a structure a unique name"));
        }
        let namespace = definitions[struct_id]
            .name
            .rsplit_once("::")
            .map(|(path, _)| path);
        let ty = resolve_type_name_scoped(&field.ty, struct_ids, enum_ids, namespace)?;
        let width = resolve_storage_width(
            ty,
            field.span,
            depth + 1,
            definitions,
            struct_ids,
            enum_ids,
            states,
            layouts,
        )?;
        fields.push(RynStructField {
            name: field.name.clone(),
            ty,
            slot_offset: slot_count,
            public: field.public,
        });
        slot_count = slot_count
            .checked_add(width)
            .filter(|slots| *slots <= i32::MAX as usize / 8)
            .ok_or_else(|| {
                diag(
                    "R0231",
                    "structure layout is too large for the native backend",
                    field.span,
                )
                .with_help("reduce the number of fields in this structure")
            })?;
    }
    layouts[struct_id] = Some(RynStruct {
        module_path: definitions[struct_id].module_path.clone(),
        derives_clone: definitions[struct_id]
            .derives
            .iter()
            .any(|name| name == "Clone"),
        derives_hash: definitions[struct_id]
            .derives
            .iter()
            .any(|name| name == "Hash"),
        name: definition.name.clone(),
        fields,
        slot_count,
        repr_c: definition.repr_c,
        drop_function: None,
    });
    states[struct_id] = 2;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolve_storage_width(
    ty: Type,
    span: Span,
    depth: usize,
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
    enum_ids: &HashMap<String, usize>,
    states: &mut [u8],
    layouts: &mut [Option<RynStruct>],
) -> Result<usize, Diagnostic> {
    match ty {
        Type::Struct(child_id) => {
            resolve_struct_layout(
                child_id,
                Some(span),
                depth,
                definitions,
                struct_ids,
                enum_ids,
                states,
                layouts,
            )?;
            layouts[child_id]
                .as_ref()
                .map(|layout| layout.slot_count)
                .ok_or_else(|| {
                    diag(
                        "R0900",
                        "internal error: child structure layout is missing",
                        span,
                    )
                })
        }
        Type::Array(id) => {
            let (element, length) = array_info(id);
            resolve_storage_width(
                element,
                span,
                depth + 1,
                definitions,
                struct_ids,
                enum_ids,
                states,
                layouts,
            )?
            .checked_mul(length)
            .ok_or_else(|| {
                diag(
                    "R0231",
                    "structure layout is too large for the native backend",
                    span,
                )
            })
        }
        _ => Ok(slot_width(ty)),
    }
}

pub fn slot_width(ty: Type) -> usize {
    match ty {
        Type::Str => 2,
        Type::Slice(_) => 2,
        Type::Array(id) => {
            let (element, length) = array_info(id);
            length.saturating_mul(slot_width(element))
        }
        _ => 1,
    }
}

pub(crate) fn storage_slot_width(ty: Type, structs: &[RynStruct]) -> usize {
    match ty {
        Type::Struct(id) => structs[id].slot_count,
        Type::Array(id) => {
            let (element, length) = array_info(id);
            length.saturating_mul(storage_slot_width(element, structs))
        }
        Type::Str | Type::Slice(_) => 2,
        _ => 1,
    }
}

pub(crate) fn type_layout(ty: Type, structs: &[RynStruct]) -> (usize, usize) {
    fn align_up(size: usize, align: usize) -> usize {
        size.saturating_add(align - 1) / align * align
    }

    match ty {
        Type::I8 | Type::U8 | Type::Bool => (1, 1),
        Type::I16 | Type::U16 => (2, 2),
        Type::I32 | Type::U32 | Type::F32 | Type::Char => (4, 4),
        Type::I64
        | Type::U64
        | Type::F64
        | Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Reference(_, _)
        | Type::RawPointer(_)
        | Type::FunctionPointer(_) => (8, 8),
        Type::Str | Type::Slice(_) => (16, 8),
        Type::Array(id) => {
            let (element, length) = array_info(id);
            let (element_size, element_align) = type_layout(element, structs);
            (
                align_up(element_size, element_align).saturating_mul(length),
                element_align,
            )
        }
        Type::Struct(id) => {
            let mut size = 0usize;
            let mut align = 1usize;
            for field in &structs[id].fields {
                let (field_size, field_align) = type_layout(field.ty, structs);
                size = align_up(size, field_align).saturating_add(field_size);
                align = align.max(field_align);
            }
            (align_up(size, align), align)
        }
    }
}

fn type_contains_vec(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::Vec(_) => true,
        Type::Array(id) => type_contains_vec(array_info(id).0, structs),
        Type::Struct(id) => structs[id]
            .fields
            .iter()
            .any(|field| type_contains_vec(field.ty, structs)),
        _ => false,
    }
}

fn is_generic_enum_name(internal: &str, requested: &str) -> bool {
    match requested {
        "Option" => internal.starts_with("$RynOption#"),
        "Result" => internal.starts_with("$RynResult#"),
        _ => internal
            .strip_prefix("$RynEnum#")
            .and_then(|name| name.rsplit_once('#').map(|(template, _)| template))
            .is_some_and(|template| {
                template == requested
                    || template
                        .rsplit_once("::")
                        .is_some_and(|(_, final_name)| final_name == requested)
            }),
    }
}

fn type_name(ty: Type) -> String {
    match ty {
        Type::I8 => "i8".into(),
        Type::I16 => "i16".into(),
        Type::I32 => "i32".into(),
        Type::I64 => "i64".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Str => "str".into(),
        Type::OwnedString => "String".into(),
        Type::Char => "char".into(),
        Type::Bool => "bool".into(),
        Type::Struct(_) => "structure".into(),
        Type::Vec(id) => format!("Vec<{}>", type_name(vec_elem(id))),
        Type::Map(id) => {
            let (key, value) = map_info(id);
            format!("Map<{}, {}>", type_name(key), type_name(value))
        }
        Type::Set(id) => format!("Set<{}>", type_name(map_info(id).0)),
        Type::Enum(_) => "enum".into(),
        Type::Array(id) => {
            let (element, length) = array_info(id);
            format!("[{}; {length}]", type_name(element))
        }
        Type::Slice(id) => format!("&[{}]", type_name(vec_elem(id))),
        Type::Reference(id, mutable) => format!(
            "&{}{}",
            if mutable { "mut " } else { "" },
            type_name(pointer_target(id))
        ),
        Type::RawPointer(id) => format!("*{}", type_name(pointer_target(id))),
        Type::FunctionPointer(id) => {
            let signature = function_pointer_info(id);
            let parameters = signature
                .parameters
                .iter()
                .map(|parameter| type_name(*parameter))
                .collect::<Vec<_>>()
                .join(", ");
            let prefix = if signature.extern_c {
                "extern \"C\" "
            } else {
                ""
            };
            match signature.result {
                Some(result) => format!("{prefix}fun({parameters}) -> {}", type_name(result)),
                None => format!("{prefix}fun({parameters})"),
            }
        }
    }
}
fn reference_pointee_supported(ty: Type) -> bool {
    reference_pointee_supported_in(ty, &[])
}

// Whole-record pointer copies cannot duplicate owning fields or borrowed views.
fn pointer_record_is_copy(ty: Type, structs: &[RynStruct]) -> bool {
    match ty {
        Type::Struct(id) => {
            structs[id].drop_function.is_none()
                && structs[id]
                    .fields
                    .iter()
                    .all(|field| pointer_record_is_copy(field.ty, structs))
        }
        Type::Array(id) => pointer_record_is_copy(array_info(id).0, structs),
        Type::OwnedString
        | Type::Vec(_)
        | Type::Map(_)
        | Type::Set(_)
        | Type::Enum(_)
        | Type::Str
        | Type::Slice(_)
        | Type::Reference(_, _) => false,
        _ => true,
    }
}

fn reference_pointee_supported_in(ty: Type, structs: &[RynStruct]) -> bool {
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
            | Type::F32
            | Type::F64
            | Type::Char
            | Type::Bool
            | Type::RawPointer(_)
            | Type::FunctionPointer(_)
    ) {
        return true;
    }
    if let Type::Struct(id) = ty {
        // Borrowing never duplicates a resource, but move-only custom-destructor
        // handles keep their whole-value replacement rule for now.
        return structs
            .get(id)
            .is_none_or(|definition| definition.drop_function.is_none());
    }
    // Owning handles live behind their stack home; dereference reads clone.
    matches!(
        ty,
        Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Set(_) | Type::Enum(_)
    )
}

fn is_integer(ty: Type) -> bool {
    matches!(
        ty,
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::U8 | Type::U16 | Type::U32 | Type::U64
    )
}

fn closest_name<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    // Ryn identifiers are ASCII. Keep typo work bounded for very long malformed names.
    if name.len() > 64 {
        return None;
    }
    let limit = (name.len() / 3).clamp(1, 2);
    let mut closest = None;
    let mut best_distance = limit + 1;
    let mut ambiguous = false;

    for candidate in candidates {
        if candidate.len() > 64 {
            continue;
        }
        let Some(distance) = edit_distance_within(name, candidate, limit) else {
            continue;
        };
        if distance < best_distance {
            closest = Some(candidate);
            best_distance = distance;
            ambiguous = false;
        } else if distance == best_distance {
            ambiguous = true;
        }
    }

    if ambiguous { None } else { closest }
}

fn edit_distance_within(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }

    let outside_limit = limit + 1;
    let mut before_previous = vec![outside_limit; right.len() + 1];
    let mut previous = vec![outside_limit; right.len() + 1];
    let mut current = vec![outside_limit; right.len() + 1];
    for (index, value) in previous
        .iter_mut()
        .take(limit.min(right.len()) + 1)
        .enumerate()
    {
        *value = index;
    }

    for left_index in 1..=left.len() {
        current.fill(outside_limit);
        let first = left_index.saturating_sub(limit).max(1);
        let last = (left_index + limit).min(right.len());
        if left_index <= limit {
            current[0] = left_index;
        }
        if first <= last {
            for right_index in first..=last {
                let substitution_cost = usize::from(left[left_index - 1] != right[right_index - 1]);
                let mut distance = (previous[right_index] + 1)
                    .min(current[right_index - 1] + 1)
                    .min(previous[right_index - 1] + substitution_cost);
                if left_index > 1
                    && right_index > 1
                    && left[left_index - 1] == right[right_index - 2]
                    && left[left_index - 2] == right[right_index - 1]
                {
                    distance = distance.min(before_previous[right_index - 2] + 1);
                }
                current[right_index] = distance;
            }
        }
        std::mem::swap(&mut before_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }

    (previous[right.len()] <= limit).then_some(previous[right.len()])
}
fn is_signed_integer(ty: Type) -> bool {
    matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64)
}
fn is_numeric(ty: Type) -> bool {
    is_integer(ty) || matches!(ty, Type::F32 | Type::F64)
}
fn is_equality_type(ty: Type) -> bool {
    is_numeric(ty)
        || matches!(
            ty,
            Type::Str
                | Type::OwnedString
                | Type::Char
                | Type::Bool
                | Type::Struct(_)
                | Type::RawPointer(_)
                | Type::FunctionPointer(_)
        )
}
fn integer_bounds(ty: Type) -> (i128, i128) {
    match ty {
        Type::I8 => (i8::MIN as i128, i8::MAX as i128),
        Type::I16 => (i16::MIN as i128, i16::MAX as i128),
        Type::I32 => (i32::MIN as i128, i32::MAX as i128),
        Type::I64 => (i64::MIN as i128, i64::MAX as i128),
        Type::U8 => (0, u8::MAX as i128),
        Type::U16 => (0, u16::MAX as i128),
        Type::U32 => (0, u32::MAX as i128),
        Type::U64 => (0, u64::MAX as i128),
        _ => unreachable!("integer bounds requested for non-integer type"),
    }
}
fn diag(code: &'static str, message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic {
        code,
        message: message.into(),
        span,
        help: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_STRUCT_NESTING, analyze, closest_name, edit_distance_within};
    use crate::parser::parse;

    fn error(source: &str) -> Diagnostic {
        analyze(parse(source).expect("source parses"))
            .expect_err("source should fail semantic analysis")
    }
    use crate::source::Diagnostic;

    #[test]
    fn enum_choose_requires_exhaustive_variants_and_a_final_wildcard() {
        let missing = error(
            "enum Flag { On, Off } fun main() { flag := Flag::On result := choose flag { Flag::On => 1 } }",
        );
        assert_eq!(missing.code, "R0235");
        assert!(missing.message.contains("not exhaustive"));

        let misplaced = error(
            "enum Flag { On, Off } fun main() { flag := Flag::On result := choose flag { _ => 1, Flag::On => 2 } }",
        );
        assert_eq!(misplaced.code, "R0235");
        assert!(misplaced.message.contains("must be last"));
    }

    #[test]
    fn process_argument_builtins_have_checked_signatures_and_can_be_shadowed() {
        let wrong_count = error("fun main() { arg_count(0) }");
        assert_eq!(wrong_count.code, "R0211");

        let wrong_index_type = error("fun main() { arg(\"0\") }");
        assert_eq!(wrong_index_type.code, "R0212");

        analyze(
            parse("fun arg_count() -> i32 { 9 } fun main() { echo(arg_count()) }")
                .expect("shadowing source parses"),
        )
        .expect("a user function may shadow the builtin name");
    }

    #[test]
    fn rejects_assignment_to_immutable_local() {
        let diagnostic = error("fun main() { value := 1 value = 2 }");
        assert_eq!(diagnostic.code, "R0204");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("declare `mut value := ...` if this binding must be mutable")
        );

        let compound = error("fun main() { value := 1 value += 2 }");
        assert_eq!(compound.code, "R0204");
        assert_eq!(compound.message, "`value` is immutable");
    }

    #[test]
    fn structural_diagnostics_suggest_direct_repairs() {
        let missing_main = analyze(parse("fun helper() {}").expect("source parses"))
            .expect_err("program without main should fail");
        assert_eq!(missing_main.code, "R0200");
        assert_eq!(
            missing_main.help.as_deref(),
            Some("add a top-level `fun main() { ... }` function")
        );

        let duplicate_function = error("fun main() {} fun main() {}");
        assert_eq!(duplicate_function.code, "R0201");
        assert_eq!(
            duplicate_function.help.as_deref(),
            Some("give each function a unique name; function overloading is not supported")
        );

        let duplicate_parameter = error("fun take(value: i32, value: i32) {} fun main() {}");
        assert_eq!(duplicate_parameter.code, "R0202");
        assert_eq!(
            duplicate_parameter.help.as_deref(),
            Some("choose a unique name for each parameter in the function")
        );

        let duplicate_local = error("fun main() { value := 1 value := 2 }");
        assert_eq!(duplicate_local.code, "R0202");
        assert_eq!(
            duplicate_local.help.as_deref(),
            Some("choose a different name or remove the second declaration of `value`")
        );

        let invalid_main = error("fun main(value: i32) {}");
        assert_eq!(invalid_main.code, "R0208");
        assert_eq!(
            invalid_main.help.as_deref(),
            Some(
                "use `fun main() { ... }` or `fun main() -> i32 { ... }`; move reusable work into another function"
            )
        );

        let unsupported_main_result = error("fun main() -> i64 { 0 }");
        assert_eq!(unsupported_main_result.code, "R0208");
    }
    #[test]
    fn rejects_arithmetic_on_strings() {
        assert_eq!(error("fun main() { value := \"x\" + \"y\" }").code, "R0206");
    }

    #[test]
    fn bitwise_operators_require_matching_integer_types() {
        analyze(
            parse(
                "fun main() { left: u8 = 0b1010 right: u8 = 0b1100 both := left & right either := left | right different := left ^ right inverted := ~left echo(both) echo(either) echo(different) echo(inverted) }",
            )
            .expect("integer bitwise source parses"),
        )
        .expect("matching integer bitwise operands type-check");

        let mismatched = error("fun main() { left: i32 = 1 right: u32 = 2 value := left & right }");
        assert_eq!(mismatched.code, "R0206");
        assert_eq!(
            mismatched.message,
            "bitwise operands must have matching integer types, found `i32` and `u32`"
        );
        assert_eq!(
            &"fun main() { left: i32 = 1 right: u32 = 2 value := left & right }"
                [mismatched.span.start..mismatched.span.end],
            "right"
        );

        assert_eq!(error("fun main() { value := 1.0 & 2.0 }").code, "R0206");
        assert_eq!(error("fun main() { value := ~true }").code, "R0206");
    }

    #[test]
    fn if_expression_requires_a_boolean_condition_and_matching_branch_types() {
        analyze(
            parse("fun main() { result: i32 = when true { 1 } else { 2 } }")
                .expect("source parses"),
        )
        .expect("matching when-expression branches type-check");

        let condition = error("fun main() { result := when 1 { 1 } else { 2 } }");
        assert_eq!(condition.code, "R0207");
        assert_eq!(
            condition.message,
            "`when` expression condition must have type `bool`"
        );

        let source = "fun main() { result := when true { 1 } else { 2.0 } }";
        let branches = error(source);
        let else_value = source.rfind("2.0").expect("else value is present");
        assert_eq!(branches.code, "R0205");
        assert_eq!(branches.span.start, else_value);
        assert_eq!(branches.span.end, else_value + "2.0".len());
        assert_eq!(
            branches.message,
            "when-expression branches must have the same type, found `i64` and `f64`"
        );
    }

    #[test]
    fn if_statements_with_void_calls_remain_statements_before_a_tail_value() {
        let source = "fun notify() { echo(1) } fun answer(flag: bool) -> i32 { when flag { notify() } else { notify() } 42 } fun main() { echo(answer(false)) }";
        analyze(parse(source).expect("source parses"))
            .expect("statement branches do not become void-valued if expressions");
    }

    #[test]
    fn checks_i32_literal_range_and_mixed_integer_types() {
        assert_eq!(
            error("fun main() { value: i32 = 2147483648 }").code,
            "R0206"
        );
        assert_eq!(
            error("fun main() { value: i32 = 1 wide: i64 = 2 echo(value + wide) }").code,
            "R0206"
        );
    }

    #[test]
    fn compound_assignment_requires_compatible_numeric_types() {
        let diagnostic = error("fun main() { mut narrow: i32 = 1 wide: i64 = 2 narrow += wide }");
        assert_eq!(diagnostic.code, "R0206");
        assert_eq!(
            diagnostic.message,
            "arithmetic operands must have matching numeric types, found `i32` and `i64`"
        );
        assert_eq!(
            diagnostic.span.start,
            "fun main() { mut narrow: i32 = 1 wide: i64 = 2 narrow += wide }"
                .rfind("wide")
                .expect("right operand is present")
        );
    }

    #[test]
    fn bitwise_compound_assignment_requires_matching_integer_types() {
        let float = error("fun main() { mut value: f32 = 1.0 value &= 1.0 }");
        assert_eq!(float.code, "R0206");
        assert!(float.message.contains("bitwise operands"));

        let mixed = error("fun main() { mut narrow: i32 = 1 wide: i64 = 2 narrow |= wide }");
        assert_eq!(mixed.code, "R0206");
        assert!(
            mixed
                .message
                .contains("bitwise operands must have matching integer types")
        );
    }

    #[test]
    fn shifts_require_an_integer_value_and_u32_count() {
        let mixed_count = error("fun main() { amount: i32 = 2 echo(1 << amount) }");
        assert_eq!(mixed_count.code, "R0206");
        assert!(mixed_count.message.contains("`u32` count"));

        analyze(
            parse("fun main() { amount: u32 = 2 value: i8 = 1 << amount echo(value) }")
                .expect("typed shift source parses"),
        )
        .expect("the left value keeps its type and the shift count is u32");
    }

    #[test]
    fn numeric_casts_accept_numeric_sources_and_targets() {
        analyze(
            parse("fun main() { signed: i8 = -1 widened: u16 = signed as u16 narrowed: i8 = widened as i8 maximum: u64 = 18446744073709551615 reinterpret: i64 = maximum as i64 small: i32 = 16777217 rounded: f32 = small as f32 promoted: f64 = rounded as f64 fractional: f64 = 12.9 integer: i32 = fractional as i32 }")
                .expect("numeric cast source parses"),
        )
        .expect("numeric casts support integer and floating-point conversions");

        let non_numeric_source = error("fun main() { value := true as i32 }");
        assert_eq!(non_numeric_source.code, "R0206");
        assert!(
            non_numeric_source
                .message
                .contains("numeric cast requires a numeric value")
        );

        let non_numeric_target = error("fun main() { value := 1 as bool }");
        assert_eq!(non_numeric_target.code, "R0206");
        assert!(
            non_numeric_target
                .message
                .contains("numeric cast requires a numeric target")
        );
    }

    #[test]
    fn operator_type_diagnostics_highlight_the_incompatible_operand() {
        let arithmetic_source = "fun main() { left: i32 = 1 right: i64 = 2 echo(left + right) }";
        let arithmetic = error(arithmetic_source);
        let right_start = arithmetic_source
            .rfind("right")
            .expect("right operand is present");
        assert_eq!(arithmetic.code, "R0206");
        assert_eq!(arithmetic.span.start, right_start);
        assert_eq!(arithmetic.span.end, right_start + "right".len());
        assert_eq!(
            arithmetic.message,
            "arithmetic operands must have matching numeric types, found `i32` and `i64`"
        );
        assert_eq!(
            arithmetic.help.as_deref(),
            Some("use matching numeric types; Ryn does not implicitly convert numeric values")
        );

        let comparison_source = "fun main() { left: i32 = 1 right: i64 = 2 echo(left < right) }";
        let comparison = error(comparison_source);
        let right_start = comparison_source
            .rfind("right")
            .expect("right comparison operand is present");
        assert_eq!(comparison.code, "R0206");
        assert_eq!(comparison.span.start, right_start);
        assert_eq!(comparison.span.end, right_start + "right".len());
        assert_eq!(
            comparison.message,
            "ordering operands are incompatible: found `i32` and `i64`"
        );
        assert_eq!(
            comparison.help.as_deref(),
            Some("use numeric values with ordering operators such as `<` and `>=`")
        );

        let logical_source = "fun main() { left: bool = true right: i32 = 1 echo(left && right) }";
        let logical = error(logical_source);
        let right_start = logical_source
            .rfind("right")
            .expect("right logical operand is present");
        assert_eq!(logical.code, "R0206");
        assert_eq!(logical.span.start, right_start);
        assert_eq!(logical.span.end, right_start + "right".len());
        assert_eq!(
            logical.message,
            "logical operators require `bool` operands, found `bool` and `i32`"
        );
        assert_eq!(
            logical.help.as_deref(),
            Some("use boolean values or comparisons with `&&` and `||`")
        );

        let bad_left_source =
            "fun main() { text: str = \"x\" number: i32 = 1 echo(text + number) }";
        let bad_left = error(bad_left_source);
        let text_start = bad_left_source
            .rfind("text")
            .expect("left arithmetic operand is present");
        assert_eq!(bad_left.code, "R0206");
        assert_eq!(bad_left.span.start, text_start);
        assert_eq!(bad_left.span.end, text_start + "text".len());
        assert_eq!(
            bad_left.message,
            "arithmetic operands must have matching numeric types, found `str` and `i32`"
        );
        assert_eq!(
            bad_left.help.as_deref(),
            Some("use numeric operands for arithmetic operators")
        );
    }

    #[test]
    fn equality_accepts_matching_bool_and_str_but_ordering_stays_numeric() {
        analyze(
            parse("fun main() { echo(true == false) echo(\"a\" != \"b\") }")
                .expect("source parses"),
        )
        .expect("bool and string equality are supported");

        let mixed = error("fun main() { echo(true == 1) }");
        assert_eq!(mixed.code, "R0206");
        assert_eq!(
            mixed.message,
            "equality operands are incompatible: found `bool` and `i64`"
        );
        assert_eq!(
            mixed.help.as_deref(),
            Some("compare values with the same type")
        );

        let ordered = error("fun main() { echo(true < false) }");
        assert_eq!(ordered.code, "R0206");
        assert_eq!(
            ordered.message,
            "ordering operands are incompatible: found `bool` and `bool`"
        );
        assert_eq!(
            ordered.help.as_deref(),
            Some("use numeric values with ordering operators such as `<` and `>=`")
        );
    }

    #[test]
    fn validates_small_and_unsigned_integer_ranges() {
        analyze(
            parse(
                "fun main() { min: i8 = -128 max: u64 = 18446744073709551615 echo(min) echo(max) }",
            )
            .expect("source parses"),
        )
        .expect("representable boundary values are accepted");
        let too_large_i8 = error("fun main() { value: i8 = 128 }");
        assert_eq!(too_large_i8.code, "R0206");
        assert_eq!(
            too_large_i8.help.as_deref(),
            Some("use a value from `-128` to `127`, or choose a wider integer type")
        );
        assert_eq!(error("fun main() { value: u8 = 256 }").code, "R0206");
        assert_eq!(
            error("fun main() { value: i64 = 9223372036854775808 }").code,
            "R0206"
        );
        let unsigned_negation = error("fun main() { value: u8 = 1 echo(-value) }");
        assert_eq!(unsigned_negation.code, "R0206");
        assert_eq!(
            unsigned_negation.help.as_deref(),
            Some(
                "declare the value with a signed integer or floating-point type before negating it"
            )
        );
        let invalid_not = error("fun main() { echo(!1) }");
        assert_eq!(invalid_not.code, "R0206");
        assert_eq!(
            invalid_not.help.as_deref(),
            Some("use `!` with a boolean value or comparison")
        );
    }

    #[test]
    fn accepts_i32_minimum_literal() {
        analyze(parse("fun main() { value: i32 = -2147483648 echo(value) }").expect("parses"))
            .expect("i32 minimum is representable");
    }

    #[test]
    fn rejects_out_of_range_f32_and_mixed_float_types() {
        let out_of_range = error("fun main() { value: f32 = 3.5e38 }");
        assert_eq!(out_of_range.code, "R0206");
        assert_eq!(
            out_of_range.help.as_deref(),
            Some("use a smaller finite value or declare it as `f64`")
        );
        assert_eq!(
            error("fun main() { narrow: f32 = 1.0 wide: f64 = 2.0 echo(narrow + wide) }").code,
            "R0206"
        );
        assert_eq!(
            error("fun main() { integer: i32 = 1 decimal: f32 = 2.0 echo(integer + decimal) }")
                .code,
            "R0206"
        );
        let modulo_source = "fun main() { echo(1.0 % 0.5) }";
        let modulo = error(modulo_source);
        let expression_start = modulo_source.find("1.0 % 0.5").expect("modulo expression");
        assert_eq!(modulo.code, "R0206");
        assert_eq!(modulo.span.start, expression_start);
        assert_eq!(modulo.span.end, expression_start + "1.0 % 0.5".len());
        assert_eq!(
            modulo.message,
            "`%` requires integer operands, found `f64` and `f64`"
        );
        assert_eq!(
            modulo.help.as_deref(),
            Some("use `%` only with integer operands; use `/` for floating-point division")
        );
    }
    #[test]
    fn rejects_use_before_declaration() {
        assert_eq!(error("fun main() { echo(missing) }").code, "R0203");
    }

    #[test]
    fn suggests_a_close_variable_name_only_when_the_match_is_unambiguous() {
        let diagnostic = error("fun main() { count := 1 echo(cout) }");
        assert_eq!(diagnostic.code, "R0203");
        assert_eq!(diagnostic.help.as_deref(), Some("did you mean `count`?"));

        let ambiguous = error("fun main() { count := 1 coat := 2 echo(cout) }");
        assert_eq!(ambiguous.code, "R0203");
        assert!(ambiguous.help.is_none());

        let assignment = error("fun main() { mut count := 1 cout = 2 }");
        assert_eq!(assignment.code, "R0203");
        assert_eq!(assignment.help.as_deref(), Some("did you mean `count`?"));

        let compound_assignment = error("fun main() { mut count := 1 cout += 2 }");
        assert_eq!(compound_assignment.code, "R0203");
        assert_eq!(
            compound_assignment.help.as_deref(),
            Some("did you mean `count`?")
        );
    }

    #[test]
    fn suggests_a_close_function_name() {
        let diagnostic =
            error("fun calculate(value: i64) -> i64 { value } fun main() { echo(calculat(1)) }");
        assert_eq!(diagnostic.code, "R0210");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("did you mean `calculate`?")
        );

        let builtin = error("fun main() { argg(0) }");
        assert_eq!(builtin.code, "R0210");
        assert_eq!(builtin.help.as_deref(), Some("did you mean `arg`?"));

        let builtin_with_underscore = error("fun main() { arg_coun(0) }");
        assert_eq!(
            builtin_with_underscore.help.as_deref(),
            Some("did you mean `arg_count`?")
        );
    }

    #[test]
    fn bounds_edit_distance_work_for_long_or_distant_names() {
        assert_eq!(
            closest_name("missing", ["count", "label"].into_iter()),
            None
        );
        assert_eq!(closest_name(&"x".repeat(65), ["x"].into_iter()), None);
        assert_eq!(edit_distance_within("widht", "width", 1), Some(1));
        assert_eq!(edit_distance_within("coutn", "count", 1), Some(1));
        assert_eq!(edit_distance_within("width", "widht", 0), None);
    }
    #[test]
    fn checks_explicit_local_types() {
        let diagnostic = error("fun main() { value: i64 = \"wrong\" }");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("make the initializer a `i64` value or change the type annotation")
        );
    }

    #[test]
    fn type_mismatch_diagnostics_highlight_the_offending_value() {
        let initializer_source = "fun main() { value: i32 = \"wrong\" }";
        let initializer = error(initializer_source);
        let initializer_start = initializer_source
            .find("\"wrong\"")
            .expect("initializer is present");
        assert_eq!(initializer.code, "R0205");
        assert_eq!(initializer.span.start, initializer_start);
        assert_eq!(initializer.span.end, initializer_start + "\"wrong\"".len());

        let assignment_source = "fun main() { mut value: i32 = 1 value = \"wrong\" }";
        let assignment = error(assignment_source);
        let assignment_start = assignment_source
            .find("\"wrong\"")
            .expect("assigned value is present");
        assert_eq!(assignment.code, "R0205");
        assert_eq!(assignment.span.start, assignment_start);
        assert_eq!(assignment.span.end, assignment_start + "\"wrong\"".len());

        let argument_source = "fun take(value: i32) {} fun main() { take(\"wrong\") }";
        let argument = error(argument_source);
        let argument_start = argument_source
            .find("\"wrong\"")
            .expect("argument is present");
        assert_eq!(argument.code, "R0212");
        assert_eq!(argument.span.start, argument_start);
        assert_eq!(argument.span.end, argument_start + "\"wrong\"".len());
    }

    #[test]
    fn unknown_interpolation_variable_highlights_its_source_name() {
        let source = r#"fun main() { echo("λ\n{missing}") }"#;
        let diagnostic = error(source);
        let name_start = source
            .find("missing")
            .expect("interpolation name is present");
        assert_eq!(diagnostic.code, "R0203");
        assert_eq!(diagnostic.span.start, name_start);
        assert_eq!(diagnostic.span.end, name_start + "missing".len());
    }
    #[test]
    fn requires_boolean_conditions() {
        let source = "fun main() { when 1 { echo(1) } }";
        let diagnostic = error(source);
        assert_eq!(diagnostic.code, "R0207");
        let condition_start = source.find("when 1").expect("when condition is present") + 5;
        assert_eq!(diagnostic.span.start, condition_start);
        assert_eq!(diagnostic.span.end, condition_start + 1);
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("use a boolean expression, such as a comparison, for the condition")
        );
    }
    #[test]
    fn branch_local_does_not_escape_its_scope() {
        assert_eq!(
            error("fun main() { when true { hidden := 1 } echo(hidden) }").code,
            "R0203"
        );
    }
    #[test]
    fn requires_boolean_loop_conditions() {
        let source = "fun main() { while 1 { echo(1) } }";
        let diagnostic = error(source);
        assert_eq!(diagnostic.code, "R0207");
        let condition_start = source.find("while 1").expect("while condition is present") + 6;
        assert_eq!(diagnostic.span.start, condition_start);
        assert_eq!(diagnostic.span.end, condition_start + 1);
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("use a boolean expression, such as a comparison, for the condition")
        );
    }

    #[test]
    fn rejects_loop_control_outside_a_while_loop() {
        let break_diagnostic = error("fun main() { break }");
        assert_eq!(break_diagnostic.code, "R0016");
        assert_eq!(
            break_diagnostic.message,
            "`break` can only be used inside a loop"
        );

        let continue_diagnostic = error("fun main() { continue }");
        assert_eq!(continue_diagnostic.code, "R0017");
        assert_eq!(
            continue_diagnostic.message,
            "`continue` can only be used inside a loop"
        );
    }
    #[test]
    fn validates_function_call_arguments() {
        let diagnostic =
            error("fun add(value: i64) -> i64 { value } fun main() { echo(add(\"wrong\")) }");
        assert_eq!(diagnostic.code, "R0212");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass a `i64` value to `add`")
        );
    }
    #[test]
    fn checks_function_arity() {
        let diagnostic =
            error("fun add(a: i64, b: i64) -> i64 { a + b } fun main() { echo(add(1)) }");
        assert_eq!(diagnostic.code, "R0211");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass exactly 2 arguments to `add`")
        );
    }
    #[test]
    fn rejects_unknown_function_calls() {
        assert_eq!(error("fun main() { echo(missing()) }").code, "R0210");
    }
    #[test]
    fn checks_function_return_type() {
        let diagnostic = error("fun answer() -> i64 { \"wrong\" } fun main() { echo(1) }");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i64` value or change the function's declared result type")
        );
    }

    #[test]
    fn checks_explicit_return_statement_types() {
        let diagnostic = error("fun answer() -> i32 { return \"wrong\" } fun main() {}");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i32` value or change the function's declared result type")
        );
    }

    #[test]
    fn requires_a_result_type_for_explicit_return_statements() {
        let diagnostic = error("fun answer() { return 42 } fun main() {}");
        assert_eq!(diagnostic.code, "R0013");
        assert!(diagnostic.message.contains("must declare its type"));
    }

    #[test]
    fn accepts_bare_return_in_void_functions() {
        let source = "fun notify(enabled: bool) { when !enabled { return } echo(1) } fun main() { notify(false) }";
        assert!(analyze(parse(source).expect("source parses")).is_ok());
    }

    #[test]
    fn requires_a_value_when_returning_from_a_value_function() {
        let diagnostic = error("fun answer() -> i32 { return } fun main() {}");
        assert_eq!(diagnostic.code, "R0216");
        assert_eq!(diagnostic.help.as_deref(), Some("return a `i32` value"));
    }

    #[test]
    fn accepts_functions_that_return_from_every_if_else_branch() {
        let source = "fun answer(flag: bool) -> i32 { when flag { return 1 } else { return 2 } } fun main() {}";
        assert!(analyze(parse(source).expect("source parses")).is_ok());

        let partial =
            error("fun answer(flag: bool) -> i32 { when flag { return 1 } } fun main() {}");
        assert_eq!(partial.code, "R0213");
    }

    #[test]
    fn suggests_returning_a_value_from_non_void_functions() {
        let diagnostic = error("fun answer() -> i64 {} fun main() {}");
        assert_eq!(diagnostic.code, "R0213");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i64` value from this function")
        );
    }

    #[test]
    fn suggests_a_result_type_for_tail_expressions() {
        let diagnostic = error("fun answer() { 42 } fun main() {}");
        assert_eq!(diagnostic.code, "R0214");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("add a `-> TYPE` result type to `answer` or remove its result expression")
        );
    }

    #[test]
    fn permits_void_calls_as_statements_but_not_values() {
        let source = "fun notify() { echo(\"hi\") } fun main() { notify() }";
        assert!(analyze(parse(source).expect("parses")).is_ok());
        let diagnostic = error("fun notify() { echo(\"hi\") } fun main() { echo(notify()) }");
        assert_eq!(diagnostic.code, "R0215");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("call `notify` as a statement or give it a return type and value")
        );
    }

    #[test]
    fn structures_validate_declarations_literals_and_field_mutability() {
        let valid = "struct Pair { left: i32, right: str } struct Boxed { pair: Pair, enabled: bool } fun identity(value: Boxed) -> Boxed { return value } fun main() { mut boxed := Boxed { enabled: true, pair: Pair { right: \"r\", left: 3 } } boxed.pair.left = 4 echo(identity(boxed).pair.left) }";
        analyze(parse(valid).expect("valid structure program parses"))
            .expect("nested fields and by-value function signatures type-check");

        assert_eq!(error("struct Empty {} fun main() {}").code, "R0221");
        assert_eq!(
            error("struct A { x: i32, x: bool } fun main() {}").code,
            "R0222"
        );
        assert_eq!(error("struct A { x: Missing } fun main() {}").code, "R0230");
        assert_eq!(error("struct A { a: A } fun main() {}").code, "R0232");
        assert_eq!(
            error("struct A { b: B } struct B { a: A } fun main() {}").code,
            "R0232"
        );
        let mut deeply_nested = (0..=MAX_STRUCT_NESTING + 1)
            .map(|index| {
                if index == MAX_STRUCT_NESTING + 1 {
                    format!("struct S{index} {{ value: i32 }}")
                } else {
                    format!("struct S{index} {{ child: S{} }}", index + 1)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        deeply_nested.push_str(" fun main() {}");
        assert_eq!(error(&deeply_nested).code, "R0233");
        assert_eq!(
            error("struct A { x: i32, y: i32 } fun main() { a := A { x: 1 } }").code,
            "R0229"
        );
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { x: 1, x: 2 } }").code,
            "R0228"
        );
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { y: 1 } }").code,
            "R0225"
        );
        let literal_field_typo = error("struct A { width: i32 } fun main() { a := A { widt: 1 } }");
        assert_eq!(literal_field_typo.code, "R0225");
        assert_eq!(
            literal_field_typo.help.as_deref(),
            Some("did you mean `width`?")
        );
        let access_field_typo =
            error("struct A { width: i32 } fun main() { a := A { width: 1 } echo(a.widt) }");
        assert_eq!(access_field_typo.code, "R0225");
        assert_eq!(
            access_field_typo.help.as_deref(),
            Some("did you mean `width`?")
        );
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { x: true } }").code,
            "R0205"
        );
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { x: 1 } a.x = 2 }").code,
            "R0204"
        );
        analyze(
            parse("struct A { x: i32 } fun main() { a := A { x: 1 } echo(a) echo(\"a={a}\") }")
                .expect("structure printing source parses"),
        )
        .expect("structures may be printed directly or interpolated");
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { x: 1 } echo(a.missing) }").code,
            "R0225"
        );
        assert_eq!(
            error("fun main() { value := 1 echo(value.field) }").code,
            "R0224"
        );
        assert_eq!(
            error("struct A { x: i32 } fun main() { mut a := A { x: 1 } a.x.y = 2 }").code,
            "R0224"
        );
        assert_eq!(
            error(
                "struct A { text: str } fun main() { mut a := A { text: \"x\" } a.text += \"y\" }"
            )
            .code,
            "R0206"
        );
        assert_eq!(
            error("struct A { count: i32 } fun main() { mut a := A { count: 1 } a.count += 0.5 }")
                .code,
            "R0206"
        );
        let structure_equality = "struct A { x: i32 } fun main() { a := A { x: 1 } b := A { x: 1 } echo(a == b) echo(a != b) }";
        analyze(parse(structure_equality).expect("structure equality source parses"))
            .expect("matching structure values support equality");
        assert_eq!(
            error("struct A { x: i32 } fun main() { a := A { x: 1 } b := A { x: 2 } echo(a < b) }")
                .code,
            "R0206"
        );
        assert_eq!(
            error("struct A { x: i32 } struct B { x: i32 } fun main() { a := A { x: 1 } b := B { x: 1 } echo(a == b) }").code,
            "R0206"
        );
    }
}
